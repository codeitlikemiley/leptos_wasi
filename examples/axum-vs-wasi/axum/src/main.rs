use std::net::SocketAddr;

use axum::Router;
use compare_app::{App, leptos_options, shell};
use leptos_axum::{LeptosRoutes, generate_route_list};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::var("LEPTOS_SITE_ADDR")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 3001)));

    let leptos_options = leptos_options(addr);
    let routes = generate_route_list(App);
    let public_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../public");

    let app = Router::new()
        .nest_service("/static", ServeDir::new(public_dir))
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .with_state(leptos_options);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {addr}: {error}"));
    println!("compare-axum listening on http://{addr}");
    // Without TCP_NODELAY the static handler's separate header and body
    // writes stall on the client's delayed ACK (about 40 ms per request).
    let listener = axum::serve::ListenerExt::tap_io(listener, |stream| {
        if let Err(error) = stream.set_nodelay(true) {
            eprintln!("failed to set TCP_NODELAY: {error}");
        }
    });
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|error| panic!("server error: {error}"));
}
