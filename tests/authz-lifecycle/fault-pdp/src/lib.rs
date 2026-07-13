//! Deterministic final-WASIp3 `AuthZEN` failure service for lifecycle tests.
//!
//! The request ID in the bounded `AuthZEN` context selects one predeclared
//! outcome. Request bodies and credentials are never logged.

#![deny(rustdoc::broken_intra_doc_links)]

use http::header::AUTHORIZATION;
use http_body_util::BodyExt as _;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use wasip3::{
    clocks::monotonic_clock::wait_for,
    http::types::{ErrorCode, Headers, Request, Response},
    wit_future, wit_stream,
};

const ACCESS_EVALUATION_PATH: &str = "/access/v1/evaluation";
const AUTHORIZATION_VALUE: &[u8] = b"Bearer lifecycle-pdp-transport-token";
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const SLOW_RESPONSE_NANOS: u64 = 1_000_000_000;
const SATURATION_LIMIT: usize = 4;

static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_BODIES: AtomicUsize = AtomicUsize::new(0);
static SATURATION_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct Component;

wasip3::http::service::export!(Component);

impl wasip3::exports::http::handler::Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let request = wasip3::http_compat::http_from_wasi_request(request)?;
        let (parts, body) = request.into_parts();
        let body = match collect_request(body).await {
            Ok(body) => body,
            Err(error) => {
                eprintln!("authz-lifecycle-fault-pdp: request-body-error");
                return Err(error);
            }
        };
        if parts.method == http::Method::GET
            && parts
                .uri
                .path_and_query()
                .map(http::uri::PathAndQuery::as_str)
                == Some("/__authz_lifecycle/status")
        {
            return status_response();
        }
        if parts.method != http::Method::POST
            || parts
                .uri
                .path_and_query()
                .map(http::uri::PathAndQuery::as_str)
                != Some(ACCESS_EVALUATION_PATH)
        {
            return response(404, b"{}".to_vec());
        }
        let mut authorization = parts.headers.get_all(AUTHORIZATION).iter();
        let authorized = authorization
            .next()
            .is_some_and(|value| value.as_bytes() == AUTHORIZATION_VALUE)
            && authorization.next().is_none();
        if !authorized {
            return response(401, b"{}".to_vec());
        }
        if contains_forbidden_request_data(&body) {
            eprintln!("authz-lifecycle-fault-pdp: forbidden-data");
            return response(400, b"{}".to_vec());
        }
        let request_id =
            request_id(&body).ok_or(ErrorCode::HttpRequestDenied)?;
        let _request_guard = ActiveGuard::new(&ACTIVE_REQUESTS);
        eprintln!("authz-lifecycle-fault-pdp: request");

        if request_id == "lifecycle-provider-503" {
            eprintln!("authz-lifecycle-fault-pdp: provider-unavailable");
            return response(503, b"{}".to_vec());
        }
        if request_id == "lifecycle-malformed" {
            eprintln!("authz-lifecycle-fault-pdp: malformed");
            return response(200, b"not-json".to_vec());
        }
        if request_id == "lifecycle-timeout-first-byte"
            || request_id == "lifecycle-disconnect"
        {
            eprintln!("authz-lifecycle-fault-pdp: delayed-first-byte");
            wait_for(SLOW_RESPONSE_NANOS).await;
            return allow_response();
        }
        if request_id == "lifecycle-timeout-body" {
            eprintln!("authz-lifecycle-fault-pdp: delayed-body");
            return slow_body_response();
        }
        if is_saturation_request_id(&request_id) {
            eprintln!("authz-lifecycle-fault-pdp: saturated");
            let Some(_saturation_guard) =
                BoundedGuard::try_new(&SATURATION_IN_FLIGHT, SATURATION_LIMIT)
            else {
                return response(503, b"{}".to_vec());
            };
            wait_for(250_000_000).await;
            return response(503, b"{}".to_vec());
        }

        eprintln!("authz-lifecycle-fault-pdp: allow");
        allow_response()
    }
}

fn contains_forbidden_request_data(body: &[u8]) -> bool {
    [
        b"Bearer allow".as_slice(),
        b"middleware-secret-cookie-sentinel".as_slice(),
        b"middleware-secret-query-sentinel".as_slice(),
        b"middleware-secret-identity-sentinel".as_slice(),
    ]
    .iter()
    .any(|forbidden| {
        body.windows(forbidden.len())
            .any(|window| window == *forbidden)
    })
}

fn is_saturation_request_id(request_id: &str) -> bool {
    request_id
        .strip_prefix("lifecycle-saturation-case-")
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

struct ActiveGuard {
    counter: &'static AtomicUsize,
}

impl ActiveGuard {
    fn new(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

struct BoundedGuard {
    _active: ActiveGuard,
}

impl BoundedGuard {
    fn try_new(counter: &'static AtomicUsize, limit: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < limit).then_some(current + 1)
            })
            .ok()
            .map(|_| Self {
                _active: ActiveGuard { counter },
            })
    }
}

async fn collect_request(
    mut body: wasip3::http_compat::IncomingRequestBody,
) -> Result<Vec<u8>, ErrorCode> {
    let mut output = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        match frame.into_data() {
            Ok(data) => {
                if output.len().saturating_add(data.len()) > MAX_DOCUMENT_BYTES
                {
                    return Err(ErrorCode::HttpRequestBodySize(Some(
                        MAX_DOCUMENT_BYTES as u64,
                    )));
                }
                output.extend_from_slice(&data);
            }
            Err(frame) => frame
                .into_trailers()
                .map(drop)
                .map_err(|_| ErrorCode::HttpProtocolError)?,
        }
    }
    Ok(output)
}

fn request_id(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    value
        .get("context")?
        .get("wasi_authz")?
        .get("request_id")?
        .as_str()
        .map(str::to_owned)
}

fn allow_response() -> Result<Response, ErrorCode> {
    response(
        200,
        br#"{"decision":true,"context":{"wasi_authz":{"decision_id":"lifecycle-decision","policy_revision":"lifecycle-policy-v1"}}}"#
            .to_vec(),
    )
}

fn status_response() -> Result<Response, ErrorCode> {
    let body = format!(
        "{{\"active_requests\":{},\"active_bodies\":{},\"saturation_in_flight\":{}}}",
        ACTIVE_REQUESTS.load(Ordering::Acquire),
        ACTIVE_BODIES.load(Ordering::Acquire),
        SATURATION_IN_FLIGHT.load(Ordering::Acquire),
    )
    .into_bytes();
    response(200, body)
}

fn slow_body_response() -> Result<Response, ErrorCode> {
    let chunks = [
        br#"{"decision":"#.to_vec(),
        b"t".to_vec(),
        b"r".to_vec(),
        b"u".to_vec(),
        b"e}".to_vec(),
    ];
    let body_length = chunks.iter().map(Vec::len).sum();
    let headers = json_headers(body_length)?;
    let (mut writer, reader) = wit_stream::new();
    let (body_result_writer, body_result) =
        wit_future::new(|| Err(ErrorCode::InternalError(None)));
    wasip3::spawn(async move {
        let _body_guard = ActiveGuard::new(&ACTIVE_BODIES);
        for (index, chunk) in chunks.into_iter().enumerate() {
            if index > 0 {
                // Each inter-frame delay is below the transport's per-byte
                // timeout; only the single absolute operation deadline can
                // stop the complete response.
                wait_for(250_000_000).await;
            }
            if !writer.write_all(chunk).await.is_empty() {
                return;
            }
        }
        drop(writer);
        let _write_result = body_result_writer.write(Ok(None)).await;
    });
    finish_response(200, headers, Some(reader), body_result)
}

fn response(status: u16, body: Vec<u8>) -> Result<Response, ErrorCode> {
    let headers = json_headers(body.len())?;
    let (mut writer, reader) = wit_stream::new();
    let (body_result_writer, body_result) =
        wit_future::new(|| Err(ErrorCode::InternalError(None)));
    wasip3::spawn(async move {
        if writer.write_all(body).await.is_empty() {
            drop(writer);
            let _write_result = body_result_writer.write(Ok(None)).await;
        }
    });
    finish_response(status, headers, Some(reader), body_result)
}

fn json_headers(body_length: usize) -> Result<Headers, ErrorCode> {
    Headers::from_list(&[
        ("content-type".to_owned(), b"application/json".to_vec()),
        ("cache-control".to_owned(), b"no-store".to_vec()),
        (
            "content-length".to_owned(),
            body_length.to_string().into_bytes(),
        ),
    ])
    .map_err(|_| ErrorCode::HttpResponseHeaderSectionSize(None))
}

fn finish_response(
    status: u16,
    headers: Headers,
    body: Option<wasip3::wit_bindgen::StreamReader<u8>>,
    body_result: wasip3::wit_bindgen::FutureReader<
        Result<Option<Headers>, ErrorCode>,
    >,
) -> Result<Response, ErrorCode> {
    let (response, transmission_result) =
        Response::new(headers, body, body_result);
    wasip3::spawn(async move {
        let _result = transmission_result.await;
    });
    response
        .set_status_code(status)
        .map_err(|()| ErrorCode::InternalError(None))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_is_read_only_from_the_namespaced_context() {
        let body = br#"{"context":{"wasi_authz":{"request_id":"lifecycle-allow"}},"request_id":"ignored"}"#;

        assert_eq!(request_id(body).as_deref(), Some("lifecycle-allow"));
    }

    #[test]
    fn request_id_rejects_missing_or_malformed_documents() {
        assert_eq!(request_id(br#"{"context":{}}"#), None);
        assert_eq!(request_id(b"not-json"), None);
    }

    #[test]
    fn forbidden_transport_data_is_rejected_without_parsing_identity() {
        assert!(contains_forbidden_request_data(b"Bearer allow"));
        assert!(contains_forbidden_request_data(
            b"middleware-secret-query-sentinel"
        ));
        assert!(!contains_forbidden_request_data(
            b"middleware-secret-issuer-sentinel"
        ));
    }

    #[test]
    fn recovery_request_ids_do_not_match_fault_selectors() {
        assert!(is_saturation_request_id("lifecycle-saturation-case-31"));
        assert!(!is_saturation_request_id(
            "lifecycle-recovery-after-saturation-0"
        ));
        assert!(!is_saturation_request_id("lifecycle-saturation-case-"));
    }
}
