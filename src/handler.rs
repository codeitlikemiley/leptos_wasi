#![forbid(unsafe_code)]

//! Shared Leptos request handling and runtime-specific WASI HTTP adapters.

use std::pin::Pin;
#[cfg(feature = "tracing")]
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

use bytes::Bytes;
use futures::{StreamExt, stream::once};
use http::{
    HeaderValue, Method, Request, StatusCode, Uri,
    header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE, REFERER},
};
use leptos::{
    IntoView,
    hydration::IslandsRouterNavigation,
    prelude::{Owner, ScopedFuture, provide_context},
};
use leptos_meta::ServerMetaContext;
use leptos_router::{RouteListing, SsrMode};
use mime_guess::MimeGuess;
use routefinder::Router;
use server_fn::{
    Protocol, ServerFn, error::FromServerFnError, middleware::BoxedService,
};
use thiserror::Error;

use crate::{
    __private::ServerWithBody,
    integration::{ExtendResponse, PinnedStream},
    response::{Body, Response, ResponseOptions},
};

mod builder;
mod http_util;
mod policy;
mod routes;
mod server_fns;
#[cfg(test)]
mod test_support;

use builder::common_handler_methods;
use http_util::{
    accepts_html, is_islands_router_navigation, provide_standard_contexts,
};
use policy::{plain_response, policy_response, set_default_nosniff};
use routes::validated_route_table;
use server_fns::{
    ReqBody, ResBody, ServerFnHandler, TypedServerFnService,
    apply_server_fn_redirect,
};

pub use routes::validate_route_table;

pub(crate) use policy::validate_content_length;
pub use policy::{
    DEFAULT_MAX_REQUEST_BODY_SIZE, HandlerConfig, RegistrationError,
    RequestPolicyError,
};

#[cfg(feature = "tracing")]
#[derive(Clone)]
struct RequestTrace {
    span: tracing::Span,
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
    fn new(core: &HandlerCore, preview: &'static str) -> Self {
        let path = core
            .trace_path
            .as_deref()
            .unwrap_or_else(|| core.req.uri().path());
        let best_match = core.ssr_router.best_match(path);
        let route_class = core.trace_route_class.unwrap_or_else(|| {
            if core.server_fn.is_some() {
                "server_fn"
            } else if core.preset_res.is_some() {
                "preset"
            } else if core.should_404 || best_match.is_none() {
                "not_found"
            } else {
                "ssr"
            }
        });
        let ssr_mode = if route_class == "ssr" {
            best_match
                .map(|matched| ssr_mode_name(matched.handler().mode()))
                .unwrap_or("none")
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
type TraceHandle = RequestTrace;
#[cfg(not(feature = "tracing"))]
#[derive(Clone, Copy)]
struct TraceHandle;

#[cfg(feature = "tracing")]
fn trace_first_byte(trace: &TraceHandle) {
    trace.mark_first_byte();
}

#[cfg(all(not(feature = "tracing"), feature = "wasip2"))]
fn trace_first_byte(_: &TraceHandle) {}

#[cfg(feature = "tracing")]
fn trace_finish(
    trace: &TraceHandle,
    status: StatusCode,
    response_bytes: u64,
    cancellation: bool,
    error_class: &'static str,
) {
    trace.finish(status, response_bytes, cancellation, error_class);
}

#[cfg(all(not(feature = "tracing"), feature = "wasip2"))]
fn trace_finish(
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
fn trace_policy_rejection(preview: &'static str, error: &RequestPolicyError) {
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

struct HandlerCore {
    req: Request<Bytes>,
    server_fn: Option<ServerFnHandler>,
    preset_res: Option<Response>,
    should_404: bool,
    ssr_router: Router<RouteListing>,
    routes_registered: bool,
    config: HandlerConfig,
    #[cfg(feature = "tracing")]
    request_started: Instant,
    #[cfg(feature = "tracing")]
    trace_path: Option<String>,
    #[cfg(feature = "tracing")]
    trace_route_class: Option<&'static str>,
}

impl HandlerCore {
    fn new(req: Request<Bytes>, config: HandlerConfig) -> Self {
        Self {
            req,
            server_fn: None,
            preset_res: None,
            should_404: false,
            ssr_router: Router::new(),
            routes_registered: false,
            config,
            #[cfg(feature = "tracing")]
            request_started: Instant::now(),
            #[cfg(feature = "tracing")]
            trace_path: None,
            #[cfg(feature = "tracing")]
            trace_route_class: None,
        }
    }

    #[cfg(feature = "tracing")]
    fn with_request_started(mut self, started: Instant) -> Self {
        self.request_started = started;
        self
    }

    fn with_preset(
        mut self,
        response: Response,
        route_class: &'static str,
    ) -> Self {
        self.preset_res = Some(response);
        #[cfg(feature = "tracing")]
        {
            self.trace_route_class = Some(route_class);
        }
        #[cfg(not(feature = "tracing"))]
        let _ = route_class;
        self
    }

    #[cfg(feature = "tracing")]
    fn request_trace(&self, preview: &'static str) -> TraceHandle {
        RequestTrace::new(self, preview)
    }

    #[cfg(not(feature = "tracing"))]
    fn request_trace(&self, _: &'static str) -> TraceHandle {
        TraceHandle
    }

    #[inline]
    fn shortcut(&self) -> bool {
        self.server_fn.is_some() || self.preset_res.is_some() || self.should_404
    }

    fn with_server_fn<T>(mut self) -> Self
    where
        T: ServerFn + 'static,
        T::Server:
            ServerWithBody<T::Error, T::InputStreamError, T::OutputStreamError>,
        ReqBody<T>: From<Bytes> + Send + 'static,
        ResBody<T>: Into<Body> + Send + 'static,
    {
        if self.shortcut() {
            return self;
        }

        let method = <T::Protocol as Protocol<
            T,
            T::Output,
            T::Client,
            T::Server,
            T::Error,
            T::InputStreamError,
            T::OutputStreamError,
        >>::METHOD;

        if self.req.method() == method && self.req.uri().path() == T::PATH {
            let limit = self.config.max_request_body_size();
            self.server_fn = Some(Box::new(move |request| {
                Box::pin(async move {
                    let (parts, bytes) = request.into_parts();
                    if bytes.len() > limit {
                        return plain_response(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!(
                                "request body exceeds limit of {limit} bytes"
                            ),
                        )
                        .0;
                    }

                    let request =
                        Request::from_parts(parts, ReqBody::<T>::from(bytes));
                    let mut service = BoxedService::new(
                        |error| T::Error::from_server_fn_error(error).ser(),
                        TypedServerFnService::<T>::default(),
                    );
                    for middleware in T::middlewares() {
                        service = middleware.layer(service);
                    }
                    service.run(request).await.map(Into::into)
                })
            }));
        }

        self
    }

    fn static_files_handler<T>(
        mut self,
        prefix: T,
        handler: impl Fn(String) -> Option<Body> + 'static + Send + Clone,
    ) -> Result<Self, RegistrationError>
    where
        T: TryInto<Uri>,
        <T as TryInto<Uri>>::Error: std::error::Error,
    {
        let prefix_uri = prefix.try_into().map_err(|error| {
            RegistrationError::InvalidStaticPrefix(error.to_string())
        })?;
        let prefix_path = prefix_uri.path();
        if !prefix_path.starts_with('/')
            || prefix_uri.scheme().is_some()
            || prefix_uri.authority().is_some()
            || prefix_uri.query().is_some()
        {
            return Err(RegistrationError::InvalidStaticPrefix(
                prefix_uri.to_string(),
            ));
        }

        // Registration errors are configuration errors, so validate the
        // prefix even when an earlier handler already selected this request.
        if self.shortcut() {
            return Ok(self);
        }

        let req_path = self.req.uri().path();
        let matches = req_path == prefix_path
            || req_path.strip_prefix(prefix_path).is_some_and(|rest| {
                rest.starts_with('/') || prefix_path.ends_with('/')
            });
        if !matches {
            return Ok(self);
        }

        #[cfg(feature = "tracing")]
        {
            self.trace_route_class = Some("static");
            self.trace_path = Some(prefix_path.to_owned());
        }

        if !matches!(self.req.method(), &Method::GET | &Method::HEAD) {
            let mut response = plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed",
            );
            response
                .0
                .headers_mut()
                .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
            self.preset_res = Some(response);
            return Ok(self);
        }

        let stripped = req_path.strip_prefix(prefix_path).unwrap_or_default();
        let raw = if prefix_path.ends_with('/') {
            stripped
        } else {
            stripped.strip_prefix('/').unwrap_or(stripped)
        };
        let decoded = match crate::static_files::normalize_static_path(raw) {
            Ok(path) => path,
            Err(_) => {
                self.should_404 = true;
                return Ok(self);
            }
        };

        #[cfg(feature = "tracing")]
        {
            self.trace_path = Some(if decoded.is_empty() {
                prefix_path.to_owned()
            } else if prefix_path.ends_with('/') {
                format!("{prefix_path}{decoded}")
            } else {
                format!("{prefix_path}/{decoded}")
            });
        }

        match handler(decoded.clone()) {
            None => self.should_404 = true,
            Some(mut body) => {
                let original_length = match &body {
                    Body::Sync(bytes) => Some(bytes.len()),
                    Body::Async(_) => None,
                };
                if self.req.method() == Method::HEAD {
                    body = Body::Sync(Bytes::new());
                }
                let mut response = http::Response::new(body);
                let mime = MimeGuess::from_path(&decoded)
                    .first_or_octet_stream()
                    .to_string();
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_str(&mime).unwrap_or_else(|_| {
                        HeaderValue::from_static("application/octet-stream")
                    }),
                );
                // `nosniff` is applied centrally in `HandlerCore::render`,
                // which every static response also funnels through.
                if let Some(length) = original_length
                    && let Ok(length) =
                        HeaderValue::from_str(&length.to_string())
                {
                    response.headers_mut().insert(CONTENT_LENGTH, length);
                }
                self.preset_res = Some(response.into());
            }
        }
        Ok(self)
    }

    fn generate_routes_with_exclusions_and_discovery_context<
        IV,
        AppFn,
        ContextFn,
    >(
        mut self,
        app_fn: AppFn,
        excluded_routes: Option<Vec<String>>,
        discovery_context: ContextFn,
    ) -> Result<Self, RegistrationError>
    where
        IV: IntoView + 'static,
        AppFn: Fn() -> IV + 'static + Send + Clone,
        ContextFn: Fn() + 'static + Send + Clone,
    {
        if self.routes_registered {
            return Err(RegistrationError::RoutesAlreadyGenerated);
        }

        // A claimed request never consults the SSR router: a server function,
        // a preset response, and a known 404 all resolve without it. Route
        // discovery renders the whole application to extract its `<Routes/>`,
        // so running it here only to drop every entry below is the single
        // largest avoidable cost on those paths - 183 us of a 1054 us request,
        // measured on `/api/get_test`. Both hosts instantiate a fresh
        // component per request, so this is paid on every one of them.
        //
        // Skipping also skips the validation below, which is why it is gated
        // on `shortcut()` rather than applied unconditionally: an SSR request
        // still builds the router and still rejects duplicate or static-mode
        // routes. A misconfigured route table therefore surfaces on any
        // request that would actually use it.
        if self.shortcut() {
            self.routes_registered = true;
            return Ok(self);
        }

        for (route_spec, listing) in validated_route_table(
            &app_fn,
            excluded_routes.as_deref(),
            &discovery_context,
        )? {
            match self.ssr_router.add(route_spec, listing) {
                Ok(()) => {}
                Err(infallible) => match infallible {},
            }
        }
        self.routes_registered = true;
        Ok(self)
    }

    async fn render<IV>(
        self,
        app: impl Fn() -> IV + 'static + Send + Clone,
        additional_context: impl Fn() + 'static + Clone + Send,
    ) -> Response
    where
        IV: IntoView + 'static,
    {
        let path = self.req.uri().path().to_string();
        let best_match = self.ssr_router.best_match(&path);
        let islands_navigation = is_islands_router_navigation(&self.req);
        let is_head = self.req.method() == Method::HEAD;
        let (parts, body) = self.req.into_parts();
        let context_parts = parts.clone();
        let req = Request::from_parts(parts, body);

        let owner = Owner::new();
        let render = owner.with(|| {
            ScopedFuture::new(async move {
                let res_opts = ResponseOptions::default();
                let response: Option<Response> = if self.should_404 {
                    None
                } else if let Some(response) = self.preset_res {
                    Some(response)
                } else if let Some(server_fn) = self.server_fn {
                    provide_standard_contexts(context_parts, res_opts.clone());
                    additional_context();

                    let accepts_html = accepts_html(req.headers());
                    let referrer = req
                        .headers()
                        .get(REFERER)
                        .or_else(|| req.headers().get("referrer"))
                        .cloned();
                    let mut response = server_fn(req).await;
                    apply_server_fn_redirect(
                        &mut response,
                        accepts_html,
                        referrer,
                    );
                    Some(response.into())
                } else if let Some(best_match) = best_match {
                    let listing = best_match.handler();
                    let (meta_context, meta_output) = ServerMetaContext::new();
                    let add_ctx = additional_context.clone();
                    let route_context = {
                        let res_opts = res_opts.clone();
                        let meta_context = meta_context.clone();
                        move || {
                            provide_context(meta_context);
                            provide_standard_contexts(context_parts, res_opts);
                            if islands_navigation {
                                provide_context(IslandsRouterNavigation);
                            }
                            add_ctx();
                        }
                    };

                    Some(
                        Response::from_app(
                            app,
                            meta_output,
                            route_context,
                            res_opts.clone(),
                            render_mode::<IV>(listing.mode().clone()),
                            !islands_navigation,
                        )
                        .await,
                    )
                } else {
                    None
                };

                response.map(|mut response| {
                    response.extend_response(&res_opts);
                    response
                })
            })
        });
        let response = render.await;

        let mut response = response.unwrap_or_else(|| {
            plain_response(StatusCode::NOT_FOUND, "404 not found")
        });
        // Single insertion point for every response this crate emits. It sits
        // after the `extend_response` tail, so an application value merged from
        // `ResponseOptions` is already present and wins, and after the 404
        // fallback, which never reaches that tail.
        set_default_nosniff(&mut response);
        if is_head {
            *response.0.body_mut() = Body::Sync(Bytes::new());
        } else if !response.0.headers().contains_key(CONTENT_LENGTH)
            && let Body::Sync(bytes) = response.0.body()
            && let Ok(value) = HeaderValue::from_str(&bytes.len().to_string())
        {
            response.0.headers_mut().insert(CONTENT_LENGTH, value);
        }
        response
    }
}

type RenderMode<IV> =
    fn(
        IV,
        Box<dyn FnOnce() -> PinnedStream<String> + Send>,
        bool,
    ) -> Pin<Box<dyn Future<Output = PinnedStream<String>> + Send>>;

// Keep this selection in one place so WASIp2 and WASIp3 cannot drift.
fn render_mode<IV>(mode: SsrMode) -> RenderMode<IV>
where
    IV: IntoView + 'static,
{
    match mode {
        SsrMode::Async | SsrMode::Static(_) => |app, chunks, _| {
            Box::pin(async move {
                let app = if cfg!(feature = "islands-router") {
                    app.to_html_stream_in_order_branching()
                } else {
                    app.to_html_stream_in_order()
                };
                let app = app.collect::<String>().await;
                Box::pin(once(async move { app }).chain(chunks()))
                    as PinnedStream<String>
            })
        },
        SsrMode::InOrder => |app, chunks, _| {
            Box::pin(async move {
                let app = if cfg!(feature = "islands-router") {
                    app.to_html_stream_in_order_branching()
                } else {
                    app.to_html_stream_in_order()
                };
                Box::pin(app.chain(chunks())) as PinnedStream<String>
            })
        },
        SsrMode::PartiallyBlocked | SsrMode::OutOfOrder => {
            |app, chunks, supports_out_of_order| {
                Box::pin(async move {
                    let app = if cfg!(feature = "islands-router") {
                        if supports_out_of_order {
                            app.to_html_stream_out_of_order_branching()
                        } else {
                            app.to_html_stream_in_order_branching()
                        }
                    } else if supports_out_of_order {
                        app.to_html_stream_out_of_order()
                    } else {
                        // Unreachable as the flag is derived today: without
                        // the `islands-router` feature
                        // `is_islands_router_navigation` is always false, so
                        // `supports_out_of_order` is always true here. Kept so
                        // the arm still behaves correctly if that derivation
                        // ever changes; it is deliberately not covered by a
                        // test, because no request can currently reach it.
                        app.to_html_stream_in_order()
                    };
                    Box::pin(app.chain(chunks())) as PinnedStream<String>
                })
            }
        }
    }
}

#[cfg(feature = "wasip2")]
pub mod wasip2;

#[cfg(feature = "wasip3")]
pub mod wasip3;

#[cfg(test)]
mod tests {
    use http::{header::ACCEPT, request::Parts};

    use http::header::LOCATION;

    use super::http_util::ISLANDS_ROUTER_HEADER;
    use super::policy::X_CONTENT_TYPE_OPTIONS;
    use super::test_support::static_route_app;
    use super::*;
    use leptos::prelude::{use_context, view};
    use leptos_router::{
        components::{Route, Router, Routes},
        path,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    static ROUTE_GENERATIONS: AtomicUsize = AtomicUsize::new(0);

    fn counted_route_app() -> impl IntoView {
        ROUTE_GENERATIONS.fetch_add(1, Ordering::Relaxed);
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/cached") view=|| view! { "cached" } />
                </Routes>
            </Router>
        }
    }

    fn repeated_route_app() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/repeated") view=|| view! { "repeated" } />
                </Routes>
            </Router>
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_body_read_timeout_response_is_labelled_like_other_rejections() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(
            policy_response(&RequestPolicyError::BodyReadTimeout {
                nanoseconds: 1,
            }),
            "request_policy",
        );

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    #[test]
    fn static_prefix_is_validated_after_a_response_was_selected() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");

        let result =
            core.static_files_handler("https://example.com/static", |_| None);
        assert!(matches!(
            result,
            Err(RegistrationError::InvalidStaticPrefix(_))
        ));
    }

    #[test]
    fn static_ssr_is_rejected_on_a_request_that_uses_the_router() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        );

        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                static_route_app,
                None,
                || {},
            );
        assert!(matches!(
            result,
            Err(RegistrationError::UnsupportedStaticSsr(path))
                if path == "/static"
        ));
    }

    /// The cost of skipping discovery on claimed requests, stated as a test so
    /// it is a decision on the record rather than a surprise. An application
    /// served only through server functions and static assets never validates
    /// its route table in production; [`validate_route_table`] is the
    /// replacement, and the two tests below pin both halves of that trade.
    #[test]
    fn a_claimed_request_does_not_reject_an_invalid_route_table() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");

        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                static_route_app,
                None,
                || {},
            );
        assert!(result.is_ok());
    }

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

        assert_eq!(core.trace_route_class, Some("static"));
        assert_eq!(core.trace_path.as_deref(), Some("/static/nested asset.js"));
    }

    #[test]
    fn route_discovery_runs_once_per_registration() {
        // Deliberately uncached. Both supported hosts instantiate a fresh
        // component per request, so a cache keyed on the application type
        // could only ever hit within a single request - and discovery runs
        // once per request, so it never hit at all. Pinning the count at one
        // generation per registration keeps a future cache from being
        // reintroduced on the assumption that it pays for itself.
        ROUTE_GENERATIONS.store(0, Ordering::Relaxed);
        for _ in 0..2 {
            let core = HandlerCore::new(
                Request::new(Bytes::new()),
                HandlerConfig::default(),
            );
            let result = core
                .generate_routes_with_exclusions_and_discovery_context(
                    counted_route_app,
                    None,
                    || {},
                );
            assert!(result.is_ok());
        }

        assert_eq!(ROUTE_GENERATIONS.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_claimed_request_never_discovers_routes() {
        // The saving that motivates the shortcut: an already-selected response
        // resolves without the SSR router, and discovery renders the whole
        // application. Measured at 183 us of a 1054 us request, paid on every
        // request because each one gets a fresh component instance.
        ROUTE_GENERATIONS.store(0, Ordering::Relaxed);
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");
        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                counted_route_app,
                None,
                || {},
            );

        assert!(result.is_ok());
        assert_eq!(ROUTE_GENERATIONS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn repeated_route_registration_is_rejected_for_shortcuts() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");
        let core = core
            .generate_routes_with_exclusions_and_discovery_context(
                repeated_route_app,
                None,
                || {},
            )
            .expect("initial route registration should succeed");

        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                repeated_route_app,
                None,
                || {},
            );
        assert!(matches!(
            result,
            Err(RegistrationError::RoutesAlreadyGenerated)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_context_runs_after_standard_parts_for_each_request() {
        const CONTEXT_HEADER: &str = "x-context-lifecycle";
        let observed = Arc::new(Mutex::new(Vec::new()));

        for value in ["first", "second"] {
            let request = Request::builder()
                .uri("/api/context")
                .header(CONTEXT_HEADER, value)
                .body(Bytes::new())
                .expect("test request should be valid");
            let mut core = HandlerCore::new(request, HandlerConfig::default());
            core.server_fn = Some(Box::new(|_| {
                Box::pin(async {
                    http::Response::new(Body::Sync(Bytes::from_static(b"ok")))
                })
            }));

            let observed = Arc::clone(&observed);
            let _response = core
                .render(
                    || view! { "unused" },
                    move || {
                        let parts = use_context::<Parts>().expect(
                            "standard request parts should be installed",
                        );
                        let value = parts
                            .headers
                            .get(CONTEXT_HEADER)
                            .expect("request header should be preserved")
                            .to_str()
                            .expect("test header should be text")
                            .to_owned();
                        observed
                            .lock()
                            .expect("test observation lock should be available")
                            .push(value);
                    },
                )
                .await;
        }

        assert_eq!(
            *observed
                .lock()
                .expect("test observation lock should be available"),
            ["first", "second"]
        );
    }

    /// `ResponseOptions` is the documented escape hatch for redirects that
    /// legitimately leave the origin (OAuth authorization endpoints, payment
    /// providers). `extend_response` runs *after* `apply_server_fn_redirect`
    /// and `http`'s `Extend` impl replaces rather than appends, so a
    /// `Location` written through the reactive context wins outright and is
    /// never reduced to a path. This pins that contract end to end.
    #[tokio::test(flavor = "current_thread")]
    async fn response_options_location_survives_server_fn_sanitizing() {
        const OFF_ORIGIN: &str =
            "https://accounts.example.com/oauth/authorize?client_id=leptos";

        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/start_oauth")
            .header(ACCEPT, "text/html")
            .header(REFERER, "http://127.0.0.1/previous-page")
            .body(Bytes::new())
            .expect("test request should be valid");
        let mut core = HandlerCore::new(request, HandlerConfig::default());
        core.server_fn = Some(Box::new(|_| {
            Box::pin(async {
                http::Response::new(Body::Sync(Bytes::from_static(b"ok")))
            })
        }));

        let response = core
            .render(
                || view! { "unused" },
                || {
                    use_context::<ResponseOptions>()
                        .expect("response options should be installed")
                        .insert_header(
                            LOCATION,
                            HeaderValue::from_static(OFF_ORIGIN),
                        );
                },
            )
            .await;

        // `apply_server_fn_redirect` saw an html form POST with a `Referer`
        // and still promoted the status, so this is the same code path a real
        // form redirect takes.
        assert_eq!(response.0.status(), StatusCode::FOUND);
        assert_eq!(
            response.0.headers().get(LOCATION),
            Some(&HeaderValue::from_static(OFF_ORIGIN)),
            "a Location set through ResponseOptions must reach the client \
             unchanged, including the scheme and authority"
        );
    }

    /// The complementary half: a `Location` the server function wrote onto its
    /// own `http::Response` is *not* an escape hatch. It is sanitized down to
    /// a path, and it takes precedence over the `Referer` fallback.
    #[tokio::test(flavor = "current_thread")]
    async fn server_fn_response_location_is_reduced_to_a_path() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/form_submit")
            .header(ACCEPT, "text/html")
            .header(REFERER, "http://127.0.0.1/previous-page")
            .body(Bytes::new())
            .expect("test request should be valid");
        let mut core = HandlerCore::new(request, HandlerConfig::default());
        core.server_fn = Some(Box::new(|_| {
            Box::pin(async {
                let mut response =
                    http::Response::new(Body::Sync(Bytes::from_static(b"ok")));
                *response.status_mut() = StatusCode::FOUND;
                response.headers_mut().insert(
                    LOCATION,
                    HeaderValue::from_static(
                        "https://malicious.example.com/steal-session?token=1",
                    ),
                );
                response
            })
        }));

        let response = core.render(|| view! { "unused" }, || {}).await;

        assert_eq!(response.0.status(), StatusCode::FOUND);
        let location = response
            .0
            .headers()
            .get(LOCATION)
            .expect("redirect should carry a Location")
            .to_str()
            .expect("Location should be text");
        assert_eq!(
            location, "/steal-session?token=1",
            "the server function's own Location must be reduced to a \
             same-origin path, not replaced by the Referer"
        );
        assert!(!location.contains("malicious.example.com"));
    }

    fn ssr_arm_app() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/rendered") view=|| view! { "rendered" } />
                </Routes>
            </Router>
        }
    }

    /// The application-override guarantee proven on the SSR arm itself.
    ///
    /// `application_nosniff_override_wins` drives the server-function arm, and
    /// the default is applied once after the shared `extend_response` tail, so
    /// the mechanism is common to both. Pinning it here as well means a future
    /// change that moves the default into the individual branches cannot
    /// silently start clobbering an application's own value on SSR responses.
    #[tokio::test(flavor = "current_thread")]
    async fn application_nosniff_override_wins_on_the_ssr_arm() {
        // `Response::from_app` drives the render through the reactive graph,
        // which needs a spawner installed before the route can render.
        let _ = any_spawner::Executor::init_futures_executor();

        let request = Request::builder()
            .uri("/rendered")
            .body(Bytes::new())
            .expect("test request should be valid");
        let core = HandlerCore::new(request, HandlerConfig::default())
            .generate_routes_with_exclusions_and_discovery_context(
                ssr_arm_app,
                None,
                || {},
            )
            .expect("route registration should succeed");

        let response = core
            .render(ssr_arm_app, || {
                let options = use_context::<ResponseOptions>()
                    .expect("response options should be installed");
                options.insert_header(
                    http::header::HeaderName::from_static(
                        X_CONTENT_TYPE_OPTIONS,
                    ),
                    HeaderValue::from_static("off"),
                );
            })
            .await;

        assert_eq!(
            response
                .0
                .headers()
                .get_all(X_CONTENT_TYPE_OPTIONS)
                .iter()
                .count(),
            1,
            "the crate must not append a second value"
        );
        assert_eq!(
            response
                .0
                .headers()
                .get(X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("off"),
            "an application value set through ResponseOptions must survive"
        );
    }

    #[cfg(feature = "islands-router")]
    mod islands_router_streaming {
        use super::*;
        use leptos::prelude::{ElementChild, Suspend, Suspense};
        use std::{
            task::{Context, Poll},
            time::{Duration, Instant},
        };

        /// How long the suspended resource stays unresolved. Waking the poller
        /// immediately is not enough: the `Suspense` boundary settles on a
        /// worker thread, so it can win the race and inline the resolved
        /// markup even in out-of-order mode. A wall-clock gate keeps the first
        /// poll reliably pending.
        const RESOURCE_DELAY: Duration = Duration::from_millis(50);

        /// Stays pending until [`RESOURCE_DELAY`] has elapsed, forcing an
        /// out-of-order stream to emit the `Suspense` fallback first.
        struct PendingResource(Instant);

        impl Default for PendingResource {
            fn default() -> Self {
                Self(Instant::now() + RESOURCE_DELAY)
            }
        }

        impl Future for PendingResource {
            type Output = ();

            fn poll(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                let now = Instant::now();
                if now >= self.0 {
                    return Poll::Ready(());
                }
                // The reactive graph may poll this from any thread, so wake
                // from a plain thread instead of a runtime-bound timer.
                let remaining = self.0 - now;
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(remaining);
                    waker.wake();
                });
                Poll::Pending
            }
        }

        fn out_of_order_app() -> impl IntoView {
            view! {
                <Router>
                    <Routes fallback=|| view! { "not found" }>
                        <Route
                            path=path!("/streamed")
                            ssr=SsrMode::OutOfOrder
                            view=|| {
                                view! {
                                    <Suspense fallback=|| view! { <p>"pending"</p> }>
                                        {move || Suspend::new(async move {
                                            PendingResource::default().await;
                                            view! { <p>"resolved"</p> }
                                        })}
                                    </Suspense>
                                }
                            }
                        />
                    </Routes>
                </Router>
            }
        }

        async fn render_streamed(islands_navigation: bool) -> String {
            // `Suspense` drives its boundary with an isomorphic effect, so the
            // reactive graph needs a spawner before this route can render.
            let _ = any_spawner::Executor::init_futures_executor();

            let mut builder = Request::builder().uri("/streamed");
            if islands_navigation {
                builder = builder.header(ISLANDS_ROUTER_HEADER, "1");
            }
            let request = builder
                .body(Bytes::new())
                .expect("test request should be valid");
            let core = HandlerCore::new(request, HandlerConfig::default())
                .generate_routes_with_exclusions_and_discovery_context(
                    out_of_order_app,
                    None,
                    || {},
                )
                .expect("route registration should succeed");
            let response = core.render(out_of_order_app, || {}).await;
            match response.0.into_body() {
                Body::Sync(bytes) => String::from_utf8(bytes.to_vec())
                    .expect("rendered body should be UTF-8"),
                Body::Async(stream) => {
                    stream
                        .map(|chunk| {
                            let chunk =
                                chunk.expect("stream chunk should not fail");
                            String::from_utf8(chunk.to_vec())
                                .expect("rendered chunk should be UTF-8")
                        })
                        .collect::<String>()
                        .await
                }
            }
        }

        #[tokio::test(flavor = "current_thread")]
        async fn islands_router_navigation_downgrades_out_of_order_streaming() {
            let streamed = render_streamed(false).await;
            let navigated = render_streamed(true).await;

            // Branching markup is selected by the cargo feature, so both
            // responses carry it; only the streaming order reacts to the header.
            assert!(
                streamed.contains("<!--bo-"),
                "branching markup should be emitted without the header, got: {streamed}"
            );
            assert!(
                navigated.contains("<!--bo-"),
                "branching markup should survive the downgrade, got: {navigated}"
            );

            // Out-of-order streaming ships the `Suspense` fallback first and
            // defers the resolved markup into a trailing `<template>`.
            assert!(
                streamed.contains("<p>pending</p>"),
                "out-of-order streaming should emit the Suspense fallback, got: {streamed}"
            );
            let deferred = streamed.find("<template id=").unwrap_or_else(|| {
                panic!(
                    "out-of-order streaming should defer markup into a template, got: {streamed}"
                )
            });
            let resolved =
                streamed.find("<p>resolved</p>").unwrap_or_else(|| {
                    panic!(
                        "out-of-order streaming should still emit the resolved markup, got: {streamed}"
                    )
                });
            assert!(
                deferred < resolved,
                "resolved markup should only appear inside the deferred template, got: {streamed}"
            );

            // An islands-router navigation downgrades to in-order streaming:
            // the resolved markup is inlined and none of the out-of-order
            // machinery (fallback, template, suspense markers) is present.
            assert!(
                navigated.contains("<p>resolved</p>"),
                "in-order streaming should inline the resolved markup, got: {navigated}"
            );
            assert!(
                !navigated.contains("<template id="),
                "an islands-router navigation must not defer markup into a template, got: {navigated}"
            );
            assert!(
                !navigated.contains("<p>pending</p>"),
                "an islands-router navigation must not emit the Suspense fallback, got: {navigated}"
            );
            assert!(
                !navigated.contains("<!--s-"),
                "an islands-router navigation must not emit suspense stream markers, got: {navigated}"
            );
        }
    }
    fn header_of<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
        response
            .0
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn nosniff_of(response: &Response) -> Option<&str> {
        header_of(response, X_CONTENT_TYPE_OPTIONS)
    }

    fn static_asset_core(method: Method) -> HandlerCore {
        HandlerCore::new(
            Request::builder()
                .method(method)
                .uri("/static/app.js")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        )
        .static_files_handler("/static", |_| {
            Some(Body::Sync(Bytes::from_static(b"console.log(1)")))
        })
        .expect("static registration should succeed")
    }

    async fn render_plain(core: HandlerCore) -> Response {
        core.render(|| view! { "unused" }, || {}).await
    }

    /// Covers the shape an SSR render reaches `render`'s tail in: a response
    /// that already carries the `text/html; charset=utf-8` installed by
    /// `set_default_content_type` inside `Response::from_app`.
    ///
    /// This drives the preset arm rather than the SSR arm, so it pins the
    /// content-type interaction but not SSR-specific precedence. That is not a
    /// coverage hole: the default is applied once, after the shared
    /// `extend_response` tail, so every arm that produced a response reaches
    /// it by the same path. `application_nosniff_override_wins` proves the
    /// precedence itself through the server-function arm.
    #[tokio::test(flavor = "current_thread")]
    async fn html_responses_gain_nosniff_and_keep_their_content_type() {
        let mut html = http::Response::new(Body::Sync(Bytes::from_static(
            b"<!DOCTYPE html><html></html>",
        )));
        html.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        let core = HandlerCore::new(
            Request::builder()
                .method(Method::GET)
                .uri("/rendered")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        )
        .with_preset(html.into(), "ssr");

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::OK);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/html; charset=utf-8")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn preset_responses_carry_nosniff() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::OK);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_fn_responses_carry_nosniff() {
        let mut core = HandlerCore::new(
            Request::builder()
                .uri("/api/echo")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        );
        core.server_fn = Some(Box::new(|_| {
            Box::pin(async {
                let mut response =
                    http::Response::new(Body::Sync(Bytes::from_static(b"{}")));
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response
            })
        }));

        let response = render_plain(core).await;

        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("application/json")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn not_found_fallback_carries_nosniff() {
        let core = HandlerCore::new(
            Request::builder()
                .uri("/missing")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        );

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::NOT_FOUND);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn policy_rejections_carry_nosniff() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(
            policy_response(&RequestPolicyError::BodyTooLarge { limit: 16 }),
            "request_policy",
        );

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    /// The server-function body guard builds its own 413 and returns it
    /// through the `server_fn` arm, so it reaches the wire by a different path
    /// than the request-policy 413 injected as a preset.
    #[tokio::test(flavor = "current_thread")]
    async fn server_fn_body_limit_rejection_carries_nosniff() {
        let limit = 8_usize;
        let mut core = HandlerCore::new(
            Request::builder()
                .uri("/api/upload")
                .body(Bytes::from_static(b"oversized payload"))
                .expect("test request should be valid"),
            HandlerConfig::default().with_max_request_body_size(limit),
        );
        core.server_fn = Some(Box::new(move |request| {
            Box::pin(async move {
                let (_, bytes) = request.into_parts();
                assert!(bytes.len() > limit, "fixture must exceed the limit");
                plain_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("request body exceeds limit of {limit} bytes"),
                )
                .0
            })
        }));

        let response = render_plain(core).await;

        assert_eq!(response.0.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_method_not_allowed_carries_nosniff() {
        let response = render_plain(static_asset_core(Method::POST)).await;

        assert_eq!(response.0.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(header_of(&response, "allow"), Some("GET, HEAD"));
        assert_eq!(
            header_of(&response, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_files_still_carry_nosniff_after_centralisation() {
        let response = render_plain(static_asset_core(Method::GET)).await;

        assert_eq!(response.0.status(), StatusCode::OK);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert!(
            header_of(&response, "content-type")
                .is_some_and(|value| value.contains("javascript"))
        );
        assert_eq!(
            response
                .0
                .headers()
                .get_all(X_CONTENT_TYPE_OPTIONS)
                .iter()
                .count(),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_responses_keep_nosniff() {
        let response = render_plain(static_asset_core(Method::HEAD)).await;

        assert_eq!(response.0.status(), StatusCode::OK);
        assert_eq!(nosniff_of(&response), Some("nosniff"));
        assert_eq!(header_of(&response, "content-length"), Some("14"));
        assert!(
            matches!(response.0.body(), Body::Sync(bytes) if bytes.is_empty())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn application_nosniff_override_wins() {
        let mut core = HandlerCore::new(
            Request::builder()
                .uri("/api/echo")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        );
        core.server_fn = Some(Box::new(|_| {
            Box::pin(async {
                http::Response::new(Body::Sync(Bytes::from_static(b"ok")))
            })
        }));

        let response = core
            .render(
                || view! { "unused" },
                || {
                    use_context::<ResponseOptions>()
                        .expect("response options should be installed")
                        .insert_header(
                            http::header::HeaderName::from_static(
                                X_CONTENT_TYPE_OPTIONS,
                            ),
                            HeaderValue::from_static("off"),
                        );
                },
            )
            .await;

        assert_eq!(nosniff_of(&response), Some("off"));
        assert_eq!(
            response
                .0
                .headers()
                .get_all(X_CONTENT_TYPE_OPTIONS)
                .iter()
                .count(),
            1
        );
    }
}
