//! Route discovery, the largest in-guest cost of a request that renders.
//!
//! `PERFORMANCE.md` measures discovery plus registration at roughly 183 us of
//! a 1054 us request on `/api/get_test` - about 17%, and the reason
//! `HandlerCore::generate_routes_with_exclusions_and_discovery_context`
//! shortcuts it for requests that never consult the SSR router.
//!
//! Most of that time is spent inside `leptos_router::RouteList::generate`,
//! which this crate does not own. A leptos bump that regressed discovery by
//! 40% would move an end-to-end request by about 7% - inside the soak job's
//! 8-12% regression budget, so nothing in CI would see it. That is the gap
//! this benchmark exists to cover.
//!
//! Deliberately run by hand (`cargo make bench`), never in push/PR CI. The
//! soak job's own notes record a +/-0.7pp measurement problem on shared
//! runners over a 600-second load test; a microsecond-scale sample has no
//! chance there.
//!
//! The sweep over route counts is the point. A flat line means discovery is
//! dominated by fixed cost - arena setup, `SsrSharedContext`, one full
//! application render - and a rising one means per-route work
//! (`RouteSpec::try_from`, `route_collision_key`, the collision `BTreeSet`)
//! is what matters. Those two answers imply different fixes, and neither is
//! visible from a single number.

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
use leptos_wasi::{RouteTable, validate_route_table};

/// Renders `N` routes, including one parameterised and one optional-segment
/// route so `expand_optionals` runs and one declared route becomes several.
macro_rules! route_app {
    ($name:ident, $($extra:tt)*) => {
        fn $name() -> impl IntoView {
            view! {
                <Router>
                    <Routes fallback=|| view! { "not found" }>
                        <Route path=path!("/") view=|| view! { "home" }/>
                        <Route path=path!("/users/:id") view=|| view! { "user" }/>
                        <Route path=path!("/docs/:page?") view=|| view! { "doc" }/>
                        $($extra)*
                    </Routes>
                </Router>
            }
        }
    };
}

route_app!(app_3,);

route_app!(
    app_8,
    <Route path=path!("/a") view=|| view! { "a" }/>
    <Route path=path!("/b") view=|| view! { "b" }/>
    <Route path=path!("/c") view=|| view! { "c" }/>
    <Route path=path!("/d") view=|| view! { "d" }/>
    <Route path=path!("/e") view=|| view! { "e" }/>
);

route_app!(
    app_32,
    <Route path=path!("/a00") view=|| view! { "x" }/>
    <Route path=path!("/a01") view=|| view! { "x" }/>
    <Route path=path!("/a02") view=|| view! { "x" }/>
    <Route path=path!("/a03") view=|| view! { "x" }/>
    <Route path=path!("/a04") view=|| view! { "x" }/>
    <Route path=path!("/a05") view=|| view! { "x" }/>
    <Route path=path!("/a06") view=|| view! { "x" }/>
    <Route path=path!("/a07") view=|| view! { "x" }/>
    <Route path=path!("/a08") view=|| view! { "x" }/>
    <Route path=path!("/a09") view=|| view! { "x" }/>
    <Route path=path!("/a10") view=|| view! { "x" }/>
    <Route path=path!("/a11") view=|| view! { "x" }/>
    <Route path=path!("/a12") view=|| view! { "x" }/>
    <Route path=path!("/a13") view=|| view! { "x" }/>
    <Route path=path!("/a14") view=|| view! { "x" }/>
    <Route path=path!("/a15") view=|| view! { "x" }/>
    <Route path=path!("/a16") view=|| view! { "x" }/>
    <Route path=path!("/a17") view=|| view! { "x" }/>
    <Route path=path!("/a18") view=|| view! { "x" }/>
    <Route path=path!("/a19") view=|| view! { "x" }/>
    <Route path=path!("/a20") view=|| view! { "x" }/>
    <Route path=path!("/a21") view=|| view! { "x" }/>
    <Route path=path!("/a22") view=|| view! { "x" }/>
    <Route path=path!("/a23") view=|| view! { "x" }/>
    <Route path=path!("/a24") view=|| view! { "x" }/>
    <Route path=path!("/a25") view=|| view! { "x" }/>
    <Route path=path!("/a26") view=|| view! { "x" }/>
    <Route path=path!("/a27") view=|| view! { "x" }/>
    <Route path=path!("/a28") view=|| view! { "x" }/>
);

/// Discovers and validates the whole route table.
///
/// `validate_route_table` is the public entry point onto exactly the path a
/// request takes: `validated_route_table` -> `registered_routes` -> a full
/// application render, then `route_collision_key` and the duplicate check per
/// route. Only a `routefinder` insert loop is missing, which cannot matter
/// next to rendering the application.
#[divan::bench(args = [3, 8, 32])]
fn discover(bencher: divan::Bencher, routes: usize) {
    bencher.bench(|| {
        let result = match routes {
            3 => validate_route_table(app_3, None, || {}),
            8 => validate_route_table(app_8, None, || {}),
            _ => validate_route_table(app_32, None, || {}),
        };
        divan::black_box(result).expect("bench route table should be valid");
    });
}

/// Same work as [`discover`], through the public [`RouteTable`] constructor.
#[divan::bench(args = [3, 8, 32])]
fn discover_table(bencher: divan::Bencher, routes: usize) {
    bencher.bench(|| {
        let result = match routes {
            3 => RouteTable::discover(app_3, None, || {}),
            8 => RouteTable::discover(app_8, None, || {}),
            _ => RouteTable::discover(app_32, None, || {}),
        };
        divan::black_box(result).expect("bench route table should be valid");
    });
}

/// Per-request install once the instance has already discovered.
///
/// `RouteTable` is `!Send` (`Rc`), so the table lives in `thread_local!` — the
/// same pattern the public API documents. The measured work is an `Rc` clone.
#[divan::bench(args = [3, 8, 32])]
fn install_from_table(bencher: divan::Bencher, routes: usize) {
    bencher.bench(|| {
        thread_local! {
            static TABLE3: RouteTable =
                RouteTable::discover(app_3, None, || {}).expect("valid");
            static TABLE8: RouteTable =
                RouteTable::discover(app_8, None, || {}).expect("valid");
            static TABLE32: RouteTable =
                RouteTable::discover(app_32, None, || {}).expect("valid");
        }
        let cloned = match routes {
            3 => TABLE3.with(Clone::clone),
            8 => TABLE8.with(Clone::clone),
            _ => TABLE32.with(Clone::clone),
        };
        divan::black_box(cloned);
    });
}

fn main() {
    divan::main();
}
