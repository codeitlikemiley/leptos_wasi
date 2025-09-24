use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use crate::pages::home::Home;

#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <MetaTags />
                <link rel="icon" type="image/svg+xml" href="/public/favicon.svg" />
                <Stylesheet id="leptos" href="/public/pkg/counter.css" />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let fallback = || view! { "Page not found." }.into_view();

    view! {
        <Meta name="charset" content="UTF-8" />
        <Meta
            name="description"
            content="A Leptos application running as a WASI Component"
        />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Meta name="theme-color" content="white" />

        <Title text="Welcome to Counter!" />

        <Router>
            <main>
                <Routes fallback>
                    <Route path=path!("/") view=Home />
                </Routes>
            </main>
        </Router>
    }
}