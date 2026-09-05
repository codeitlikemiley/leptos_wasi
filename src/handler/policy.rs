//! Request policy: size and time budgets, the errors that reject a
//! request, and the plain responses used to report them.

use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use thiserror::Error;

use crate::response::{Body, Response};

/// Default maximum request body size: 16 MiB.
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: usize = 16 * 1024 * 1024;

pub(super) const X_CONTENT_TYPE_OPTIONS: &str = "x-content-type-options";

/// Request policy applied while converting incoming WASI HTTP requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandlerConfig {
    max_request_body_size: usize,
    request_body_timeout_ns: Option<u64>,
}

impl HandlerConfig {
    /// Returns a copy configured with a different maximum request body size.
    #[must_use]
    pub const fn with_max_request_body_size(mut self, bytes: usize) -> Self {
        self.max_request_body_size = bytes;
        self
    }

    /// Returns the maximum accepted request body size in bytes.
    #[must_use]
    pub const fn max_request_body_size(&self) -> usize {
        self.max_request_body_size
    }

    /// Returns a copy that abandons a request body taking longer than
    /// `nanoseconds` to arrive in full.
    ///
    /// This is off by default, because the host, not the guest, is the layer
    /// that can bound a client it is still feeding. See the request contract in
    /// `PRODUCTION.md`: the size limit already applied here bounds how much a
    /// body may be, never how long it may take. Enable this when the ingress
    /// cannot supply a read deadline of its own, and treat it as defense in
    /// depth rather than the primary control.
    ///
    /// The budget covers the whole body rather than the gap between chunks, so
    /// a client trickling bytes indefinitely is bounded, and the two previews
    /// mean the same thing by it.
    #[must_use]
    pub const fn with_request_body_timeout_ns(
        mut self,
        nanoseconds: u64,
    ) -> Self {
        self.request_body_timeout_ns = Some(nanoseconds);
        self
    }

    /// Returns the configured whole-body read budget in nanoseconds.
    #[must_use]
    pub const fn request_body_timeout_ns(&self) -> Option<u64> {
        self.request_body_timeout_ns
    }
}

impl Default for HandlerConfig {
    fn default() -> Self {
        Self {
            max_request_body_size: DEFAULT_MAX_REQUEST_BODY_SIZE,
            request_body_timeout_ns: None,
        }
    }
}

/// Errors detected while registering static files or Leptos routes.
#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// The static-file URI prefix could not be parsed or is not absolute.
    #[error("invalid static-file URI prefix: {0}")]
    InvalidStaticPrefix(String),

    /// Route generation was requested more than once for one handler.
    #[error("routes have already been generated for this handler")]
    RoutesAlreadyGenerated,

    /// Two generated route definitions resolve to the same path pattern.
    #[error("duplicate generated route `{0}`")]
    DuplicateRoute(String),

    /// Static SSR is not supported by the component request handler.
    #[error("static SSR route `{0}` is not supported")]
    UnsupportedStaticSsr(String),

    /// A generated route could not be registered.
    #[error("failed to register route `{path}`: {reason}")]
    InvalidRoute {
        /// Route path that could not be registered.
        path: String,
        /// Parser-provided failure description.
        reason: String,
    },
}

/// Errors produced while validating request size headers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RequestPolicyError {
    /// A Content-Length value was not a valid unsigned byte count.
    #[error("invalid Content-Length header")]
    InvalidContentLength,
    /// Multiple Content-Length values did not agree.
    #[error("conflicting Content-Length headers")]
    ConflictingContentLength,
    /// The declared or collected body exceeded the configured limit.
    #[error("request body exceeds limit of {limit} bytes")]
    BodyTooLarge {
        /// Configured limit in bytes.
        limit: usize,
    },
    /// The body did not arrive in full within the configured budget.
    #[error("request body exceeded its {nanoseconds} ns read budget")]
    BodyReadTimeout {
        /// Configured whole-body read budget in nanoseconds.
        nanoseconds: u64,
    },
}

impl RequestPolicyError {
    pub(super) const fn status(&self) -> StatusCode {
        match self {
            Self::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::BodyReadTimeout { .. } => StatusCode::REQUEST_TIMEOUT,
            Self::InvalidContentLength | Self::ConflictingContentLength => {
                StatusCode::BAD_REQUEST
            }
        }
    }
}

pub(crate) fn validate_content_length(
    headers: &HeaderMap,
    limit: usize,
) -> Result<(), RequestPolicyError> {
    let mut parsed = None;
    for value in headers.get_all(CONTENT_LENGTH) {
        let value = value
            .to_str()
            .map_err(|_| RequestPolicyError::InvalidContentLength)?;
        for value in value.split(',') {
            let value = value.trim();
            if value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(RequestPolicyError::InvalidContentLength);
            }
            let value = value
                .parse::<u64>()
                .map_err(|_| RequestPolicyError::InvalidContentLength)?;
            if parsed.is_some_and(|previous| previous != value) {
                return Err(RequestPolicyError::ConflictingContentLength);
            }
            parsed = Some(value);
        }
    }
    if parsed.is_some_and(|length| length > limit as u64) {
        return Err(RequestPolicyError::BodyTooLarge { limit });
    }
    Ok(())
}

pub(super) fn policy_response(error: &RequestPolicyError) -> Response {
    let message = match error {
        RequestPolicyError::BodyTooLarge { .. } => "request body too large",
        RequestPolicyError::BodyReadTimeout { .. } => "request body timed out",
        RequestPolicyError::InvalidContentLength
        | RequestPolicyError::ConflictingContentLength => {
            "invalid Content-Length"
        }
    };
    plain_response(error.status(), message)
}

/// Builds one of the crate's own error responses.
///
/// The body is always a short ASCII sentence, so it is declared as
/// `text/plain; charset=utf-8`: a body with no declared type is exactly the
/// case where content sniffing is dangerous.
pub(super) fn plain_response(
    status: StatusCode,
    message: impl Into<Bytes>,
) -> Response {
    let mut response = http::Response::new(Body::Sync(message.into()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.into()
}

/// Applies `x-content-type-options: nosniff` unless the response already
/// carries a value for it.
///
/// Insert-if-absent mirrors
/// [`ExtendResponse::set_default_content_type`](crate::integration::ExtendResponse::set_default_content_type):
/// an application that deliberately set its own value through
/// [`ResponseOptions`](crate::response::ResponseOptions) keeps it, because `extend_response` has already merged
/// those headers in by the time this runs.
pub(super) fn set_default_nosniff(response: &mut Response) {
    let name = http::header::HeaderName::from_static(X_CONTENT_TYPE_OPTIONS);
    let headers = response.0.headers_mut();
    if !headers.contains_key(&name) {
        headers.insert(name, HeaderValue::from_static("nosniff"));
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, StatusCode, header::CONTENT_LENGTH};

    use super::*;
    use crate::response::Body;

    #[test]
    fn the_body_read_budget_is_off_by_default() {
        // The published contract puts request deadlines at the ingress. This
        // knob is defense in depth, so an upgrade must not start converting
        // anybody's slow uploads into errors.
        assert_eq!(HandlerConfig::default().request_body_timeout_ns(), None);
    }

    #[test]
    fn the_body_read_budget_round_trips() {
        let config =
            HandlerConfig::default().with_request_body_timeout_ns(30_000_000);
        assert_eq!(config.request_body_timeout_ns(), Some(30_000_000));
        // The builder must leave the size limit alone.
        assert_eq!(
            config.max_request_body_size(),
            DEFAULT_MAX_REQUEST_BODY_SIZE
        );
    }

    #[test]
    fn a_body_read_timeout_is_reported_as_request_timeout() {
        let error = RequestPolicyError::BodyReadTimeout {
            nanoseconds: 30_000_000,
        };
        assert_eq!(error.status(), StatusCode::REQUEST_TIMEOUT);
        // Display keeps the budget for operators; the HTTP body does not.
        assert!(error.to_string().contains("30000000"));
        assert_eq!(policy_body(&error), "request body timed out");
    }

    #[test]
    fn policy_response_bodies_do_not_echo_limits() {
        assert_eq!(
            policy_body(&RequestPolicyError::BodyTooLarge { limit: 16 }),
            "request body too large"
        );
        assert_eq!(
            policy_body(&RequestPolicyError::InvalidContentLength),
            "invalid Content-Length"
        );
        assert_eq!(
            policy_body(&RequestPolicyError::ConflictingContentLength),
            "invalid Content-Length"
        );
    }

    fn policy_body(error: &RequestPolicyError) -> String {
        match policy_response(error).0.into_body() {
            Body::Sync(bytes) => {
                String::from_utf8(bytes.to_vec()).expect("ascii policy body")
            }
            Body::Async(_) => String::new(),
        }
    }

    #[test]
    fn default_request_limit_is_sixteen_mib() {
        assert_eq!(
            HandlerConfig::default().max_request_body_size(),
            16 * 1024 * 1024
        );
    }

    #[test]
    fn conflicting_content_lengths_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_LENGTH, HeaderValue::from_static("1"));
        headers.append(CONTENT_LENGTH, HeaderValue::from_static("2"));
        assert!(matches!(
            validate_content_length(&headers, 1024),
            Err(RequestPolicyError::ConflictingContentLength)
        ));
    }

    #[test]
    fn exact_content_length_limit_is_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("1024"));

        assert!(validate_content_length(&headers, 1024).is_ok());
    }

    #[test]
    fn oversized_content_length_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("1025"));

        assert!(matches!(
            validate_content_length(&headers, 1024),
            Err(RequestPolicyError::BodyTooLarge { limit: 1024 })
        ));
    }

    #[test]
    fn non_digit_content_length_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("+1"));

        assert!(matches!(
            validate_content_length(&headers, 1024),
            Err(RequestPolicyError::InvalidContentLength)
        ));
    }
}
