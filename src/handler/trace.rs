//! Per-request tracing spans and the counters they carry.
//!
//! Everything here is `#[cfg(feature = "tracing")]` except the stand-in
//! [`TraceHandle`] and the two no-op functions that keep the Preview 2
//! send path compiling when the feature is off.

#[cfg(feature = "tracing")]
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

// Both `trace_finish` arms take a status: the real one under `tracing`, the
// stand-in under `all(not(tracing), wasip2)`. That union is `any(tracing,
// wasip2)` - a wasip3-only build with tracing off compiles neither.
#[cfg(any(feature = "tracing", feature = "wasip2"))]
use http::StatusCode;
#[cfg(feature = "tracing")]
use leptos_router::SsrMode;

#[cfg(feature = "tracing")]
use super::core::{HandlerCore, Selection};
#[cfg(feature = "tracing")]
use super::policy::RequestPolicyError;

#[cfg(feature = "tracing")]
#[derive(Clone)]
pub(super) struct RequestTrace {
    pub(super) span: tracing::Span,
    state: Arc<RequestTraceState>,
}

#[cfg(feature = "tracing")]
struct RequestTraceState {
    started: Instant,
    first_byte_micros: AtomicU64,
    finished: AtomicBool,
}

#[cfg(feature = "tracing")]
impl RequestTrace {
    pub(super) fn new(core: &HandlerCore, preview: &'static str) -> Self {
        let path = core
            .trace_path
            .as_deref()
            .unwrap_or_else(|| core.req.uri().path());
        let best_match = core.ssr_router.best_match(path);
        let route_class = match &core.selection {
            Selection::ServerFn(_) => "server_fn",
            Selection::Preset(_, class) => *class,
            Selection::NotFound => "not_found",
            Selection::Unclaimed => {
                if best_match.is_none() {
                    "not_found"
                } else {
                    "ssr"
                }
            }
        };
        let ssr_mode = if route_class == "ssr" {
            best_match.map_or("none", |matched| {
                ssr_mode_name(matched.handler().mode())
            })
        } else {
            "none"
        };
        let request_id = core
            .req
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| {
                value.len() <= 128
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
            })
            .unwrap_or_default();
        let span = tracing::info_span!(
            "leptos_wasi.request",
            runtime = "wasi",
            preview,
            method = %core.req.method(),
            path,
            route_class,
            ssr_mode,
            request_id,
            request_bytes = core.req.body().len(),
        );
        Self {
            span,
            state: Arc::new(RequestTraceState {
                started: core.request_started,
                first_byte_micros: AtomicU64::new(0),
                finished: AtomicBool::new(false),
            }),
        }
    }

    fn mark_first_byte(&self) {
        if self.state.first_byte_micros.load(Ordering::Relaxed) != 0 {
            return;
        }
        let elapsed = self.state.started.elapsed().as_micros();
        let encoded = u64::try_from(elapsed)
            .unwrap_or(u64::MAX - 1)
            .saturating_add(1);
        let _ = self.state.first_byte_micros.compare_exchange(
            0,
            encoded,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    fn finish(
        &self,
        status: StatusCode,
        response_bytes: u64,
        cancellation: bool,
        error_class: &'static str,
    ) {
        if self.state.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let first_byte = self.state.first_byte_micros.load(Ordering::Relaxed);
        // A metric, not a measurement anyone computes with. u64 microseconds
        // only lose f64 precision past 2^53, i.e. roughly 285 years.
        #[expect(
            clippy::cast_precision_loss,
            reason = "reported as a metric; loss starts past 285 years"
        )]
        let first_byte_ms = first_byte
            .checked_sub(1)
            .map(|micros| micros as f64 / 1_000.0);
        tracing::info!(
            parent: &self.span,
            status = status.as_u16(),
            response_bytes,
            duration_ms = self.state.started.elapsed().as_secs_f64() * 1_000.0,
            first_byte_ms,
            cancellation,
            error_class,
            "request completed"
        );
    }
}

#[cfg(feature = "tracing")]
pub(super) type TraceHandle = RequestTrace;
#[cfg(not(feature = "tracing"))]
#[derive(Clone, Copy)]
pub(super) struct TraceHandle;

#[cfg(feature = "tracing")]
pub(super) fn trace_first_byte(trace: &TraceHandle) {
    trace.mark_first_byte();
}

// Both stand-ins mirror the signatures of the `tracing` arms above, which
// take `&RequestTrace` by reference because it is not `Copy`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature must match the tracing arm"
)]
#[cfg(all(not(feature = "tracing"), feature = "wasip2"))]
pub(super) fn trace_first_byte(_: &TraceHandle) {}

#[cfg(feature = "tracing")]
pub(super) fn trace_finish(
    trace: &TraceHandle,
    status: StatusCode,
    response_bytes: u64,
    cancellation: bool,
    error_class: &'static str,
) {
    trace.finish(status, response_bytes, cancellation, error_class);
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature must match the tracing arm"
)]
#[cfg(all(not(feature = "tracing"), feature = "wasip2"))]
pub(super) fn trace_finish(
    _: &TraceHandle,
    _: StatusCode,
    _: u64,
    _: bool,
    _: &'static str,
) {
}

#[cfg(feature = "tracing")]
fn ssr_mode_name(mode: &SsrMode) -> &'static str {
    match mode {
        SsrMode::Async => "async",
        SsrMode::InOrder => "in_order",
        SsrMode::PartiallyBlocked => "partially_blocked",
        SsrMode::OutOfOrder => "out_of_order",
        SsrMode::Static(_) => "static",
    }
}

#[cfg(feature = "tracing")]
pub(super) fn trace_policy_rejection(
    preview: &'static str,
    error: &RequestPolicyError,
) {
    let error_class = match error {
        RequestPolicyError::BodyTooLarge { .. } => "body_too_large",
        RequestPolicyError::BodyReadTimeout { .. } => "body_read_timeout",
        RequestPolicyError::InvalidContentLength => "invalid_content_length",
        RequestPolicyError::ConflictingContentLength => {
            "conflicting_content_length"
        }
    };
    tracing::warn!(
        runtime = "wasi",
        preview,
        status = error.status().as_u16(),
        error_class,
        "request policy rejected incoming body"
    );
}

#[cfg(all(test, feature = "tracing"))]
mod tests {
    use bytes::Bytes;
    use http::{Method, Request};

    use super::super::core::{HandlerCore, Selection};
    use super::super::policy::HandlerConfig;
    use crate::response::Body;

    #[cfg(feature = "tracing")]
    #[test]
    fn static_trace_fields_use_the_normalized_callback_path() {
        let core = HandlerCore::new(
            Request::builder()
                .method(Method::GET)
                .uri("/static/nested%20asset.js")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        )
        .static_files_handler("/static", |_| {
            Some(Body::Sync(Bytes::from_static(b"asset")))
        })
        .expect("static registration should succeed");

        assert!(matches!(core.selection, Selection::Preset(_, "static")));
        assert_eq!(core.trace_path.as_deref(), Some("/static/nested asset.js"));
    }
}
