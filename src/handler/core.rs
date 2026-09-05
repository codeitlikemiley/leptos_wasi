//! The request handler's registration phase.
//!
//! [`HandlerCore`] accumulates what a request needs - a matched server
//! function, a preset response, a static-asset result, or an SSR route table.
//! [`super::render`] turns that into a response.
//!
//! The fields are `pub(super)`, which is exactly the access sibling modules
//! had when all of this lived in one file.

#[cfg(feature = "tracing")]
use std::time::Instant;

use bytes::Bytes;
use http::{
    HeaderValue, Method, Request, StatusCode, Uri,
    header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE},
};
use leptos::IntoView;
use leptos_router::RouteListing;
use mime_guess::MimeGuess;
use routefinder::Router;
use server_fn::{
    Protocol, ServerFn, error::FromServerFnError, middleware::BoxedService,
};

use super::policy::{HandlerConfig, RegistrationError, plain_response};
use super::routes::validated_route_table;
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

pub(super) struct HandlerCore {
    pub(super) req: Request<Bytes>,
    pub(super) server_fn: Option<ServerFnHandler>,
    pub(super) preset_res: Option<Response>,
    pub(super) should_404: bool,
    pub(super) ssr_router: Router<RouteListing>,
    routes_registered: bool,
    config: HandlerConfig,
    #[cfg(feature = "tracing")]
    pub(super) request_started: Instant,
    #[cfg(feature = "tracing")]
    pub(super) trace_path: Option<String>,
    #[cfg(feature = "tracing")]
    pub(super) trace_route_class: Option<&'static str>,
}

impl HandlerCore {
    pub(super) fn new(req: Request<Bytes>, config: HandlerConfig) -> Self {
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
    pub(super) fn with_request_started(mut self, started: Instant) -> Self {
        self.request_started = started;
        self
    }

    pub(super) fn with_preset(
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
        self.server_fn.is_some() || self.preset_res.is_some() || self.should_404
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
            self.server_fn = Some(Box::new(move |request| {
                Box::pin(async move {
                    let (parts, bytes) = request.into_parts();
                    if bytes.len() > limit {
                        return plain_response(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "request body too large",
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
        let Ok(decoded) = crate::static_files::normalize_static_path(raw)
        else {
            self.should_404 = true;
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

        for (route_spec, listing) in
            validated_route_table(&app_fn, excluded_routes, &discovery_context)?
        {
            match self.ssr_router.add(route_spec, listing) {
                Ok(()) => {}
                Err(infallible) => match infallible {},
            }
        }
        self.routes_registered = true;
        Ok(self)
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
}
