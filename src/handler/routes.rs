//! Route-table discovery and validation.
//!
//! Shared by the request path and by [`validate_route_table`] so the rules a
//! deployment enforces and the rules a test suite checks cannot drift apart.

use std::{collections::BTreeSet, sync::Arc};

use http::Request;
use hydration_context::SsrSharedContext;
use leptos::{
    IntoView,
    prelude::{Owner, provide_context},
};
use leptos_meta::ServerMetaContext;
use leptos_router::{
    ExpandOptionals, PathSegment, RouteList, RouteListing, SsrMode,
};
use routefinder::{RouteSpec, Segment};

use super::http_util::provide_standard_contexts;
use super::policy::RegistrationError;
use crate::response::ResponseOptions;

/// A discovered route list, or the first registration error that rejects it.
type DiscoveredRoutes =
    Result<Vec<(String, RouteSpec, RouteListing)>, RegistrationError>;

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

/// Discovers an application's routes and rejects an unusable table.
///
/// Shared by the request path and by [`validate_route_table`] so the rules a
/// deployment enforces and the rules a test suite checks cannot drift apart.
pub(super) fn validated_route_table<IV, AppFn, ContextFn>(
    app_fn: &AppFn,
    excluded_routes: Option<&[String]>,
    discovery_context: &ContextFn,
) -> Result<Vec<(RouteSpec, RouteListing)>, RegistrationError>
where
    IV: IntoView + 'static,
    AppFn: Fn() -> IV + 'static + Send + Clone,
    ContextFn: Fn() + 'static + Send + Clone,
{
    let routes = registered_routes(app_fn, discovery_context)?;
    let mut registered_paths = BTreeSet::new();
    let mut validated = Vec::new();
    for (path, route_spec, listing) in routes {
        if excluded_routes.is_some_and(|excluded| excluded.contains(&path)) {
            continue;
        }
        let collision_key = route_collision_key(&route_spec);
        if !registered_paths.insert(collision_key) {
            return Err(RegistrationError::DuplicateRoute(path));
        }
        if matches!(listing.mode(), SsrMode::Static(_)) {
            return Err(RegistrationError::UnsupportedStaticSsr(path));
        }
        validated.push((route_spec, listing));
    }
    Ok(validated)
}

/// Checks an application's route table without serving a request.
///
/// Route discovery renders the whole application, so the request path runs it
/// only when a request will actually consult the SSR router: a server
/// function, a static asset, or an already-selected response resolves without
/// it and skips the work. A route table that is only ever reached through
/// those paths is therefore never validated in production.
///
/// Call this once from a test to close that gap. It applies exactly the rules
/// the request path applies - duplicate patterns, including ones that differ
/// only by parameter name, and unsupported [`SsrMode::Static`] routes.
///
/// ```no_run
/// # use leptos::prelude::*;
/// # fn app() -> AnyView { todo!() }
/// #[test]
/// fn route_table_is_valid() {
///     leptos_wasi::validate_route_table(app, None, || {})
///         .expect("route table should be valid");
/// }
/// ```
///
/// # Errors
///
/// Returns [`RegistrationError::DuplicateRoute`] for colliding patterns and
/// [`RegistrationError::UnsupportedStaticSsr`] for static-mode routes.
pub fn validate_route_table<IV, AppFn, ContextFn>(
    app: AppFn,
    excluded_routes: Option<Vec<String>>,
    discovery_context: ContextFn,
) -> Result<(), RegistrationError>
where
    IV: IntoView + 'static,
    AppFn: Fn() -> IV + 'static + Send + Clone,
    ContextFn: Fn() + 'static + Send + Clone,
{
    validated_route_table(&app, excluded_routes.as_deref(), &discovery_context)
        .map(|_| ())
}

pub(super) fn registered_routes<IV, AppFn, ContextFn>(
    app_fn: &AppFn,
    discovery_context: &ContextFn,
) -> DiscoveredRoutes
where
    IV: IntoView + 'static,
    AppFn: Fn() -> IV + 'static + Send + Clone,
    ContextFn: Fn() + 'static + Send + Clone,
{
    // Route discovery is deliberately uncached. Both supported hosts
    // (`wasmtime serve` and Spin) instantiate a fresh component per request,
    // so any in-guest cache starts empty on every lookup and is discarded with
    // the instance. The previous thread-local cache never returned a hit in
    // production; it only added a map allocation and a full deep clone of the
    // route list to each request. Measured on `/api/get_test`, discovery plus
    // registration accounts for roughly 183 us of a 1054 us request, none of
    // which the cache was removing.
    let generated: DiscoveredRoutes = {
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

    generated
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

#[cfg(test)]
mod tests {
    use leptos::prelude::view;
    use leptos_router::{
        components::{Route, Router, Routes},
        path,
    };
    use routefinder::RouteSpec;

    use bytes::Bytes;

    use super::super::HandlerCore;
    use super::super::policy::HandlerConfig;
    use super::super::test_support::static_route_app;
    use super::*;

    fn parsed_route_collision_key(
        path: &str,
    ) -> Result<Vec<RouteCollisionSegment>, String> {
        RouteSpec::try_from(path).map(|route| route_collision_key(&route))
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

    #[test]
    fn validate_route_table_rejects_what_the_request_path_would() {
        assert!(matches!(
            validate_route_table(static_route_app, None, || {}),
            Err(RegistrationError::UnsupportedStaticSsr(path))
                if path == "/static"
        ));
        assert!(matches!(
            validate_route_table(semantic_duplicate_route_app, None, || {}),
            Err(RegistrationError::DuplicateRoute(path))
                if path == "/users/:slug"
        ));
    }

    #[test]
    fn validate_route_table_accepts_a_sound_table() {
        assert!(validate_route_table(alpha_app, None, || {}).is_ok());
    }

    #[test]
    fn validate_route_table_honours_exclusions() {
        // An excluded route is not registered, so it must not be rejected
        // either - otherwise excluding a static-mode route would still fail.
        assert!(
            validate_route_table(
                static_route_app,
                Some(vec!["/static".to_owned()]),
                || {}
            )
            .is_ok()
        );
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
        );

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

    /// Two applications coerced to the same `fn()` type are indistinguishable
    /// by `TypeId`, which is what made the removed route cache unsound to key
    /// that way. Discovery is per-registration now, so each gets its own list;
    /// this keeps that guarantee pinned independently of how routes are built.
    #[test]
    fn function_pointer_applications_do_not_share_a_route_list() {
        type AppPointer = fn() -> leptos::prelude::AnyView;

        assert_eq!(generated_paths(alpha_app as AppPointer), ["/alpha"]);
        assert_eq!(generated_paths(beta_app as AppPointer), ["/beta"]);
    }
}
