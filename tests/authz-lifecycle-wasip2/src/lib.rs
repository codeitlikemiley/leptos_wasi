//! Real-host `WASIp2` `AuthZEN` and `WaitPoll` lifecycle fixture.
//!
//! The component proves that the framework-neutral `WASIp2` authorization
//! transport can use `leptos_wasi`'s cancellation-safe pollable waiter.

#![deny(rustdoc::broken_intra_doc_links)]

use std::time::Duration;

use leptos_wasi::ExecutorError;
use thiserror::Error;
use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use wasi::io::poll::Pollable;
use wasi_authz_client::wasip2::{
    PollableWaitError, PollableWaitFuture, PollableWaiter, Wasip2Transport,
};
use wasi_authz_client::{
    AuthzenClient, AuthzenEndpoint, BearerAuthTransport, ClientError,
    TransportError,
};
use wasi_authz_contract::AccessEvaluation;

const CEDAR_EVALUATION: &[u8] = br#"{
  "subject":{"type":"anonymous","id":"anonymous","properties":{"wasi_authz":{"state":"anonymous"}}},
  "action":{"name":"http.request","properties":{"wasi_authz":{}}},
  "resource":{"type":"service","id":"leptos-wasi-counter","properties":{"wasi_authz":{}}},
  "context":{"wasi_authz":{"attributes":[
    {"name":"http_method","value":{"type":"string","value":"GET"},"provenance":"gateway"},
    {"name":"http_path","value":{"type":"string","value":"/"},"provenance":"gateway"}
  ],"request_id":"wasip2-cedar-allow"}}
}"#;

const TIMEOUT_EVALUATION: &[u8] = br#"{
  "subject":{"type":"anonymous","id":"anonymous","properties":{"wasi_authz":{"state":"anonymous"}}},
  "action":{"name":"http.request","properties":{"wasi_authz":{}}},
  "resource":{"type":"service","id":"leptos-wasi-counter","properties":{"wasi_authz":{}}},
  "context":{"wasi_authz":{"attributes":[
    {"name":"http_method","value":{"type":"string","value":"GET"},"provenance":"gateway"},
    {"name":"http_path","value":{"type":"string","value":"/"},"provenance":"gateway"}
  ],"request_id":"lifecycle-timeout-body"}}
}"#;

const CEDAR_URL_ENV: &str = "CEDAR_PDP_URL";
const CEDAR_TOKEN_ENV: &str = "CEDAR_PDP_TOKEN";
const FAULT_URL_ENV: &str = "FAULT_PDP_URL";
const FAULT_TOKEN_ENV: &str = "FAULT_PDP_TOKEN";
const TIMEOUT_MS_ENV: &str = "P2_AUTHZ_TIMEOUT_MS";

#[derive(Clone, Copy, Debug)]
struct LeptosPollableWaiter;

impl PollableWaiter for LeptosPollableWaiter {
    fn wait(&self, pollable: Pollable) -> PollableWaitFuture<'_> {
        Box::pin(async move {
            leptos_wasi::wasip2::WaitPoll::new(pollable).await.map_err(
                |error| match error {
                    ExecutorError::PollableCanceled
                    | ExecutorError::RunUntilCanceled => {
                        PollableWaitError::Canceled
                    }
                    _ => PollableWaitError::Unavailable,
                },
            )
        })
    }
}

struct Component;

impl wasi::exports::http::incoming_handler::Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request
            .path_with_query()
            .and_then(|path| path.split('?').next().map(str::to_owned))
            .unwrap_or_else(|| "/".to_owned());
        let baseline = pollable_queue_depth();
        let status = match path.as_str() {
            "/allow" | "/recovery" => run_cedar_evaluation().map_or_else(
                |error| classify_error(&error),
                |allowed| if allowed { 204 } else { 403 },
            ),
            "/timeout" => match run_timeout_evaluation() {
                Err(FixtureError::Client(ClientError::Transport(
                    TransportError::Timeout,
                ))) => 504,
                Ok(_) => 500,
                Err(error) => classify_error(&error),
            },
            "/queue-depth" => {
                if baseline == 0 {
                    204
                } else {
                    500
                }
            }
            _ => 404,
        };
        let depth = pollable_queue_depth();
        let status = if depth == baseline && depth == 0 {
            status
        } else {
            500
        };
        send_empty(response_out, status, depth);
    }
}

fn run_cedar_evaluation() -> Result<bool, FixtureError> {
    run_evaluation(
        CEDAR_URL_ENV,
        CEDAR_TOKEN_ENV,
        CEDAR_EVALUATION,
        Duration::from_secs(2),
    )
}

fn run_timeout_evaluation() -> Result<bool, FixtureError> {
    let environment = wasi::cli::environment::get_environment();
    let timeout_ms = environment_value(&environment, TIMEOUT_MS_ENV)?
        .parse::<u64>()
        .map_err(|_| FixtureError::Configuration)?;
    if !(100..=1_000).contains(&timeout_ms) {
        return Err(FixtureError::Configuration);
    }
    run_evaluation_with_environment(
        &environment,
        FAULT_URL_ENV,
        FAULT_TOKEN_ENV,
        TIMEOUT_EVALUATION,
        Duration::from_millis(timeout_ms),
    )
}

fn run_evaluation(
    endpoint_name: &str,
    token_name: &str,
    encoded_evaluation: &[u8],
    timeout: Duration,
) -> Result<bool, FixtureError> {
    let environment = wasi::cli::environment::get_environment();
    run_evaluation_with_environment(
        &environment,
        endpoint_name,
        token_name,
        encoded_evaluation,
        timeout,
    )
}

fn run_evaluation_with_environment(
    environment: &[(String, String)],
    endpoint_name: &str,
    token_name: &str,
    encoded_evaluation: &[u8],
    timeout: Duration,
) -> Result<bool, FixtureError> {
    let endpoint = environment_value(environment, endpoint_name)?;
    let token = environment_value(environment, token_name)?;
    let endpoint = AuthzenEndpoint::new_loopback_for_dev(endpoint)
        .map_err(|_| FixtureError::Configuration)?;
    let transport = Wasip2Transport::new(LeptosPollableWaiter)
        .with_timeout(timeout)
        .map_err(|_| FixtureError::Configuration)?;
    let transport = BearerAuthTransport::new(transport, token)
        .map_err(|_| FixtureError::Configuration)?;
    let client = AuthzenClient::new(endpoint, transport);
    let evaluation = AccessEvaluation::from_json_slice(encoded_evaluation)
        .map_err(|_| FixtureError::Configuration)?;
    let executor = leptos_wasi::wasip2::init_wasip2_executor(
        leptos_wasi::wasip2::Mode::Preemptive,
    )
    .map_err(|_| FixtureError::Executor)?;
    executor
        .run_until(async move { client.evaluate(&evaluation).await })
        .map_err(|_| FixtureError::Executor)?
        .map(|decision| decision.is_allowed())
        .map_err(FixtureError::Client)
}

fn environment_value<'a>(
    environment: &'a [(String, String)],
    name: &str,
) -> Result<&'a str, FixtureError> {
    let mut values = environment
        .iter()
        .filter(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str());
    let value = values.next().ok_or(FixtureError::Configuration)?;
    if values.next().is_some() || value.is_empty() {
        return Err(FixtureError::Configuration);
    }
    Ok(value)
}

fn classify_error(error: &FixtureError) -> u16 {
    match error {
        FixtureError::Client(ClientError::Transport(
            TransportError::Timeout,
        )) => 504,
        FixtureError::Configuration
        | FixtureError::Executor
        | FixtureError::Client(_) => 503,
    }
}

fn pollable_queue_depth() -> usize {
    leptos_wasi::__private::wasip2_pollable_queue_depth()
}

fn send_empty(
    response_out: ResponseOutparam,
    status: u16,
    pollable_depth: usize,
) {
    let fields = Fields::from_list(&[
        ("cache-control".to_owned(), b"no-store".to_vec()),
        ("content-length".to_owned(), b"0".to_vec()),
        (
            "x-wasip2-pollable-depth".to_owned(),
            pollable_depth.to_string().into_bytes(),
        ),
    ])
    .unwrap_or_else(|_| Fields::new());
    let response = OutgoingResponse::new(fields);
    if response.set_status_code(status).is_err() {
        return;
    }
    let Ok(body) = response.body() else {
        return;
    };
    ResponseOutparam::set(response_out, Ok(response));
    let _result = OutgoingBody::finish(body, None);
}

#[derive(Debug, Error)]
enum FixtureError {
    #[error("invalid fixture configuration")]
    Configuration,
    #[error("WASIp2 executor failure")]
    Executor,
    #[error("authorization client failure")]
    Client(#[source] ClientError),
}

wasi::http::proxy::export!(Component);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_environment_values_fail_closed() {
        let environment = vec![
            (CEDAR_URL_ENV.to_owned(), "first".to_owned()),
            (CEDAR_URL_ENV.to_owned(), "second".to_owned()),
        ];

        assert!(matches!(
            environment_value(&environment, CEDAR_URL_ENV),
            Err(FixtureError::Configuration)
        ));
    }

    #[test]
    fn timeout_vector_selects_the_drip_body_fault() {
        let evaluation = AccessEvaluation::from_json_slice(TIMEOUT_EVALUATION)
            .expect("fixture evaluation is valid");
        let encoded = evaluation.to_json_vec().expect("evaluation encodes");
        let value: serde_json::Value =
            serde_json::from_slice(&encoded).expect("evaluation is JSON");

        assert_eq!(
            value["context"]["wasi_authz"]["request_id"],
            "lifecycle-timeout-body"
        );
    }
}
