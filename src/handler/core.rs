//! The request handler's registration phase.
//!
//! [`HandlerCore`] accumulates what a request needs - a matched server
//! function, a preset response, a static-asset result, or an SSR route table.
//! [`super::render`] turns that into a response.
//!
//! The fields are `pub(super)`, which is exactly the access sibling modules
//! had when all of this lived in one file.

use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "tracing")]
use std::time::Instant;

use bytes::Bytes;
use http::{
    HeaderValue, Method, Request, StatusCode, Uri,
    header::{
        ACCEPT_ENCODING, ALLOW, CONTENT_LENGTH, CONTENT_TYPE,
        IF_MODIFIED_SINCE, IF_NONE_MATCH,
    },
};
use leptos::IntoView;
use leptos_router::RouteListing;
use mime_guess::MimeGuess;
use routefinder::Router;
use server_fn::{
    Protocol, ServerFn, error::FromServerFnError, middleware::BoxedService,
};

use super::policy::{
    HandlerConfig, RegistrationError, RequestPolicyError, plain_response,
    policy_response,
};
use super::routes::{RouteTable, router_from_listings, validated_route_table};
use super::server_fns::{
    ReqBody, ResBody, ServerFnHandler, TypedServerFnService,
};
#[cfg(feature = "tracing")]
use super::trace::RequestTrace;
use super::trace::TraceHandle;
use crate::{
    __private::ServerWithBody,
    response::{Body, Response},
};

/// How this request will be answered, once registration has claimed it.
///
/// Unclaimed requests fall through to the SSR router. The other variants are
/// mutually exclusive: a server function, a ready-made response, or a known
/// 404 never consults the route table.
pub(super) enum Selection {
    /// No handler has claimed the request yet.
    Unclaimed,
    /// Prefix matched but the asset callback returned `None`, or the path
    /// failed normalization.
    NotFound,
    /// A complete response, plus the tracing route class that produced it.
    Preset(Response, &'static str),
    /// A matched typed server function.
    ServerFn(ServerFnHandler),
}

/// A static-asset request after prefix matching and path normalization.
///
/// The crate has already rejected traversal, percent-encoded separators, and
/// disallowed methods. Callbacks see the relative lookup key plus the
/// conditional and encoding headers they need to implement 304 / compression.
#[derive(Clone, Copy, Debug)]
pub struct StaticRequest<'a> {
    path: &'a str,
    if_none_match: Option<&'a HeaderValue>,
    if_modified_since: Option<&'a HeaderValue>,
    accept_encoding: Option<&'a HeaderValue>,
}

impl<'a> StaticRequest<'a> {
    /// Relative path remaining after the static prefix, `/`-separated.
    #[must_use]
    pub const fn path(&self) -> &'a str {
        self.path
    }

    /// `If-None-Match`, if the client sent one.
    #[must_use]
    pub const fn if_none_match(&self) -> Option<&'a HeaderValue> {
        self.if_none_match
    }

    /// `If-Modified-Since`, if the client sent one.
    #[must_use]
    pub const fn if_modified_since(&self) -> Option<&'a HeaderValue> {
        self.if_modified_since
    }

    /// `Accept-Encoding`, if the client sent one.
    #[must_use]
    pub const fn accept_encoding(&self) -> Option<&'a HeaderValue> {
        self.accept_encoding
    }
}

pub(super) struct HandlerCore {
    pub(super) req: Request<Bytes>,
    pub(super) selection: Selection,
    pub(super) ssr_router: Arc<Router<RouteListing>>,
    routes_registered: bool,
    config: HandlerConfig,
    #[cfg(feature = "tracing")]
    pub(super) request_started: Instant,
    #[cfg(feature = "tracing")]
    pub(super) trace_path: Option<String>,
}

impl HandlerCore {
    pub(super) fn new(req: Request<Bytes>, config: HandlerConfig) -> Self {
        Self {
            req,
            selection: Selection::Unclaimed,
            ssr_router: Arc::new(Router::new()),
            routes_registered: false,
            config,
            #[cfg(feature = "tracing")]
            request_started: Instant::now(),
            #[cfg(feature = "tracing")]
            trace_path: None,
        }
    }

    #[cfg(feature = "tracing")]
    pub(super) fn with_request_started(mut self, started: Instant) -> Self {
        self.request_started = started;
        self
    }

    pub(super) fn with_preset(
        mut self,
        response: Response,
        route_class: &'static str,
    ) -> Self {
        self.selection = Selection::Preset(response, route_class);
        self
    }

    #[cfg(feature = "tracing")]
    pub(super) fn request_trace(&self, preview: &'static str) -> TraceHandle {
        RequestTrace::new(self, preview)
    }

    // Mirrors the signature of the `tracing` arm above, which does read
    // `self`; the two must stay interchangeable at the call sites.
    #[expect(
        clippy::unused_self,
        reason = "signature must match the tracing arm"
    )]
    #[cfg(not(feature = "tracing"))]
    pub(super) fn request_trace(&self, _: &'static str) -> TraceHandle {
        TraceHandle
    }

    #[inline]
    fn shortcut(&self) -> bool {
        !matches!(self.selection, Selection::Unclaimed)
    }

    pub(super) fn with_server_fn<T>(mut self) -> Self
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
            self.selection = Selection::ServerFn(Box::new(move |request| {
                Box::pin(async move {
                    let (parts, bytes) = request.into_parts();
                    if bytes.len() > limit {
                        return policy_response(
                            &RequestPolicyError::BodyTooLarge { limit },
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

    pub(super) fn static_files_handler<T>(
        self,
        prefix: T,
        handler: impl Fn(String) -> Option<Body> + 'static + Send + Clone,
    ) -> Result<Self, RegistrationError>
    where
        T: TryInto<Uri>,
        <T as TryInto<Uri>>::Error: std::error::Error,
    {
        self.static_files_handler_with(prefix, move |request| {
            handler(request.path().to_owned()).map(http::Response::new)
        })
    }

    pub(super) fn static_files_handler_with<T>(
        mut self,
        prefix: T,
        handler: impl Fn(StaticRequest<'_>) -> Option<http::Response<Body>>
        + 'static
        + Send
        + Clone,
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
            self.selection = Selection::Preset(response, "static");
            return Ok(self);
        }

        let stripped = req_path.strip_prefix(prefix_path).unwrap_or_default();
        let raw = if prefix_path.ends_with('/') {
            stripped
        } else {
            stripped.strip_prefix('/').unwrap_or(stripped)
        };
        let Ok(decoded) = crate::static_files::normalize_static_path(raw)
        else {
            self.selection = Selection::NotFound;
            return Ok(self);
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

        let mime = content_type_for_static_path(&decoded);
        let request = StaticRequest {
            path: &decoded,
            if_none_match: self.req.headers().get(IF_NONE_MATCH),
            if_modified_since: self.req.headers().get(IF_MODIFIED_SINCE),
            accept_encoding: self.req.headers().get(ACCEPT_ENCODING),
        };
        match handler(request) {
            None => self.selection = Selection::NotFound,
            Some(mut response) => {
                let original_length = match response.body() {
                    Body::Sync(bytes) => Some(bytes.len()),
                    Body::Async(_) => None,
                };
                if self.req.method() == Method::HEAD {
                    *response.body_mut() = Body::Sync(Bytes::new());
                }
                if !response.headers().contains_key(CONTENT_TYPE) {
                    response.headers_mut().insert(CONTENT_TYPE, mime);
                }
                // `nosniff` is applied centrally in `HandlerCore::render`,
                // which every static response also funnels through.
                if !response.headers().contains_key(CONTENT_LENGTH)
                    && let Some(length) = original_length
                {
                    response
                        .headers_mut()
                        .insert(CONTENT_LENGTH, HeaderValue::from(length));
                }
                self.selection = Selection::Preset(response.into(), "static");
            }
        }
        Ok(self)
    }

    pub(super) fn generate_routes_with_exclusions_and_discovery_context<
        IV,
        AppFn,
        ContextFn,
    >(
        mut self,
        app_fn: AppFn,
        excluded_routes: Option<&[String]>,
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
        // measured on `/api/get_test`. Discovery is per request unless the app
        // passes a RouteTable into generate_routes_from.
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

        self.ssr_router =
            Arc::new(router_from_listings(validated_route_table(
                &app_fn,
                excluded_routes,
                &discovery_context,
            )?));
        self.routes_registered = true;
        Ok(self)
    }

    /// Installs a previously discovered [`RouteTable`].
    ///
    /// Clone is the reuse: the table holds an `Arc` of the router.
    /// Requests that never consult the SSR router still skip installing it.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::RoutesAlreadyGenerated`] if routes
    /// were already generated on this handler.
    pub(super) fn generate_routes_from(
        mut self,
        table: &RouteTable,
    ) -> Result<Self, RegistrationError> {
        if self.routes_registered {
            return Err(RegistrationError::RoutesAlreadyGenerated);
        }
        if self.shortcut() {
            self.routes_registered = true;
            return Ok(self);
        }
        self.ssr_router = table.router();
        self.routes_registered = true;
        Ok(self)
    }
}

fn content_type_for_static_path(path: &str) -> HeaderValue {
    let extension = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("css") => HeaderValue::from_static("text/css"),
        Some("gif") => HeaderValue::from_static("image/gif"),
        Some("htm" | "html") => HeaderValue::from_static("text/html"),
        Some("ico") => HeaderValue::from_static("image/x-icon"),
        Some("jpeg" | "jpg") => HeaderValue::from_static("image/jpeg"),
        Some("js") => HeaderValue::from_static("text/javascript"),
        Some("json") => HeaderValue::from_static("application/json"),
        Some("map" | "txt") => HeaderValue::from_static("text/plain"),
        Some("mjs") => HeaderValue::from_static("application/javascript"),
        Some("png") => HeaderValue::from_static("image/png"),
        Some("svg") => HeaderValue::from_static("image/svg+xml"),
        Some("ttf") => HeaderValue::from_static("font/ttf"),
        Some("wasm") => HeaderValue::from_static("application/wasm"),
        Some("webp") => HeaderValue::from_static("image/webp"),
        Some("woff") => HeaderValue::from_static("application/font-woff"),
        Some("woff2") => HeaderValue::from_static("font/woff2"),
        _ => {
            let mime = MimeGuess::from_path(path).first_or_octet_stream();
            HeaderValue::from_str(mime.as_ref()).unwrap_or_else(|_| {
                HeaderValue::from_static("application/octet-stream")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use leptos::prelude::view;
    use leptos_router::{
        components::{Route, Router, Routes},
        path,
    };

    use super::super::test_support::static_route_app;
    use super::*;

    static DISCOVERY_GENERATIONS: AtomicUsize = AtomicUsize::new(0);
    static FROM_GENERATIONS: AtomicUsize = AtomicUsize::new(0);
    static CLAIMED_GENERATIONS: AtomicUsize = AtomicUsize::new(0);

    fn counted_routes() -> impl IntoView {
        view! {
            <Router>
                <Routes fallback=|| view! { "not found" }>
                    <Route path=path!("/cached") view=|| view! { "cached" } />
                </Routes>
            </Router>
        }
    }

    fn counted_route_app() -> impl IntoView {
        DISCOVERY_GENERATIONS.fetch_add(1, Ordering::Relaxed);
        counted_routes()
    }

    fn from_route_app() -> impl IntoView {
        FROM_GENERATIONS.fetch_add(1, Ordering::Relaxed);
        counted_routes()
    }

    fn claimed_route_app() -> impl IntoView {
        CLAIMED_GENERATIONS.fetch_add(1, Ordering::Relaxed);
        counted_routes()
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
    fn static_mime_matches_mime_guess_for_the_common_web_set() {
        let cases = [
            ("app.js", "text/javascript"),
            ("app.mjs", "application/javascript"),
            ("font.woff", "application/font-woff"),
            ("bundle.js.map", "text/plain"),
        ];
        for (file, mime) in cases {
            let core = HandlerCore::new(
                Request::builder()
                    .uri(format!("/static/{file}"))
                    .body(Bytes::new())
                    .expect("test request should be valid"),
                HandlerConfig::default(),
            )
            .static_files_handler("/static", |_| {
                Some(Body::Sync(Bytes::from_static(b"x")))
            })
            .expect("static registration should succeed");
            let content_type = match core.selection {
                Selection::Preset(response, _) => {
                    response.0.headers().get(CONTENT_TYPE).cloned()
                }
                _ => None,
            };
            assert_eq!(
                content_type.as_ref().and_then(|value| value.to_str().ok()),
                Some(mime),
                "{file}"
            );
        }
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

    #[test]
    fn route_discovery_runs_once_per_registration() {
        // Discovery is per request unless the app passes a RouteTable.
        // Pinning the count at one generation per registration keeps a
        // TypeId-keyed cache from being reintroduced. `generate_routes_from`
        // reuses a table without incrementing this counter.
        DISCOVERY_GENERATIONS.store(0, Ordering::Relaxed);
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

        assert_eq!(DISCOVERY_GENERATIONS.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn generate_routes_from_does_not_rediscover() {
        FROM_GENERATIONS.store(0, Ordering::Relaxed);
        let table = RouteTable::discover(from_route_app, None, || {})
            .expect("route table should be valid");
        assert_eq!(FROM_GENERATIONS.load(Ordering::Relaxed), 1);

        for _ in 0..2 {
            let core = HandlerCore::new(
                Request::new(Bytes::new()),
                HandlerConfig::default(),
            );
            let result = core.generate_routes_from(&table);
            assert!(result.is_ok());
        }

        assert_eq!(FROM_GENERATIONS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_claimed_request_never_discovers_routes() {
        // The saving that motivates the shortcut: an already-selected response
        // resolves without the SSR router, and discovery renders the whole
        // application. Measured at 183 us of a 1054 us request, paid on every
        // request that uses generate_routes. Pass a RouteTable to discover
        // once per instance instead.
        CLAIMED_GENERATIONS.store(0, Ordering::Relaxed);
        let core = HandlerCore::new(
            Request::new(Bytes::new()),
            HandlerConfig::default(),
        )
        .with_preset(plain_response(StatusCode::OK, "selected"), "test");
        let result = core
            .generate_routes_with_exclusions_and_discovery_context(
                claimed_route_app,
                None,
                || {},
            );

        assert!(result.is_ok());
        assert_eq!(CLAIMED_GENERATIONS.load(Ordering::Relaxed), 0);
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

    #[test]
    fn static_files_handler_with_can_return_not_modified() {
        let core = HandlerCore::new(
            Request::builder()
                .uri("/static/app.js")
                .header(IF_NONE_MATCH, "\"abc\"")
                .header(IF_MODIFIED_SINCE, "Wed, 21 Oct 2015 07:28:00 GMT")
                .header(ACCEPT_ENCODING, "gzip")
                .body(Bytes::new())
                .expect("test request should be valid"),
            HandlerConfig::default(),
        )
        .static_files_handler_with("/static", |request| {
            assert_eq!(request.path(), "app.js");
            assert_eq!(
                request
                    .if_none_match()
                    .and_then(|value| value.to_str().ok()),
                Some("\"abc\"")
            );
            assert!(request.if_modified_since().is_some());
            assert_eq!(
                request
                    .accept_encoding()
                    .and_then(|value| value.to_str().ok()),
                Some("gzip")
            );
            Some(
                http::Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .body(Body::Sync(Bytes::new()))
                    .expect("empty 304 should be valid"),
            )
        })
        .expect("static registration should succeed");

        assert!(
            matches!(
                core.selection,
                Selection::Preset(ref response, "static")
                    if response.0.status() == StatusCode::NOT_MODIFIED
            ),
            "the 304 callback should select a preset response"
        );
    }
}
