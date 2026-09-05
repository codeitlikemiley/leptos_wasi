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
        let (parts, body) = request.into_parts();
        if let Err(error) = validate_content_length(
            &parts.headers,
            config.max_request_body_size(),
        ) {
            #[cfg(feature = "tracing")]
            trace_policy_rejection("p3", &error);
            let core = HandlerCore::new(
                Request::from_parts(parts, Bytes::new()),
                config,
            )
            .with_preset(policy_response(&error), "request_policy");
            #[cfg(feature = "tracing")]
            let core = core.with_request_started(request_started);
            return Ok(Self { core });
        }

        let body = Limited::new(body, config.max_request_body_size());
        // Same total-budget meaning as Preview 2: one timer for the whole
        // body, raced against the collect. `Limited::collect` is a single
        // opaque future with no per-frame hook, so a total budget is also
        // the only shape both previews can agree on.
        let collected = match config.request_body_timeout_ns() {
            None => body.collect().await,
            Some(nanoseconds) => {
                let collect = std::pin::pin!(body.collect());
                let expiry = std::pin::pin!(
                    ::wasip3::clocks::monotonic_clock::wait_for(nanoseconds)
                );
                match futures::future::select(collect, expiry).await {
                    futures::future::Either::Left((collected, _)) => collected,
                    futures::future::Either::Right(((), _)) => {
                        let policy =
                            RequestPolicyError::BodyReadTimeout { nanoseconds };
                        #[cfg(feature = "tracing")]
                        trace_policy_rejection("p3", &policy);
                        let core = HandlerCore::new(
                            Request::from_parts(parts, Bytes::new()),
                            config,
                        )
                        .with_preset(
                            policy_response(&policy),
                            "request_policy",
                        );
                        #[cfg(feature = "tracing")]
                        let core = core.with_request_started(request_started);
                        return Ok(Self { core });
                    }
                }
            }
        };
        match collected {
            Ok(body) => {
                let core = HandlerCore::new(
                    Request::from_parts(parts, body.to_bytes()),
                    config,
                );
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core })
            }
            Err(error) if error.is::<http_body_util::LengthLimitError>() => {
                let policy = RequestPolicyError::BodyTooLarge {
                    limit: config.max_request_body_size(),
                };
                #[cfg(feature = "tracing")]
                trace_policy_rejection("p3", &policy);
                let core = HandlerCore::new(
                    Request::from_parts(parts, Bytes::new()),
                    config,
                )
                .with_preset(policy_response(&policy), "request_policy");
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core })
            }
            Err(error) => {
                let code = error
                    .downcast::<::wasip3::http::types::ErrorCode>()
                    .map_or(
                        ::wasip3::http::types::ErrorCode::InternalError(None),
                        |code| *code,
                    );
                Err(HandlerError::Wasi(code))
            }
        }
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
    #[test]
    fn select_prefers_the_collect_side_when_both_are_ready() {
        // `build_with_config` races `Limited::collect` against
        // `wait_for`. When both are ready, `futures::future::select`
        // takes the left (collect) future, so a completed body is not
        // reported as a timeout.
        let collect = std::pin::pin!(std::future::ready("collect"));
        let expiry = std::pin::pin!(std::future::ready("timeout"));
        let chosen = futures::executor::block_on(futures::future::select(
            collect, expiry,
        ));
        assert!(matches!(chosen, futures::future::Either::Left(_)));
    }
}
