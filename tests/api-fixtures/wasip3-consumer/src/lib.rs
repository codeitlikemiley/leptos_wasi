#![deny(warnings)]

use leptos::prelude::*;
use server_fn::codec::GetUrl;

use leptos_wasi::{
    ExecutorError,
    wasip3::prelude::{
        Handler, HandlerConfig, HandlerError, IncomingRequest,
        RegistrationError, init_wasip3_spawner,
    },
};

pub fn initialize_spawner() -> Result<(), ExecutorError> {
    init_wasip3_spawner()
}

pub fn convert_request(
    request: IncomingRequest,
) -> Result<
    http::Request<::wasip3::http_compat::IncomingRequestBody>,
    ::wasip3::http::types::ErrorCode,
> {
    ::wasip3::http_compat::http_from_wasi_request(request)
}

pub async fn build_default(
    request: http::Request<::wasip3::http_compat::IncomingRequestBody>,
) -> Result<Handler, HandlerError> {
    Handler::build(request).await
}

pub async fn build_configured(
    request: http::Request<::wasip3::http_compat::IncomingRequestBody>,
    config: HandlerConfig,
) -> Result<Handler, HandlerError> {
    Handler::build_with_config(request, config).await
}

fn app() -> impl IntoView {
    view! { <p>"consumer"</p> }
}

#[server(input = GetUrl, prefix = "/api", endpoint = "probe")]
pub async fn probe() -> Result<String, ServerFnError> {
    Ok(String::from("probe"))
}

/// Exercises every builder method through the same import surface a
/// downstream crate uses.
///
/// The `leptos_wasi` imports above are exactly the ones this fixture needed
/// before these calls existed. If a refactor made any of these methods
/// require a new import - by moving them onto a trait, for example - this
/// file would stop compiling. That is what the fixture is for.
pub fn register_everything(
    handler: Handler,
) -> Result<Handler, RegistrationError> {
    handler
        .with_server_fn::<Probe>()
        .static_files_handler("/pkg", |_path| None)?
        .generate_routes(app)?
        .generate_routes_with_discovery_context(app, || {})?
        .generate_routes_with_context(app, || {})?
        .generate_routes_with_exclusions_and_discovery_context(
            app,
            None,
            || {},
        )?
        .generate_routes_with_exclusions_and_context(app, None, || {})
}
