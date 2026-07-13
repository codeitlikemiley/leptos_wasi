use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use tracing_subscriber::EnvFilter;

const MAX_IDENTIFIER_LENGTH: usize = 128;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
}

#[derive(Debug, Deserialize)]
struct IncrementRequest {
    operation_id: String,
}

#[derive(Debug, Serialize)]
struct CounterResponse {
    value: i64,
}

#[derive(Debug, Error)]
enum StoreError {
    #[error("invalid request")]
    InvalidRequest,
    #[error("counter store unavailable")]
    Unavailable(#[from] sqlx::Error),
}

impl IntoResponse for StoreError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        (
            status,
            [("cache-control", "no-store")],
            status.canonical_reason().unwrap_or("error"),
        )
            .into_response()
    }
}

fn validate_identifier(value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_LENGTH
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(StoreError::InvalidRequest);
    }
    Ok(())
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn read_counter(
    State(state): State<AppState>,
    Path(counter_id): Path<String>,
) -> Result<Json<CounterResponse>, StoreError> {
    validate_identifier(&counter_id)?;
    let value = sqlx::query_scalar::<_, i64>(
        "SELECT value FROM counters WHERE counter_id = ?1",
    )
    .bind(counter_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(0);
    Ok(Json(CounterResponse { value }))
}

async fn increment_counter(
    State(state): State<AppState>,
    Path(counter_id): Path<String>,
    Json(request): Json<IncrementRequest>,
) -> Result<Json<CounterResponse>, StoreError> {
    validate_identifier(&counter_id)?;
    validate_identifier(&request.operation_id)?;

    let mut transaction = state.pool.begin().await?;
    sqlx::query("INSERT INTO counters (counter_id, value) VALUES (?1, 0) ON CONFLICT DO NOTHING")
        .bind(&counter_id)
        .execute(&mut *transaction)
        .await?;

    let reserved = sqlx::query_scalar::<_, i32>(
        "INSERT INTO counter_operations (counter_id, operation_id) VALUES (?1, ?2) \
         ON CONFLICT DO NOTHING RETURNING 1",
    )
    .bind(&counter_id)
    .bind(&request.operation_id)
    .fetch_optional(&mut *transaction)
    .await?
    .is_some();

    let value = if reserved {
        let value = sqlx::query_scalar::<_, i64>(
            "UPDATE counters SET value = value + 1 WHERE counter_id = ?1 RETURNING value",
        )
        .bind(&counter_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE counter_operations SET result_value = ?3 \
             WHERE counter_id = ?1 AND operation_id = ?2",
        )
        .bind(&counter_id)
        .bind(&request.operation_id)
        .bind(value)
        .execute(&mut *transaction)
        .await?;
        value
    } else {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT result_value FROM counter_operations \
             WHERE counter_id = ?1 AND operation_id = ?2",
        )
        .bind(&counter_id)
        .bind(&request.operation_id)
        .fetch_one(&mut *transaction)
        .await?
        .ok_or(StoreError::InvalidRequest)?
    };

    transaction.commit().await?;
    Ok(Json(CounterResponse { value }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .try_init()?;
    let database_url = env::var("DATABASE_URL")?;
    let listen_addr: SocketAddr = env::var("COUNTER_STORE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:4040".to_owned())
        .parse()?;
    let connect_options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    // A single connection serializes the reference counter's short write
    // transactions and avoids turning SQLite lock contention into retries.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options)
        .await?;
    sqlx::migrate!().run(&pool).await?;

    let application = Router::new()
        .route("/health", get(health))
        .route("/v1/counters/{counter_id}", get(read_counter))
        .route(
            "/v1/counters/{counter_id}/increment",
            post(increment_counter),
        )
        .with_state(AppState { pool });
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(listener, application)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state(
        database_url: &str,
    ) -> Result<AppState, Box<dyn std::error::Error>> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(AppState { pool })
    }

    #[test]
    fn identifier_validation_accepts_bounded_safe_ascii() {
        assert!(validate_identifier("tenant.counter_1").is_ok());
    }

    #[test]
    fn identifier_validation_rejects_path_and_control_characters() {
        for value in ["", "../counter", "counter/name", "counter\nname"] {
            assert!(validate_identifier(value).is_err(), "accepted {value:?}");
        }
    }

    #[tokio::test]
    async fn increments_are_idempotent_concurrent_and_durable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database_url = format!(
            "sqlite://{}",
            directory.path().join("counter.db").display()
        );
        let state = test_state(&database_url).await?;
        let first = increment_counter(
            State(state.clone()),
            Path("demo".to_owned()),
            Json(IncrementRequest {
                operation_id: "same-operation".to_owned(),
            }),
        )
        .await?
        .0
        .value;
        let replay = increment_counter(
            State(state.clone()),
            Path("demo".to_owned()),
            Json(IncrementRequest {
                operation_id: "same-operation".to_owned(),
            }),
        )
        .await?
        .0
        .value;
        assert_eq!((first, replay), (1, 1));

        let mut increments = Vec::new();
        for index in 0..20 {
            increments.push(tokio::spawn(increment_counter(
                State(state.clone()),
                Path("demo".to_owned()),
                Json(IncrementRequest {
                    operation_id: format!("concurrent-{index}"),
                }),
            )));
        }
        for increment in increments {
            let _response = increment.await??;
        }
        state.pool.close().await;

        let restarted = test_state(&database_url).await?;
        let value =
            read_counter(State(restarted.clone()), Path("demo".to_owned()))
                .await?
                .0
                .value;
        assert_eq!(value, 21);
        restarted.pool.close().await;
        Ok(())
    }
}
