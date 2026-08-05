//! WASI Preview 2 request handler.

use futures::StreamExt;
use wasi::{
    http::types::{
        IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
    },
    io::streams::{OutputStream, StreamError},
};

#[cfg(feature = "tracing")]
use std::time::Instant;

use bytes::Bytes;
use http::{Request, StatusCode, Uri, header::CONTENT_TYPE};
use leptos::IntoView;
use server_fn::ServerFn;
use thiserror::Error;

use super::builder::common_handler_methods;
use super::core::HandlerCore;
use super::policy::{
    HandlerConfig, RegistrationError, RequestPolicyError,
    X_CONTENT_TYPE_OPTIONS, policy_response,
};
use super::server_fns::{ReqBody, ResBody};
#[cfg(feature = "tracing")]
use super::trace::trace_policy_rejection;
use super::trace::{TraceHandle, trace_finish, trace_first_byte};
use crate::{
    __private::ServerWithBody,
    response::{Body, Response},
};

struct ResponseOutGuard(Option<ResponseOutparam>);

impl ResponseOutGuard {
    fn new(response_out: ResponseOutparam) -> Self {
        Self(Some(response_out))
    }

    fn take(&mut self) -> Option<ResponseOutparam> {
        self.0.take()
    }
}

impl Drop for ResponseOutGuard {
    fn drop(&mut self) {
        if let Some(response_out) = self.0.take() {
            send_internal_error(response_out);
        }
    }
}

/// Errors returned by the WASI Preview 2 handler.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HandlerError {
    /// Incoming request conversion failed.
    #[error("error handling request")]
    Request(#[from] crate::request::p2::RequestError),
    /// Response header conversion failed.
    #[error("error handling response")]
    Response(#[from] crate::response::ResponseError),
    /// A response stream emitted an error.
    #[error("response stream emitted an error")]
    ResponseStream(throw_error::Error),
    /// A WASI stream operation failed.
    #[error("wasi stream failure")]
    WasiStream(#[from] StreamError),
    /// A WASI response body operation failed.
    #[error("failed to finish response body: {0:?}")]
    WasiResponseBody(wasi::http::types::ErrorCode),
    /// The host rejected response construction before commitment.
    #[error("host rejected outgoing response operation: {0}")]
    OutgoingResponse(&'static str),
    /// The cooperative executor could not wait for output capacity.
    #[error("executor error while writing the response")]
    Executor(#[from] crate::executor::ExecutorError),
}

/// Leptos request handler for WASI Preview 2.
pub struct Handler {
    core: HandlerCore,
    response_out: ResponseOutGuard,
}

impl Handler {
    /// Builds a handler using [`HandlerConfig::default`].
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Request`] if the incoming request cannot be
    /// converted, which includes a body that breaches the configured size
    /// or time budget.
    pub fn build(
        request: IncomingRequest,
        response_out: ResponseOutparam,
    ) -> Result<Self, HandlerError> {
        Self::build_with_config(request, response_out, HandlerConfig::default())
    }

    /// Builds a handler with an explicit request policy.
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Request`] if the incoming request cannot be
    /// converted, which includes a body that breaches `config`.
    pub fn build_with_config(
        request: IncomingRequest,
        response_out: ResponseOutparam,
        config: HandlerConfig,
    ) -> Result<Self, HandlerError> {
        let response_out = ResponseOutGuard::new(response_out);
        #[cfg(feature = "tracing")]
        let request_started = Instant::now();
        let rejected_request = Request::from_parts(
            crate::request::p2::request_parts(&request)?,
            Bytes::new(),
        );
        match crate::request::p2::from_wasi_request_with_deadline(
            request,
            config.max_request_body_size(),
            config.request_body_timeout_ns(),
        ) {
            Ok(request) => {
                let core = HandlerCore::new(request, config);
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core, response_out })
            }
            Err(crate::request::p2::RequestError::BodyTooLarge(_)) => {
                let policy = RequestPolicyError::BodyTooLarge {
                    limit: config.max_request_body_size(),
                };
                #[cfg(feature = "tracing")]
                trace_policy_rejection("p2", &policy);
                let response = policy_response(&policy);
                let core = HandlerCore::new(rejected_request, config)
                    .with_preset(response, "request_policy");
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core, response_out })
            }
            Err(crate::request::p2::RequestError::BodyReadTimeout(
                nanoseconds,
            )) => {
                let policy =
                    RequestPolicyError::BodyReadTimeout { nanoseconds };
                #[cfg(feature = "tracing")]
                trace_policy_rejection("p2", &policy);
                let response = policy_response(&policy);
                let core = HandlerCore::new(rejected_request, config)
                    .with_preset(response, "request_policy");
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core, response_out })
            }
            Err(crate::request::p2::RequestError::Policy(error)) => {
                #[cfg(feature = "tracing")]
                trace_policy_rejection("p2", &error);
                let response = policy_response(&error);
                let core = HandlerCore::new(rejected_request, config)
                    .with_preset(response, "request_policy");
                #[cfg(feature = "tracing")]
                let core = core.with_request_started(request_started);
                Ok(Self { core, response_out })
            }
            Err(error) => Err(error.into()),
        }
    }

    common_handler_methods!();

    /// Renders and transmits the response through the WASI out-parameter.
    ///
    /// For SSR routes and registered server functions, `context` runs after
    /// standard request contexts such as [`http::request::Parts`] have been
    /// installed. This is the only handler hook for request-dependent
    /// application context; route-discovery context is request-independent.
    ///
    /// # Errors
    ///
    /// Returns [`HandlerError::Response`] if response headers cannot be
    /// converted, [`HandlerError::ResponseStream`] if the body stream fails,
    /// [`HandlerError::WasiStream`] or [`HandlerError::WasiResponseBody`] if
    /// a host stream or body operation fails,
    /// [`HandlerError::OutgoingResponse`] if the host rejects response
    /// construction, and [`HandlerError::Executor`] if the Preview 2 executor
    /// cannot make progress while the body is written.
    pub async fn handle_with_context<IV>(
        self,
        app: impl Fn() -> IV + 'static + Send + Clone,
        context: impl Fn() + 'static + Clone + Send,
    ) -> Result<(), HandlerError>
    where
        IV: IntoView + 'static,
    {
        let Self {
            core,
            mut response_out,
        } = self;
        let trace = core.request_trace("p2");
        let render = core.render(app, context);
        #[cfg(feature = "tracing")]
        let response = {
            use tracing::Instrument;
            render.instrument(trace.span.clone()).await
        };
        #[cfg(not(feature = "tracing"))]
        let response = render.await;
        let Some(response_out) = response_out.take() else {
            return Err(HandlerError::OutgoingResponse(
                "response out-parameter",
            ));
        };
        send_response(response, response_out, trace).await
    }
}

async fn send_response(
    response: Response,
    response_out: ResponseOutparam,
    trace: TraceHandle,
) -> Result<(), HandlerError> {
    let status = response.0.status();
    let prepared = (|| {
        let headers = response.headers()?;
        let outgoing = OutgoingResponse::new(headers);
        outgoing
            .set_status_code(response.0.status().as_u16())
            .map_err(|()| HandlerError::OutgoingResponse("status"))?;
        let body = outgoing
            .body()
            .map_err(|()| HandlerError::OutgoingResponse("body"))?;
        let output = body
            .write()
            .map_err(|()| HandlerError::OutgoingResponse("body writer"))?;
        Ok::<_, HandlerError>((outgoing, body, output))
    })();
    let (outgoing, body, output) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            send_internal_error(response_out);
            trace_finish(
                &trace,
                StatusCode::INTERNAL_SERVER_ERROR,
                0,
                false,
                "response_setup",
            );
            return Err(error);
        }
    };
    ResponseOutparam::set(response_out, Ok(outgoing));

    let mut response_bytes = 0_u64;
    let mut input = match response.0.into_body() {
        Body::Sync(bytes) => {
            Box::pin(futures::stream::once(async { Ok(bytes) }))
        }
        Body::Async(stream) => stream,
    };
    let transfer = async {
        while let Some(bytes) = input.next().await {
            let bytes = bytes.map_err(HandlerError::ResponseStream)?;
            write_all(&output, &bytes, &trace, &mut response_bytes).await?;
        }
        output.flush()?;
        crate::executor::WaitPoll::new(output.subscribe()).await?;
        Ok::<_, HandlerError>(())
    }
    .await;

    // The response has already been committed. Always close the output
    // stream and finish the outgoing body, even when a producer or host
    // write fails, so the next component request cannot inherit a live
    // response resource.
    drop(output);
    let finish = OutgoingBody::finish(body, None)
        .map_err(HandlerError::WasiResponseBody);
    match transfer {
        Err(error) => {
            let _ = finish;
            let cancellation = matches!(
                &error,
                HandlerError::WasiStream(StreamError::Closed)
                    | HandlerError::Executor(
                        crate::executor::ExecutorError::PollableCanceled
                            | crate::executor::ExecutorError::RunUntilCanceled
                    )
            );
            let error_class = match &error {
                HandlerError::ResponseStream(_) => "response_stream",
                HandlerError::WasiStream(_) => "wasi_stream",
                HandlerError::Executor(_) => "executor",
                _ => "response_transfer",
            };
            trace_finish(
                &trace,
                status,
                response_bytes,
                cancellation,
                error_class,
            );
            Err(error)
        }
        Ok(()) => match finish {
            Ok(()) => {
                trace_finish(&trace, status, response_bytes, false, "none");
                Ok(())
            }
            Err(error) => {
                trace_finish(
                    &trace,
                    status,
                    response_bytes,
                    false,
                    "response_finish",
                );
                Err(error)
            }
        },
    }
}

fn send_internal_error(response_out: ResponseOutparam) {
    let headers = wasi::http::types::Headers::new();
    // This response is built outside `HandlerCore::render`, so the
    // centralized default cannot reach it. Its body is a typeless ASCII
    // sentence, which is exactly the case content sniffing exploits.
    let _ = headers.append(
        CONTENT_TYPE.as_str(),
        b"text/plain; charset=utf-8".to_vec().as_ref(),
    );
    let _ =
        headers.append(X_CONTENT_TYPE_OPTIONS, b"nosniff".to_vec().as_ref());
    let response = OutgoingResponse::new(headers);
    if response
        .set_status_code(StatusCode::INTERNAL_SERVER_ERROR.as_u16())
        .is_err()
    {
        return;
    }
    let Ok(body) = response.body() else {
        return;
    };
    let Ok(output) = body.write() else {
        return;
    };
    ResponseOutparam::set(response_out, Ok(response));
    let _ = output.blocking_write_and_flush(b"internal server error");
    drop(output);
    let _ = OutgoingBody::finish(body, None);
}

async fn write_all(
    output: &OutputStream,
    mut bytes: &[u8],
    trace: &TraceHandle,
    response_bytes: &mut u64,
) -> Result<(), HandlerError> {
    while !bytes.is_empty() {
        let capacity = output.check_write()?;
        if capacity == 0 {
            crate::executor::WaitPoll::new(output.subscribe()).await?;
            continue;
        }
        let count = usize::try_from(capacity)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        output.write(&bytes[..count])?;
        *response_bytes = (*response_bytes).saturating_add(count as u64);
        if count != 0 {
            trace_first_byte(trace);
        }
        bytes = &bytes[count..];
    }
    Ok(())
}
