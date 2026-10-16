use std::net::SocketAddr;

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
use server_fn::codec::{GetUrl, Json};

/// Shared Leptos options for both backends. Hydration is out of scope so
/// `output_name` / `site_root` only need to be stable, not cargo-leptos
/// generated.
pub fn leptos_options(site_addr: SocketAddr) -> LeptosOptions {
    LeptosOptions::builder()
        .output_name("compare_app")
        .site_root("public")
        .site_addr(site_addr)
        .env(leptos::config::Env::PROD)
        .build()
}

/// HTML shell used by both the WASI handler and the Axum integration.
pub fn shell(_options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

/// Application routes shared by both backends.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let fallback = || view! { "Page not found." }.into_view();

    view! {
        <Title text="leptos_wasi vs axum" />
        <Router>
            <main>
                <Routes fallback>
                    <Route path=path!("") view=HomePage />
                    <Route path=path!("/*any") view=NotFound />
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn HomePage() -> impl IntoView {
    view! {
        <h1>"leptos_wasi vs axum"</h1>
        <p id="ssr-marker">"simple SSR page"</p>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! { <h1>"Not Found"</h1> }
}

/// Soak-style GET server function. Hits `/api/get_test`.
#[server(input = GetUrl, prefix = "/api", endpoint = "get_test")]
pub async fn get_test() -> Result<String, ServerFnError> {
    Ok("GET response".to_string())
}

/// JSON POST server function. Hits `/api/post_test`.
#[server(input = Json, prefix = "/api", endpoint = "post_test")]
pub async fn post_test(msg: String) -> Result<String, ServerFnError> {
    Ok(format!("POST response: {msg}"))
}

/// Form-urlencoded POST server function. Hits `/api/form_test`.
#[server(prefix = "/api", endpoint = "form_test")]
pub async fn form_test(msg: String) -> Result<String, ServerFnError> {
    Ok(format!("FORM response: {msg}"))
}
