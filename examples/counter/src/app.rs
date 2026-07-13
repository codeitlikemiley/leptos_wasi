use components::{Route, Router, Routes};
use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::*;

#[cfg(feature = "ssr")]
#[path = "counter_store.rs"]
mod counter_store;

#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() islands=true root="" />
                <MetaTags />
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
        <Stylesheet id="leptos" href="/pkg/counter.css" />
        <Meta
            name="description"
            content="A website running its server-side as a WASIp3 Component :D"
        />

        <Title text="Welcome to Leptos X WASIp3!" />

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
        <div class="min-h-screen bg-[#1a2332] flex flex-col lg:flex-row items-center justify-center p-4 gap-6">
            <section class="bg-[#263343] rounded-xl shadow-2xl p-8 md:p-12 max-w-md w-full border border-[#3a4a5c]">
                <header class="text-center space-y-2 mb-8">
                    <div class="flex items-center justify-center gap-3 mb-4">
                        <div class="w-10 h-10 bg-[#00d4aa] rounded-lg flex items-center justify-center">
                            <span class="text-[#1a2332] font-bold text-xl">L</span>
                        </div>
                        <h1 class="text-3xl md:text-4xl font-medium text-white">
                            "Counter"
                        </h1>
                    </div>
                    <p class="text-[#8b9cb8] text-sm">
                        "Server-rendered by Leptos on a WASI Preview 3 component."
                    </p>
                </header>

                <CounterIsland />

                <footer class="pt-6 mt-8 border-t border-[#3a4a5c] text-center">
                    <p class="text-[#8b9cb8] text-xs">
                        "The page shell stays on the server; only this counter hydrates."
                    </p>
                </footer>
            </section>

            <aside class="bg-[#202c3b] rounded-xl p-6 max-w-sm w-full border border-[#3a4a5c] text-slate-200 space-y-4">
                <p class="text-[#00d4aa] text-xs font-semibold uppercase tracking-widest">
                    "Islands + WASM splitting"
                </p>
                <h2 class="text-xl font-medium text-white">
                    "One server component, one lazy browser island"
                </h2>
                <ul class="space-y-3 text-sm text-[#a9b7cc]">
                    <li>"• App, routing, layout, and copy render only on the server."</li>
                    <li>"• Counter interactivity is compiled behind #[island(lazy)]."</li>
                    <li>"• Spin and Wasmtime serve the same generated split assets."</li>
                </ul>
            </aside>
        </div>
    }
}

#[island(lazy)]
fn CounterIsland() -> impl IntoView {
    let load_action = ServerAction::<GetCount>::new();
    let increment_action = ServerAction::<IncrementCount>::new();
    let (count, set_count) = signal(0_u32);
    let (load_finished, set_load_finished) = signal(false);
    let (operation_id, set_operation_id) = signal(None::<String>);
    let load_value = load_action.value();
    let action_value = increment_action.value();

    Effect::new(move |_| {
        load_action.dispatch(GetCount {});
    });

    Effect::new(move |_| {
        load_value.with(|result| {
            if let Some(Ok(stored_count)) = result {
                set_count.set(*stored_count);
            }
            if result.is_some() {
                set_load_finished.set(true);
            }
        });
    });

    Effect::new(move |_| {
        action_value.with(|result| {
            if let Some(Ok(next_count)) = result {
                set_count.set(*next_count);
                set_operation_id.set(None);
            }
        });
    });

    let increment = move |_| {
        let operation_id = operation_id
            .get_untracked()
            .unwrap_or_else(new_operation_id);
        set_operation_id.set(Some(operation_id.clone()));
        increment_action.dispatch(IncrementCount {
            current: count.get_untracked(),
            operation_id,
        });
    };

    let action_failed = move || {
        load_action
            .value()
            .with(|result| matches!(result, Some(Err(_))))
            || increment_action
                .value()
                .with(|result| matches!(result, Some(Err(_))))
    };

    let pending =
        move || load_action.pending().get() || increment_action.pending().get();

    view! {
        <div class="text-center space-y-8">
            <div class="relative">
                <div class="bg-[#1a2332] rounded-lg p-8 border border-[#3a4a5c]">
                    <div class="text-5xl md:text-6xl font-light text-white tabular-nums">
                        {move || count.get()}
                    </div>
                    <div class="text-[#8b9cb8] text-sm mt-2 uppercase tracking-wider">
                        "COUNT VALUE"
                    </div>
                </div>

                <Show when=pending>
                    <div class="absolute inset-0 flex items-center justify-center bg-[#1a2332]/50 rounded-lg">
                        <div class="animate-spin rounded-full h-8 w-8 border-2 border-transparent border-t-[#00d4aa]"></div>
                    </div>
                </Show>
            </div>

            <button
                type="button"
                on:click=increment
                disabled=pending
                class="w-full rounded-lg bg-[#00d4aa] px-6 py-3 text-[#1a2332] font-medium transition-all duration-200 hover:bg-[#00b894] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-[#00d4aa]"
            >
                {move || if increment_action.pending().get() {
                    "Updating..."
                } else {
                    "Increment Counter"
                }}
            </button>

            <div class="flex items-center justify-center gap-2 text-xs">
                <div class={move || {
                    if pending() {
                        "w-2 h-2 rounded-full bg-[#00d4aa] animate-pulse"
                    } else if action_failed() {
                        "w-2 h-2 rounded-full bg-red-500"
                    } else {
                        "w-2 h-2 rounded-full bg-[#00d4aa]"
                    }
                }}>
                </div>
                <span
                    data-testid="counter-status"
                    class="text-[#8b9cb8] uppercase tracking-wider"
                >
                    {move || {
                        if !load_finished.get() || load_action.pending().get() {
                            "Loading saved count"
                        } else if increment_action.pending().get() {
                            "Saving"
                        } else if action_failed() {
                            "Update failed"
                        } else {
                            "Ready"
                        }
                    }}
                </span>
            </div>
        </div>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    {
        if let Some(resp) =
            use_context::<leptos_wasi::response::ResponseOptions>()
        {
            resp.set_status(leptos_wasi::prelude::StatusCode::NOT_FOUND);
        }
    }

    view! { <h1>"Not Found"</h1> }
}

#[server(prefix = "/api", endpoint = "get_count")]
pub async fn get_count() -> Result<u32, ServerFnError<String>> {
    #[cfg(feature = "ssr")]
    {
        if let Some(value) = counter_store::read_configured_count()
            .await
            .map_err(counter_store_error)?
        {
            return Ok(value);
        }
    }
    Ok(0)
}

#[server(prefix = "/api", endpoint = "increment_count")]
pub async fn increment_count(
    current: u32,
    operation_id: String,
) -> Result<u32, ServerFnError<String>> {
    #[cfg(feature = "ssr")]
    {
        if let Some(value) =
            counter_store::increment_configured_count(&operation_id)
                .await
                .map_err(counter_store_error)?
        {
            return Ok(value);
        }
    }
    current.checked_add(1).ok_or_else(|| {
        ServerFnError::ServerError("counter reached its maximum value".into())
    })
}

fn new_operation_id() -> String {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        return uuid::Uuid::new_v4().simple().to_string();
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        String::new()
    }
}

#[cfg(feature = "ssr")]
fn counter_store_error(
    error: counter_store::CounterStoreError,
) -> ServerFnError<String> {
    eprintln!("counter persistence error: {error}");
    if let Some(response) =
        use_context::<leptos_wasi::response::ResponseOptions>()
    {
        response.set_status(http::StatusCode::SERVICE_UNAVAILABLE);
        response.insert_header(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
    }
    ServerFnError::ServerError("counter store unavailable".into())
}
