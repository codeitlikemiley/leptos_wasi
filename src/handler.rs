#![forbid(unsafe_code)]

//! Shared Leptos request handling and runtime-specific WASI HTTP adapters.

use std::{
    any::TypeId,
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};
#[cfg(feature = "tracing")]
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

use bytes::Bytes;
use futures::{StreamExt, stream::once};
use http::{
    HeaderMap, HeaderValue, Method, Request, StatusCode, Uri,
    header::{ACCEPT, ALLOW, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, REFERER},
    request::Parts,
};
use hydration_context::SsrSharedContext;
use leptos::{
    IntoView,
    hydration::IslandsRouterNavigation,
    prelude::{Owner, ScopedFuture, provide_context},
};
use leptos_meta::ServerMetaContext;
use leptos_router::{
    ExpandOptionals, PathSegment, RouteList, RouteListing, SsrMode,
    components::provide_server_redirect, location::RequestUrl,
};
use mime_guess::MimeGuess;
use routefinder::{RouteSpec, Router, Segment};
use server_fn::{
    Protocol, ServerFn,
    error::{FromServerFnError, ServerFnErrorErr},
    middleware::{BoxedService, Service},
};
use thiserror::Error;

use crate::{
    __private::ServerWithBody,
    integration::{ExtendResponse, PinnedStream},
    response::{Body, Response, ResponseOptions},
    utils::redirect,
};

/// Default maximum request body size: 16 MiB.
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: usize = 16 * 1024 * 1024;

const ISLANDS_ROUTER_HEADER: &str = "Islands-Router";
const X_CONTENT_TYPE_OPTIONS: &str = "x-content-type-options";

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

/// Request policy applied while converting incoming WASI HTTP requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandlerConfig {
    max_request_body_size: usize,
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
}

impl Default for HandlerConfig {
    fn default() -> Self {
        Self {
            max_request_body_size: DEFAULT_MAX_REQUEST_BODY_SIZE,
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
}

impl RequestPolicyError {
    const fn status(&self) -> StatusCode {
        match self {
            Self::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
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

fn policy_response(error: &RequestPolicyError) -> Response {
    plain_response(error.status(), error.to_string())
}

#[cfg(feature = "tracing")]
fn trace_policy_rejection(preview: &'static str, error: &RequestPolicyError) {
    let error_class = match error {
        RequestPolicyError::BodyTooLarge { .. } => "body_too_large",
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

fn plain_response(status: StatusCode, message: impl Into<Bytes>) -> Response {
    let mut response = http::Response::new(Body::Sync(message.into()));
    *response.status_mut() = status;
    response.into()
}

type ServerFnHandler = Box<
    dyn Fn(
            Request<Bytes>,
        )
            -> Pin<Box<dyn Future<Output = http::Response<Body>> + Send>>
        + Send,
>;

type ReqBody<T> = <<T as ServerFn>::Server as ServerWithBody<
    <T as ServerFn>::Error,
    <T as ServerFn>::InputStreamError,
    <T as ServerFn>::OutputStreamError,
>>::ReqBody;

type ResBody<T> = <<T as ServerFn>::Server as ServerWithBody<
    <T as ServerFn>::Error,
    <T as ServerFn>::InputStreamError,
    <T as ServerFn>::OutputStreamError,
>>::ResBody;

struct TypedServerFnService<T>(PhantomData<fn() -> T>);

impl<T> Default for TypedServerFnService<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T> Service<Request<ReqBody<T>>, http::Response<ResBody<T>>>
    for TypedServerFnService<T>
where
    T: ServerFn + 'static,
    T::Server:
        ServerWithBody<T::Error, T::InputStreamError, T::OutputStreamError>,
    ReqBody<T>: Send + 'static,
    ResBody<T>: Send + 'static,
{
    fn run(
        &mut self,
        request: Request<ReqBody<T>>,
        _serialize_error: fn(ServerFnErrorErr) -> Bytes,
    ) -> Pin<
        Box<dyn Future<Output = http::Response<ResBody<T>>> + Send + 'static>,
    > {
        Box::pin(T::run_on_server(request))
    }
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RouteCacheKey {
    app: TypeId,
    context: TypeId,
}

type CachedRoutes =
    Result<Vec<(String, RouteSpec, RouteListing)>, RegistrationError>;

thread_local! {
    static ROUTE_CACHE: RefCell<HashMap<RouteCacheKey, CachedRoutes>> =
        RefCell::new(HashMap::new());
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
            let limit = self.config.max_request_body_size;
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
                response.headers_mut().insert(
                    http::header::HeaderName::from_static(
                        X_CONTENT_TYPE_OPTIONS,
                    ),
                    HeaderValue::from_static("nosniff"),
                );
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

        let routes = registered_routes(&app_fn, &discovery_context)?;
        let routes = routes.into_iter().filter(|route| {
            excluded_routes
                .as_ref()
                .is_none_or(|excluded| !excluded.contains(&route.0))
        });
        let shortcut = self.shortcut();
        let mut registered_paths = BTreeSet::new();
        for (path, route_spec, listing) in routes {
            let collision_key = route_collision_key(&route_spec);
            if !registered_paths.insert(collision_key) {
                return Err(RegistrationError::DuplicateRoute(path));
            }
            if matches!(listing.mode(), SsrMode::Static(_)) {
                return Err(RegistrationError::UnsupportedStaticSsr(path));
            }
            if shortcut {
                continue;
            }
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

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RouteCollisionSegment {
    Slash,
    Dot,
    Exact(String),
    Param,
    Wildcard,
}

fn route_collision_key(route: &RouteSpec) -> Vec<RouteCollisionSegment> {
    route
        .segments()
        .iter()
        .map(|segment| match segment {
            Segment::Slash => RouteCollisionSegment::Slash,
            Segment::Dot => RouteCollisionSegment::Dot,
            Segment::Exact(value) => {
                RouteCollisionSegment::Exact(value.to_string())
            }
            Segment::Param(_) => RouteCollisionSegment::Param,
            Segment::Wildcard => RouteCollisionSegment::Wildcard,
        })
        .collect()
}

fn registered_routes<IV, AppFn, ContextFn>(
    app_fn: &AppFn,
    discovery_context: &ContextFn,
) -> CachedRoutes
where
    IV: IntoView + 'static,
    AppFn: Fn() -> IV + 'static + Send + Clone,
    ContextFn: Fn() + 'static + Send + Clone,
{
    let key = RouteCacheKey {
        app: TypeId::of::<AppFn>(),
        context: TypeId::of::<ContextFn>(),
    };
    // A `TypeId` identifies behavior only when the type has exactly one
    // inhabitant. Zero-sized function items and non-capturing closures qualify;
    // function pointers and capturing closures do not, and two distinct
    // applications coerced to the same `fn()` type would otherwise share one
    // cached route list.
    let cacheable = size_of::<AppFn>() == 0 && size_of::<ContextFn>() == 0;
    if cacheable
        && let Some(cached) =
            ROUTE_CACHE.with(|cache| cache.borrow().get(&key).cloned())
    {
        return cached;
    }

    let generated: CachedRoutes = {
        let owner = Owner::new_root(Some(Arc::new(SsrSharedContext::new())));
        let routes = owner
            .with(|| {
                let (meta, _) = ServerMetaContext::new();
                let (parts, _) = Request::new("").into_parts();
                provide_context(meta);
                provide_standard_contexts(parts, ResponseOptions::default());
                discovery_context();
                RouteList::generate(app_fn)
            })
            .unwrap_or_default()
            .into_inner()
            .into_iter()
            .flat_map(IntoRouteListing::into_route_listing)
            .collect::<Vec<_>>();

        // Validate each pattern independently. Duplicate collisions are
        // checked after exclusions are applied for the current registration.
        routes
            .into_iter()
            .map(|(path, listing)| {
                let route_spec =
                    RouteSpec::try_from(path.as_str()).map_err(|reason| {
                        RegistrationError::InvalidRoute {
                            path: path.clone(),
                            reason,
                        }
                    })?;
                Ok((path, route_spec, listing))
            })
            .collect()
    };

    if cacheable {
        ROUTE_CACHE.with(|cache| {
            cache.borrow_mut().insert(key, generated.clone());
        });
    }
    generated
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
                        app.to_html_stream_in_order()
                    };
                    Box::pin(app.chain(chunks())) as PinnedStream<String>
                })
            }
        }
    }
}

fn provide_standard_contexts(parts: Parts, response: ResponseOptions) {
    let request_url = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    provide_context(RequestUrl::new(request_url));
    provide_context(parts);
    provide_context(response);
    provide_server_redirect(redirect);
    leptos::nonce::provide_nonce();
}

fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|media_range| {
            let mut fields = media_range.split(';');
            let media_type = fields.next().unwrap_or_default().trim();
            let quality = fields
                .filter_map(|field| field.trim().strip_prefix("q="))
                .filter_map(|value| value.parse::<f32>().ok())
                .next()
                .unwrap_or(1.0);
            quality > 0.0
                && matches!(media_type, "text/html" | "application/xhtml+xml")
        })
}

fn apply_server_fn_redirect(
    response: &mut http::Response<Body>,
    accepts_html: bool,
    referrer: Option<HeaderValue>,
) {
    let mut redirect_target = None;
    if accepts_html && let Some(referrer) = referrer {
        let is_default = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            == Some("/");
        let has_location = response.headers().contains_key(LOCATION);
        if !has_location || is_default {
            *response.status_mut() = StatusCode::FOUND;
            redirect_target = sanitize_referrer(&referrer)
                .or_else(|| Some(HeaderValue::from_static("/")));
        }
    }
    if redirect_target.is_none()
        && let Some(location) = response.headers().get(LOCATION).cloned()
    {
        redirect_target = sanitize_referrer(&location)
            .or_else(|| Some(HeaderValue::from_static("/")));
    }
    if let Some(target) = redirect_target {
        response.headers_mut().insert(LOCATION, target);
    }
}

fn is_islands_router_navigation<B>(request: &Request<B>) -> bool {
    cfg!(feature = "islands-router")
        && request.headers().contains_key(ISLANDS_ROUTER_HEADER)
}

fn sanitize_referrer(referrer: &HeaderValue) -> Option<HeaderValue> {
    let value = referrer.to_str().ok()?;
    let uri = value.parse::<Uri>().ok()?;
    let path = uri.path_and_query()?.as_str();
    if path.starts_with("/\\")
        || path.contains('\\')
        || path.contains("%5c")
        || path.contains("%5C")
    {
        return None;
    }
    if path.starts_with('/') && !path.starts_with("//") {
        HeaderValue::from_str(path).ok()
    } else {
        None
    }
}

trait IntoRouteListing: Sized {
    fn into_route_listing(self) -> Vec<(String, RouteListing)>;
}

impl IntoRouteListing for RouteListing {
    fn into_route_listing(self) -> Vec<(String, RouteListing)> {
        self.path()
            .to_vec()
            .expand_optionals()
            .into_iter()
            .map(|path| {
                let path = path.to_rf_str_representation();
                let path = if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                };
                (path, self.clone())
            })
            .collect()
    }
}

trait RouterPathRepresentation {
    fn to_rf_str_representation(&self) -> String;
}

impl RouterPathRepresentation for Vec<PathSegment> {
    fn to_rf_str_representation(&self) -> String {
        let mut path = String::new();
        for segment in self {
            let raw = segment.as_raw_str();
            if !raw.is_empty() && !raw.starts_with('/') {
                path.push('/');
            }
            match segment {
                PathSegment::Static(value) => path.push_str(value),
                PathSegment::Param(value) => {
                    path.push(':');
                    path.push_str(value);
                }
                PathSegment::Splat(_) => path.push('*'),
                PathSegment::Unit => {}
                PathSegment::OptionalParam(_) => {}
            }
        }
        path
    }
}

macro_rules! common_handler_methods {
    () => {
        /// Registers a typed Leptos server function.
        #[must_use]
        pub fn with_server_fn<T>(mut self) -> Self
        where
            T: ServerFn + 'static,
            T::Server: ServerWithBody<
                    T::Error,
                    T::InputStreamError,
                    T::OutputStreamError,
                >,
            ReqBody<T>: From<Bytes> + Send + 'static,
            ResBody<T>: Into<Body> + Send + 'static,
        {
            self.core = self.core.with_server_fn::<T>();
            self
        }

        /// Registers a static-file callback for one URI prefix.
        pub fn static_files_handler<T>(
            mut self,
            prefix: T,
            handler: impl Fn(String) -> Option<Body> + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            T: TryInto<Uri>,
            <T as TryInto<Uri>>::Error: std::error::Error,
        {
            self.core = self.core.static_files_handler(prefix, handler)?;
            Ok(self)
        }

        /// Generates Leptos routes for the application.
        pub fn generate_routes<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app,
                None,
                || {},
            )
        }

        /// Generates routes with deterministic route-discovery context.
        ///
        /// The context closure runs only while discovering the application's
        /// route list. It receives synthetic standard contexts and may be
        /// skipped when an identical application/context closure type is
        /// already cached. It must not inspect authentication, headers, or any
        /// other request-dependent state.
        ///
        /// Use [`Self::handle_with_context`] for per-request context.
        pub fn generate_routes_with_discovery_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app, None, context,
            )
        }

        /// Compatibility alias for route-discovery context.
        ///
        /// This method has always applied `context` while discovering routes;
        /// request-dependent context belongs in [`Self::handle_with_context`].
        pub fn generate_routes_with_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_discovery_context(app, context)
        }

        /// Generates routes with exclusions and deterministic discovery context.
        ///
        /// Route discovery is cached per concrete application/context closure
        /// type, and only when both are zero-sized (function items or
        /// non-capturing closures) so that one type cannot describe two
        /// different applications. Function pointers and capturing closures
        /// re-run discovery on every request.
        /// The context closure runs against synthetic standard contexts
        /// only when route discovery executes. Route structure, exclusions,
        /// and discovery context must be deterministic deployment
        /// configuration rather than request-dependent state.
        ///
        /// Use [`Self::handle_with_context`] for per-request context.
        pub fn generate_routes_with_exclusions_and_discovery_context<IV>(
            mut self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            excluded: Option<Vec<String>>,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.core = self
                .core
                .generate_routes_with_exclusions_and_discovery_context(
                    app, excluded, context,
                )?;
            Ok(self)
        }

        /// Compatibility alias for exclusions plus route-discovery context.
        ///
        /// This method has always applied `context` while discovering routes;
        /// request-dependent context belongs in [`Self::handle_with_context`].
        pub fn generate_routes_with_exclusions_and_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            excluded: Option<Vec<String>>,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app, excluded, context,
            )
        }
    };
}

/// WASI Preview 2 request handler.
#[cfg(feature = "wasip2")]
pub mod wasip2 {
    use futures::StreamExt;
    use wasi::{
        http::types::{
            IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
        },
        io::streams::{OutputStream, StreamError},
    };

    use super::*;

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
        pub fn build(
            request: IncomingRequest,
            response_out: ResponseOutparam,
        ) -> Result<Self, HandlerError> {
            Self::build_with_config(
                request,
                response_out,
                HandlerConfig::default(),
            )
        }

        /// Builds a handler with an explicit request policy.
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
            match crate::request::p2::from_wasi_request(
                request,
                config.max_request_body_size(),
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
}

/// WASI Preview 3 request handler.
#[cfg(feature = "wasip3")]
pub mod wasip3 {
    use http_body_util::{BodyExt, Limited};

    use super::*;

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
        pub async fn build(
            request: Request<::wasip3::http_compat::IncomingRequestBody>,
        ) -> Result<Self, HandlerError> {
            Self::build_with_config(request, HandlerConfig::default()).await
        }

        /// Builds a handler with an explicit request policy.
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
            match body.collect().await {
                Ok(body) => {
                    let core = HandlerCore::new(
                        Request::from_parts(parts, body.to_bytes()),
                        config,
                    );
                    #[cfg(feature = "tracing")]
                    let core = core.with_request_started(request_started);
                    Ok(Self { core })
                }
                Err(error)
                    if error.is::<http_body_util::LengthLimitError>() =>
                {
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
                        .map(|code| *code)
                        .unwrap_or(
                            ::wasip3::http::types::ErrorCode::InternalError(
                                None,
                            ),
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
            let response = response.0.map(|body| {
                body.map_frame(|frame| frame.map_data(WasiBuf::new))
                    .map_err(|_| {
                        std::io::Error::other("response stream failure")
                    })
            });
            #[cfg(feature = "tracing")]
            let response = response
                .map(|body| TraceBody::new(body, trace.clone(), status));
            #[cfg(not(feature = "tracing"))]
            let _ = (trace, status);
            ::wasip3::http_compat::http_into_wasi_response(response)
                .map_err(HandlerError::Wasi)
        }
    }

    #[derive(Clone, Debug)]
    struct WasiBuf {
        bytes: Bytes,
        offset: usize,
    }

    impl WasiBuf {
        fn new(bytes: Bytes) -> Self {
            Self { bytes, offset: 0 }
        }
    }

    impl bytes::Buf for WasiBuf {
        fn remaining(&self) -> usize {
            self.bytes.len().saturating_sub(self.offset)
        }

        fn chunk(&self) -> &[u8] {
            &self.bytes[self.offset..]
        }

        fn advance(&mut self, count: usize) {
            self.offset = (self.offset + count).min(self.bytes.len());
        }
    }

    impl From<WasiBuf> for Vec<u8> {
        fn from(buffer: WasiBuf) -> Self {
            let remaining = if buffer.offset == 0 {
                buffer.bytes
            } else {
                buffer.bytes.slice(buffer.offset..)
            };
            remaining.into()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::{use_context, view};
    use leptos_router::{
        components::{Route, Router, Routes},
        path,
        static_routes::StaticRoute,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    static ROUTE_GENERATIONS: AtomicUsize = AtomicUsize::new(0);

    fn parsed_route_collision_key(
        path: &str,
    ) -> Result<Vec<RouteCollisionSegment>, String> {
        RouteSpec::try_from(path).map(|route| route_collision_key(&route))
    }

    fn static_route_app() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route
                        path=path!("/static")
                        ssr=SsrMode::Static(StaticRoute::new())
                        view=|| view! { "static" }
                    />
                </Routes>
            </Router>
        }
    }

    fn duplicate_route_app() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/duplicate/:id?") view=|| view! { "one" } />
                    <Route path=path!("/duplicate") view=|| view! { "two" } />
                </Routes>
            </Router>
        }
    }

    fn semantic_duplicate_route_app() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/users/:id") view=|| view! { "one" } />
                    <Route path=path!("/users/:slug") view=|| view! { "two" } />
                </Routes>
            </Router>
        }
    }

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
    fn static_ssr_is_rejected_after_a_response_was_selected() {
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
        assert!(matches!(
            result,
            Err(RegistrationError::UnsupportedStaticSsr(path))
                if path == "/static"
        ));
    }

    #[test]
    fn duplicate_generated_routes_are_rejected() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        );

        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                duplicate_route_app,
                None,
                || {},
            );
        assert!(matches!(
            result,
            Err(RegistrationError::DuplicateRoute(path))
                if path == "/duplicate"
        ));
    }

    #[test]
    fn semantically_duplicate_parameter_routes_are_rejected() {
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");

        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                semantic_duplicate_route_app,
                None,
                || {},
            );
        assert!(matches!(
            result,
            Err(RegistrationError::DuplicateRoute(path))
                if path == "/users/:slug"
        ));
    }

    #[test]
    fn route_collision_keys_ignore_parameter_names() {
        assert_eq!(
            parsed_route_collision_key("/users/:id/files/*"),
            parsed_route_collision_key("/users/:slug/files/*")
        );
        assert_ne!(
            parsed_route_collision_key("/users/static"),
            parsed_route_collision_key("/users/:id")
        );
        assert_eq!(
            parsed_route_collision_key("/users/:id/"),
            parsed_route_collision_key("users/:slug")
        );
        assert_eq!(
            parsed_route_collision_key("/"),
            parsed_route_collision_key("////")
        );
        assert_eq!(
            parsed_route_collision_key("/users//:id"),
            parsed_route_collision_key("/users/:slug")
        );
        assert_eq!(
            parsed_route_collision_key("/archive/:id.json"),
            parsed_route_collision_key("/archive/:slug.json")
        );
        assert_ne!(
            parsed_route_collision_key("/archive/:id.json"),
            parsed_route_collision_key("/archive/:slug.xml")
        );
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
    fn validated_route_lists_are_cached_by_application_type() {
        ROUTE_GENERATIONS.store(0, Ordering::Relaxed);
        for _ in 0..2 {
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
        }

        assert_eq!(ROUTE_GENERATIONS.load(Ordering::Relaxed), 1);
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

    #[test]
    fn html_with_zero_quality_is_not_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html;q=0"));
        assert!(!accepts_html(&headers));
    }

    fn accept(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn browser_navigation_accept_headers_are_html() {
        assert!(accepts_html(&accept(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        assert!(accepts_html(&accept("application/xhtml+xml")));
        assert!(accepts_html(&accept(" text/html ; charset=utf-8")));
    }

    #[test]
    fn non_navigation_accept_headers_are_not_html() {
        assert!(!accepts_html(&accept("application/json")));
        assert!(!accepts_html(&accept("*/*")));
        assert!(!accepts_html(&HeaderMap::new()));
        assert!(!accepts_html(&accept("text/html;q=0.000")));
    }

    #[test]
    fn accept_html_is_detected_across_repeated_headers() {
        let mut headers = HeaderMap::new();
        headers.append(ACCEPT, HeaderValue::from_static("application/json"));
        headers.append(ACCEPT, HeaderValue::from_static("text/html"));

        assert!(accepts_html(&headers));
    }

    fn sanitized(value: &'static str) -> Option<String> {
        sanitize_referrer(&HeaderValue::from_static(value))
            .map(|value| value.to_str().expect("ascii").to_owned())
    }

    #[test]
    fn referrers_are_reduced_to_same_origin_paths() {
        assert_eq!(
            sanitized("http://127.0.0.1/previous-page"),
            Some("/previous-page".to_owned())
        );
        assert_eq!(
            sanitized("https://malicious.example.com/steal?a=1"),
            Some("/steal?a=1".to_owned())
        );
        assert_eq!(
            sanitized("/relative/page"),
            Some("/relative/page".to_owned())
        );
    }

    #[test]
    fn protocol_relative_and_backslash_referrers_are_rejected() {
        assert_eq!(sanitized("//evil.example.com/path"), None);
        assert_eq!(sanitized("http://127.0.0.1/\\evil.example.com"), None);
        assert_eq!(sanitized("http://127.0.0.1/%5Cevil.example.com"), None);
        assert_eq!(sanitized("http://127.0.0.1/%5cevil.example.com"), None);
        assert_eq!(sanitized("mailto:someone@example.com"), None);
    }

    fn redirected(
        status: StatusCode,
        location: Option<&'static str>,
        accepts_html: bool,
        referrer: Option<&'static str>,
    ) -> (StatusCode, Option<String>) {
        let mut response =
            http::Response::new(Body::Sync(Bytes::from_static(b"body")));
        *response.status_mut() = status;
        if let Some(location) = location {
            response
                .headers_mut()
                .insert(LOCATION, HeaderValue::from_static(location));
        }
        apply_server_fn_redirect(
            &mut response,
            accepts_html,
            referrer.map(HeaderValue::from_static),
        );
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().expect("ascii").to_owned());
        (response.status(), location)
    }

    #[test]
    fn html_form_posts_redirect_back_to_a_same_origin_referrer() {
        assert_eq!(
            redirected(
                StatusCode::OK,
                None,
                true,
                Some("http://127.0.0.1/previous-page")
            ),
            (StatusCode::FOUND, Some("/previous-page".to_owned()))
        );
    }

    #[test]
    fn html_form_posts_fall_back_to_root_for_unusable_referrers() {
        assert_eq!(
            redirected(
                StatusCode::OK,
                None,
                true,
                Some("http://127.0.0.1/%5Cevil.example.com")
            ),
            (StatusCode::FOUND, Some("/".to_owned()))
        );
    }

    #[test]
    fn cross_origin_locations_are_reduced_to_their_path() {
        assert_eq!(
            redirected(
                StatusCode::FOUND,
                Some("https://evil.example.com/take-over"),
                false,
                None
            ),
            (StatusCode::FOUND, Some("/take-over".to_owned()))
        );
    }

    #[test]
    fn api_clients_keep_an_explicit_same_origin_location() {
        assert_eq!(
            redirected(StatusCode::OK, Some("/dashboard"), false, None),
            (StatusCode::OK, Some("/dashboard".to_owned()))
        );
    }

    #[test]
    fn responses_without_a_location_are_left_alone() {
        assert_eq!(
            redirected(StatusCode::OK, None, false, None),
            (StatusCode::OK, None)
        );
    }

    fn alpha_app() -> leptos::prelude::AnyView {
        use leptos::prelude::IntoAny;
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/alpha") view=|| view! { "alpha" } />
                </Routes>
            </Router>
        }
        .into_any()
    }

    fn beta_app() -> leptos::prelude::AnyView {
        use leptos::prelude::IntoAny;
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/beta") view=|| view! { "beta" } />
                </Routes>
            </Router>
        }
        .into_any()
    }

    fn generated_paths<AppFn>(app: AppFn) -> Vec<String>
    where
        AppFn: Fn() -> leptos::prelude::AnyView + 'static + Send + Clone,
    {
        let noop: fn() = || {};
        registered_routes(&app, &noop)
            .expect("route generation should succeed")
            .into_iter()
            .map(|(path, _, _)| path)
            .collect()
    }

    #[test]
    fn function_pointer_applications_do_not_share_a_cached_route_list() {
        type AppPointer = fn() -> leptos::prelude::AnyView;

        assert_eq!(generated_paths(alpha_app as AppPointer), ["/alpha"]);
        assert_eq!(generated_paths(beta_app as AppPointer), ["/beta"]);
    }
}
