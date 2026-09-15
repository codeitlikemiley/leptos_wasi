//! WASI Preview 3 request handler.

use http_body_util::{BodyExt, Limited};

#[cfg(feature = "tracing")]
use std::pin::Pin;
#[cfg(feature = "tracing")]
use std::time::Instant;

use bytes::Bytes;
#[cfg(feature = "tracing")]
use http::StatusCode;
use http::{Request, Uri};
use leptos::IntoView;
use server_fn::ServerFn;
use thiserror::Error;

use super::builder::common_handler_methods;
use super::core::HandlerCore;
use super::policy::{
    HandlerConfig, RegistrationError, RequestPolicyError, policy_response,
    validate_content_length,
};
use super::server_fns::{ReqBody, ResBody};
#[cfg(feature = "tracing")]
use super::trace::{
    TraceHandle, trace_finish, trace_first_byte, trace_policy_rejection,
};
use crate::{__private::ServerWithBody, response::Body};

/// Errors returned by the WASI Preview 3 handler.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HandlerError {
    /// WASI HTTP conversion failed.
    #[error("wasi http error: {0:?}")]
    Wasi(::wasip3::http::types::ErrorCode),
    /// A response stream emitted an error.
    #[error("response stream emitted an error")]
    ResponseStream(throw_error::Error),
}

impl HandlerError {
    /// Converts this failure into a Preview 3 WASI `ErrorCode`.
    ///
    /// [`HandlerError::Wasi`] is the code the adapter already recovered.
    /// [`HandlerError::ResponseStream`] has no WASI code and becomes
    /// `ErrorCode::InternalError(None)`.
    ///
    /// Registration and configuration errors are not [`HandlerError`]
    /// and must not use this method.
    #[must_use]
    pub fn into_error_code(self) -> ::wasip3::http::types::ErrorCode {
        match self {
            Self::Wasi(code) => code,
            Self::ResponseStream(_) => {
                ::wasip3::http::types::ErrorCode::InternalError(None)
            }
        }
    }
}

/// Result of Preview 3 body ingest after headers are known.
///
/// Policy stays [`Ok`]. Host body failure stays [`Err`].
#[derive(Debug)]
enum Ingested {
    Collected {
        parts: http::request::Parts,
        body: Bytes,
    },
    Rejected {
        parts: http::request::Parts,
        policy: RequestPolicyError,
    },
}

/// Leptos request handler for WASI Preview 3.
pub struct Handler {
    core: HandlerCore,
}

impl Handler {
    /// Builds a handler using [`HandlerConfig::default`].
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Wasi`] if the request body cannot be read.
    pub async fn build(
        request: Request<::wasip3::http_compat::IncomingRequestBody>,
    ) -> Result<Self, HandlerError> {
        Self::build_with_config(request, HandlerConfig::default()).await
    }

    /// Builds a handler with an explicit request policy.
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Wasi`] if the request body cannot be read.
    /// A body that breaches `config` is reported as a rejection response
    /// rather than an error.
    pub async fn build_with_config(
        request: Request<::wasip3::http_compat::IncomingRequestBody>,
        config: HandlerConfig,
    ) -> Result<Self, HandlerError> {
        #[cfg(feature = "tracing")]
        let request_started = Instant::now();
        let expiry = config
            .request_body_timeout_ns()
            .map(::wasip3::clocks::monotonic_clock::wait_for);
        let ingested = ingest(request, config, expiry).await?;
        Ok(Self::from_ingested(
            ingested,
            config,
            #[cfg(feature = "tracing")]
            request_started,
        ))
    }

    fn from_ingested(
        ingested: Ingested,
        config: HandlerConfig,
        #[cfg(feature = "tracing")] request_started: Instant,
    ) -> Self {
        let core = match ingested {
            Ingested::Collected { parts, body } => {
                HandlerCore::new(Request::from_parts(parts, body), config)
            }
            Ingested::Rejected { parts, policy } => {
                #[cfg(feature = "tracing")]
                trace_policy_rejection("p3", &policy);
                HandlerCore::new(
                    Request::from_parts(parts, Bytes::new()),
                    config,
                )
                .with_preset(policy_response(&policy), "request_policy")
            }
        };
        #[cfg(feature = "tracing")]
        let core = core.with_request_started(request_started);
        Self { core }
    }

    common_handler_methods!();

    /// Renders and converts the response to a WASI Preview 3 response.
    ///
    /// For SSR routes and registered server functions, `context` runs after
    /// standard request contexts such as [`http::request::Parts`] have been
    /// installed. This is the only handler hook for request-dependent
    /// application context; route-discovery context is request-independent.
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Wasi`] if the response cannot be converted
    /// to the host representation.
    pub async fn handle_with_context<IV>(
        self,
        app: impl Fn() -> IV + 'static + Send + Clone,
        context: impl Fn() + 'static + Clone + Send,
    ) -> Result<::wasip3::http::types::Response, HandlerError>
    where
        IV: IntoView + 'static,
    {
        let trace = self.core.request_trace("p3");
        let render = self.core.render(app, context);
        #[cfg(feature = "tracing")]
        let response = {
            use tracing::Instrument;
            render.instrument(trace.span.clone()).await
        };
        #[cfg(not(feature = "tracing"))]
        let response = render.await;
        let status = response.0.status();
        let response = response
            .0
            .map(|body| body.map_err(HandlerError::ResponseStream));
        #[cfg(feature = "tracing")]
        let response =
            response.map(|body| TraceBody::new(body, trace.clone(), status));
        #[cfg(not(feature = "tracing"))]
        let _ = (trace, status);
        ::wasip3::http_compat::http_into_wasi_response(response)
            .map_err(HandlerError::Wasi)
    }
}

/// Validates Content-Length, collects through [`Limited`], and optionally
/// races an injected expiry future.
///
/// `wait_for` is a WASI import, so the expiry is a future the caller owns.
/// `select` prefers the collect side when both are ready.
async fn ingest<B, Exp>(
    request: Request<B>,
    config: HandlerConfig,
    expiry: Option<Exp>,
) -> Result<Ingested, HandlerError>
where
    B: http_body::Body,
    B::Data: bytes::Buf,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
    Exp: std::future::Future<Output = ()>,
{
    let (parts, body) = request.into_parts();
    if let Err(policy) =
        validate_content_length(&parts.headers, config.max_request_body_size())
    {
        return Ok(Ingested::Rejected { parts, policy });
    }

    let body = Limited::new(body, config.max_request_body_size());
    let collected = match expiry {
        None => body.collect().await,
        Some(expiry) => {
            let collect = std::pin::pin!(body.collect());
            let expiry = std::pin::pin!(expiry);
            match futures::future::select(collect, expiry).await {
                futures::future::Either::Left((collected, _)) => collected,
                futures::future::Either::Right(((), _)) => {
                    let nanoseconds =
                        config.request_body_timeout_ns().unwrap_or(0);
                    return Ok(Ingested::Rejected {
                        parts,
                        policy: RequestPolicyError::BodyReadTimeout {
                            nanoseconds,
                        },
                    });
                }
            }
        }
    };

    match collected {
        Ok(body) => Ok(Ingested::Collected {
            parts,
            body: body.to_bytes(),
        }),
        Err(error) if error.is::<http_body_util::LengthLimitError>() => {
            Ok(Ingested::Rejected {
                parts,
                policy: RequestPolicyError::BodyTooLarge {
                    limit: config.max_request_body_size(),
                },
            })
        }
        Err(error) => {
            let code =
                error.downcast::<::wasip3::http::types::ErrorCode>().map_or(
                    ::wasip3::http::types::ErrorCode::InternalError(None),
                    |code| *code,
                );
            Err(HandlerError::Wasi(code))
        }
    }
}

#[cfg(feature = "tracing")]
struct TraceBody<B> {
    inner: Pin<Box<B>>,
    trace: TraceHandle,
    status: StatusCode,
    response_bytes: u64,
}

#[cfg(feature = "tracing")]
impl<B> TraceBody<B> {
    fn new(inner: B, trace: TraceHandle, status: StatusCode) -> Self {
        Self {
            inner: Box::pin(inner),
            trace,
            status,
            response_bytes: 0,
        }
    }
}

#[cfg(feature = "tracing")]
impl<B> http_body::Body for TraceBody<B>
where
    B: http_body::Body,
    B::Data: bytes::Buf,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<
        Option<Result<http_body::Frame<Self::Data>, Self::Error>>,
    > {
        let this = self.get_mut();
        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let count = bytes::Buf::remaining(data) as u64;
                    if count != 0 {
                        trace_first_byte(&this.trace);
                        this.response_bytes =
                            this.response_bytes.saturating_add(count);
                    }
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                trace_finish(
                    &this.trace,
                    this.status,
                    this.response_bytes,
                    false,
                    "response_stream",
                );
                std::task::Poll::Ready(Some(Err(error)))
            }
            std::task::Poll::Ready(None) => {
                trace_finish(
                    &this.trace,
                    this.status,
                    this.response_bytes,
                    false,
                    "none",
                );
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(feature = "tracing")]
impl<B> Drop for TraceBody<B> {
    fn drop(&mut self) {
        trace_finish(
            &self.trace,
            self.status,
            self.response_bytes,
            true,
            "body_dropped",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use futures::executor::block_on;
    use http::{Request, StatusCode, header::CONTENT_LENGTH};
    use http_body::{Body, Frame};
    use http_body_util::Full;

    use super::{Handler, HandlerError, Ingested, ingest};
    use crate::handler::core::Selection;
    use crate::handler::policy::{HandlerConfig, RequestPolicyError};

    struct PendingBody;

    impl Body for PendingBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }
    }

    struct FailBody(Option<::wasip3::http::types::ErrorCode>);

    impl Body for FailBody {
        type Data = Bytes;
        type Error = ::wasip3::http::types::ErrorCode;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let this = self.get_mut();
            Poll::Ready(Some(Err(this.0.take().unwrap_or(
                ::wasip3::http::types::ErrorCode::InternalError(None),
            ))))
        }
    }

    #[test]
    fn content_length_over_the_limit_is_rejected_before_collect() {
        let request = Request::builder()
            .header(CONTENT_LENGTH, "9")
            .body(Full::new(Bytes::new()))
            .expect("request");
        let config = HandlerConfig::default().with_max_request_body_size(8);

        let ingested =
            block_on(ingest(request, config, None::<std::future::Ready<()>>))
                .expect("policy is Ok");

        assert!(matches!(
            ingested,
            Ingested::Rejected {
                policy: RequestPolicyError::BodyTooLarge { limit: 8 },
                ..
            }
        ));
    }

    #[test]
    fn conflicting_content_length_is_rejected() {
        let request = Request::builder()
            .header(CONTENT_LENGTH, "1")
            .header(CONTENT_LENGTH, "2")
            .body(Full::new(Bytes::new()))
            .expect("request");

        let ingested = block_on(ingest(
            request,
            HandlerConfig::default(),
            None::<std::future::Ready<()>>,
        ))
        .expect("policy is Ok");

        assert!(matches!(
            ingested,
            Ingested::Rejected {
                policy: RequestPolicyError::ConflictingContentLength,
                ..
            }
        ));
    }

    #[test]
    fn limited_overflow_without_content_length_is_rejected() {
        let request = Request::new(Full::new(Bytes::from_static(b"123456789")));
        let config = HandlerConfig::default().with_max_request_body_size(8);

        let ingested =
            block_on(ingest(request, config, None::<std::future::Ready<()>>))
                .expect("policy is Ok");

        assert!(matches!(
            ingested,
            Ingested::Rejected {
                policy: RequestPolicyError::BodyTooLarge { limit: 8 },
                ..
            }
        ));
    }

    #[test]
    fn timeout_wins_when_the_body_is_still_pending() {
        let request = Request::new(PendingBody);
        let config = HandlerConfig::default().with_request_body_timeout_ns(1);

        let ingested =
            block_on(ingest(request, config, Some(std::future::ready(()))))
                .expect("policy is Ok");

        assert!(matches!(
            ingested,
            Ingested::Rejected {
                policy: RequestPolicyError::BodyReadTimeout { nanoseconds: 1 },
                ..
            }
        ));
    }

    #[test]
    fn collect_wins_when_the_body_and_expiry_are_both_ready() {
        let request = Request::new(Full::new(Bytes::from_static(b"ok")));
        let config = HandlerConfig::default().with_request_body_timeout_ns(1);

        let ingested =
            block_on(ingest(request, config, Some(std::future::ready(()))))
                .expect("body ready");

        assert!(matches!(
            ingested,
            Ingested::Collected { ref body, .. } if body.as_ref() == b"ok"
        ));
    }

    #[test]
    fn collect_wins_when_the_expiry_is_still_pending() {
        let request = Request::new(Full::new(Bytes::from_static(b"ok")));
        let config = HandlerConfig::default().with_request_body_timeout_ns(1);

        let ingested =
            block_on(ingest(request, config, Some(std::future::pending())))
                .expect("body ready");

        assert!(matches!(
            ingested,
            Ingested::Collected { ref body, .. } if body.as_ref() == b"ok"
        ));
    }

    #[test]
    fn a_downcastable_collect_error_keeps_its_wasi_code() {
        let request = Request::new(FailBody(Some(
            ::wasip3::http::types::ErrorCode::HttpProtocolError,
        )));
        let err = block_on(ingest(
            request,
            HandlerConfig::default(),
            None::<std::future::Ready<()>>,
        ))
        .expect_err("transport is Err");

        assert!(matches!(
            err,
            HandlerError::Wasi(
                ::wasip3::http::types::ErrorCode::HttpProtocolError
            )
        ));
        assert!(matches!(
            HandlerError::Wasi(
                ::wasip3::http::types::ErrorCode::HttpProtocolError
            )
            .into_error_code(),
            ::wasip3::http::types::ErrorCode::HttpProtocolError
        ));
    }

    #[test]
    fn from_ingested_presets_a_policy_rejection() {
        let (parts, _) = Request::new(Bytes::new()).into_parts();
        let handler = Handler::from_ingested(
            Ingested::Rejected {
                parts,
                policy: RequestPolicyError::BodyTooLarge { limit: 8 },
            },
            HandlerConfig::default(),
            #[cfg(feature = "tracing")]
            std::time::Instant::now(),
        );

        let (status, class) = match &handler.core.selection {
            Selection::Preset(response, class) => (response.0.status(), *class),
            Selection::Unclaimed
            | Selection::NotFound
            | Selection::ServerFn(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "not-preset")
            }
        };
        assert_eq!(class, "request_policy");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn into_error_code_preserves_a_recovered_wasi_code() {
        let error = HandlerError::Wasi(
            ::wasip3::http::types::ErrorCode::ConnectionReadTimeout,
        );
        assert!(matches!(
            error.into_error_code(),
            ::wasip3::http::types::ErrorCode::ConnectionReadTimeout
        ));
    }
}
