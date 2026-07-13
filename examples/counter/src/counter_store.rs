#[cfg(target_arch = "wasm32")]
use std::time::Duration;

use http::Uri;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};
use thiserror::Error;

const STORE_URL_ENV: &str = "COUNTER_STORE_URL";
const COUNTER_ID_ENV: &str = "COUNTER_ID";
const DEFAULT_COUNTER_ID: &str = "demo";
#[cfg(target_arch = "wasm32")]
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_arch = "wasm32")]
const MAX_RESPONSE_BYTES: usize = 4 * 1024;

#[derive(Debug, Error)]
pub(crate) enum CounterStoreError {
    #[error("counter store configuration is invalid")]
    InvalidConfiguration,
    #[cfg(target_arch = "wasm32")]
    #[error("counter store request failed")]
    RequestFailed,
    #[cfg(target_arch = "wasm32")]
    #[error("counter store request timed out")]
    Timeout,
    #[cfg(target_arch = "wasm32")]
    #[error("counter store response is invalid")]
    InvalidResponse,
    #[cfg(target_arch = "wasm32")]
    #[error("counter store response exceeded its size limit")]
    ResponseTooLarge,
    #[cfg(not(target_arch = "wasm32"))]
    #[error("counter store is unsupported on this target")]
    UnsupportedTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoreConfig {
    authority: String,
    counter_id: String,
}

impl StoreConfig {
    fn parse(url: &str, counter_id: &str) -> Result<Self, CounterStoreError> {
        let uri = url
            .parse::<Uri>()
            .map_err(|_| CounterStoreError::InvalidConfiguration)?;
        if uri.scheme_str() != Some("http")
            || uri.query().is_some()
            || !matches!(uri.path(), "" | "/")
        {
            return Err(CounterStoreError::InvalidConfiguration);
        }
        let authority = uri
            .authority()
            .ok_or(CounterStoreError::InvalidConfiguration)?;
        let host = authority.host();
        if !matches!(host, "127.0.0.1" | "localhost")
            || authority.port_u16().is_none()
            || !valid_identifier(counter_id)
        {
            return Err(CounterStoreError::InvalidConfiguration);
        }
        Ok(Self {
            authority: authority.as_str().to_owned(),
            counter_id: counter_id.to_owned(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn path(&self, suffix: &str) -> String {
        format!("/v1/counters/{}{}", self.counter_id, suffix)
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Deserialize)]
struct CounterResponse {
    value: u32,
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Serialize)]
struct IncrementRequest<'a> {
    operation_id: &'a str,
}

pub(crate) async fn read_configured_count()
-> Result<Option<u32>, CounterStoreError> {
    let Some(config) = load_config()? else {
        return Ok(None);
    };
    request(&config, RequestKind::Read).await.map(Some)
}

pub(crate) async fn increment_configured_count(
    operation_id: &str,
) -> Result<Option<u32>, CounterStoreError> {
    let Some(config) = load_config()? else {
        return Ok(None);
    };
    if !valid_identifier(operation_id) {
        return Err(CounterStoreError::InvalidConfiguration);
    }
    request(&config, RequestKind::Increment(operation_id.to_owned()))
        .await
        .map(Some)
}

fn load_config() -> Result<Option<StoreConfig>, CounterStoreError> {
    let environment = wasip3::cli::environment::get_environment();
    let Some(url) = unique_environment_value(&environment, STORE_URL_ENV)?
    else {
        return Ok(None);
    };
    let counter_id = unique_environment_value(&environment, COUNTER_ID_ENV)?
        .unwrap_or(DEFAULT_COUNTER_ID);
    StoreConfig::parse(url, counter_id).map(Some)
}

fn unique_environment_value<'a>(
    environment: &'a [(String, String)],
    key: &str,
) -> Result<Option<&'a str>, CounterStoreError> {
    let mut values = environment
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str());
    let value = values.next();
    if values.next().is_some() {
        return Err(CounterStoreError::InvalidConfiguration);
    }
    Ok(value)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
        })
}

enum RequestKind {
    Read,
    Increment(String),
}

async fn request(
    config: &StoreConfig,
    kind: RequestKind,
) -> Result<u32, CounterStoreError> {
    #[cfg(target_arch = "wasm32")]
    {
        use futures::future::{Either, select};

        let operation = Box::pin(send_request(config, kind));
        let timeout_nanos = u64::try_from(OPERATION_TIMEOUT.as_nanos())
            .map_err(|_| CounterStoreError::InvalidConfiguration)?;
        let deadline =
            Box::pin(wasip3::clocks::monotonic_clock::wait_for(timeout_nanos));
        match select(operation, deadline).await {
            Either::Left((result, _deadline)) => result,
            Either::Right(((), _operation)) => Err(CounterStoreError::Timeout),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = config;
        if let RequestKind::Increment(operation_id) = kind {
            let _ = operation_id;
        }
        Err(CounterStoreError::UnsupportedTarget)
    }
}

#[cfg(target_arch = "wasm32")]
async fn send_request(
    config: &StoreConfig,
    kind: RequestKind,
) -> Result<u32, CounterStoreError> {
    use wasip3::{
        http::{
            client,
            types::{
                ErrorCode, Headers, Method, Request, RequestOptions, Scheme,
            },
        },
        wit_future, wit_stream,
    };

    let (method, path, body) = match kind {
        RequestKind::Read => (Method::Get, config.path(""), Vec::new()),
        RequestKind::Increment(operation_id) => {
            let body = serde_json::to_vec(&IncrementRequest {
                operation_id: &operation_id,
            })
            .map_err(|_| CounterStoreError::RequestFailed)?;
            (Method::Post, config.path("/increment"), body)
        }
    };
    let mut header_fields = vec![
        ("accept".to_owned(), b"application/json".to_vec()),
        (
            "content-length".to_owned(),
            body.len().to_string().into_bytes(),
        ),
    ];
    if !body.is_empty() {
        header_fields
            .push(("content-type".to_owned(), b"application/json".to_vec()));
    }
    let headers = Headers::from_list(&header_fields)
        .map_err(|_| CounterStoreError::RequestFailed)?;
    let timeout_nanos = u64::try_from(OPERATION_TIMEOUT.as_nanos())
        .map_err(|_| CounterStoreError::InvalidConfiguration)?;
    let options = RequestOptions::new();
    options
        .set_connect_timeout(Some(timeout_nanos))
        .and_then(|()| options.set_first_byte_timeout(Some(timeout_nanos)))
        .and_then(|()| options.set_between_bytes_timeout(Some(timeout_nanos)))
        .map_err(|_| CounterStoreError::RequestFailed)?;

    let (mut body_writer, body_reader) = wit_stream::new();
    let (body_result_writer, body_result) =
        wit_future::new(|| Err(ErrorCode::InternalError(None)));
    let (request, transmission_result) =
        Request::new(headers, Some(body_reader), body_result, Some(options));
    request
        .set_method(&method)
        .and_then(|()| request.set_scheme(Some(&Scheme::Http)))
        .and_then(|()| request.set_authority(Some(&config.authority)))
        .and_then(|()| request.set_path_with_query(Some(&path)))
        .map_err(|()| CounterStoreError::RequestFailed)?;

    let write_body = async move {
        let remaining = body_writer.write_all(body).await;
        if !remaining.is_empty() {
            return Err(CounterStoreError::RequestFailed);
        }
        drop(body_writer);
        body_result_writer
            .write(Ok(None))
            .await
            .map_err(|_| CounterStoreError::RequestFailed)
    };
    let send_and_collect = async move {
        let response = client::send(request).await.map_err(map_http_error)?;
        collect_response(response).await
    };
    let await_transmission =
        async move { transmission_result.await.map_err(map_http_error) };
    let (value, (), ()) =
        futures::try_join!(send_and_collect, write_body, await_transmission)?;
    Ok(value)
}

#[cfg(target_arch = "wasm32")]
async fn collect_response(
    response: wasip3::http::types::Response,
) -> Result<u32, CounterStoreError> {
    use wasip3::{
        http::types::{ErrorCode, Response},
        wit_bindgen::StreamResult,
        wit_future,
    };

    if response.get_status_code() != 200 {
        return Err(CounterStoreError::RequestFailed);
    }
    let (result_writer, body_result) =
        wit_future::new(|| Err(ErrorCode::InternalError(None)));
    let (mut body, trailers) = Response::consume_body(response, body_result);
    let mut collected = Vec::new();
    loop {
        let (status, chunk) = body.read(Vec::with_capacity(1024)).await;
        if collected.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(CounterStoreError::ResponseTooLarge);
        }
        collected.extend_from_slice(&chunk);
        match status {
            StreamResult::Complete(_) => {}
            StreamResult::Dropped => {
                trailers.await.map_err(map_http_error)?;
                result_writer
                    .write(Ok(()))
                    .await
                    .map_err(|_| CounterStoreError::RequestFailed)?;
                return serde_json::from_slice::<CounterResponse>(&collected)
                    .map(|response| response.value)
                    .map_err(|_| CounterStoreError::InvalidResponse);
            }
            StreamResult::Cancelled => {
                return Err(CounterStoreError::RequestFailed);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn map_http_error(error: wasip3::http::types::ErrorCode) -> CounterStoreError {
    use wasip3::http::types::ErrorCode;

    if matches!(
        error,
        ErrorCode::DnsTimeout
            | ErrorCode::ConnectionTimeout
            | ErrorCode::ConnectionReadTimeout
            | ErrorCode::ConnectionWriteTimeout
            | ErrorCode::HttpResponseTimeout
    ) {
        CounterStoreError::Timeout
    } else {
        CounterStoreError::RequestFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_accepts_only_explicit_loopback_http() {
        let config = StoreConfig::parse("http://127.0.0.1:4040", "demo")
            .expect("loopback configuration should be accepted");
        assert_eq!(config.authority, "127.0.0.1:4040");
    }

    #[test]
    fn config_rejects_remote_or_ambiguous_destinations() {
        for url in [
            "https://127.0.0.1:4040",
            "http://store.example:4040",
            "http://127.0.0.1",
            "http://127.0.0.1:4040/nested",
            "http://127.0.0.1:4040?query=forbidden",
        ] {
            assert!(StoreConfig::parse(url, "demo").is_err(), "accepted {url}");
        }
    }

    #[test]
    fn identifiers_reject_paths_controls_and_oversized_values() {
        for value in ["", "../demo", "demo/name", "demo\nname"] {
            assert!(!valid_identifier(value), "accepted {value:?}");
        }
        assert!(!valid_identifier(&"a".repeat(129)));
    }
}
