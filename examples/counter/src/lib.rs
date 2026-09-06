#![recursion_limit = "256"]

mod app;

#[cfg(feature = "ssr")]
mod server;

/// This is the entrypoint called by the JS "igniter" script.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_islands();
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::app::App;

    #[test]
    fn route_table_is_valid() {
        leptos_wasi::validate_route_table(App, None, || {})
            .expect("route table should be valid");
    }
}
