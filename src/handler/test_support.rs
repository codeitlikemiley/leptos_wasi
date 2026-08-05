//! Fixtures shared by more than one module's test suite.
//!
//! Keep this small. A test-support module that accumulates every helper is
//! how a file grows back to the size this split was undoing.

use leptos::{IntoView, prelude::view};
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    path,
    static_routes::StaticRoute,
};

pub(super) fn static_route_app() -> impl IntoView {
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
