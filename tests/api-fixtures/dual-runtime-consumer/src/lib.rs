#![deny(warnings)]

use leptos::prelude::*;
use server_fn::codec::GetUrl;

use leptos_wasi::{
    ExecutorError, HandlerConfig, RegistrationError, RouteTable,
    wasip2::{
        Handler as Wasip2Handler, HandlerError as Wasip2HandlerError,
        prelude::{IncomingRequest as Wasip2Request, ResponseOutparam},
    },
    wasip3::{
        Handler as Wasip3Handler, HandlerError as Wasip3HandlerError,
        init_wasip3_spawner,
    },
};

pub fn initialize_wasip3_spawner() -> Result<(), ExecutorError> {
    init_wasip3_spawner()
}

pub fn timeout_config() -> HandlerConfig {
    HandlerConfig::default()
        .with_request_body_timeout(std::time::Duration::from_secs(30))
}

pub fn build_wasip2_default(
    request: Wasip2Request,
    response_out: ResponseOutparam,
) -> Result<Wasip2Handler, Wasip2HandlerError> {
    Wasip2Handler::build(request, response_out)
}

pub fn build_wasip2_configured(
    request: Wasip2Request,
    response_out: ResponseOutparam,
    config: HandlerConfig,
) -> Result<Wasip2Handler, Wasip2HandlerError> {
    Wasip2Handler::build_with_config(request, response_out, config)
}

pub async fn build_wasip3_default(
    request: http::Request<::wasip3::http_compat::IncomingRequestBody>,
) -> Result<Wasip3Handler, Wasip3HandlerError> {
    Wasip3Handler::build(request).await
}

pub async fn build_wasip3_configured(
    request: http::Request<::wasip3::http_compat::IncomingRequestBody>,
    config: HandlerConfig,
) -> Result<Wasip3Handler, Wasip3HandlerError> {
    Wasip3Handler::build_with_config(request, config).await
}

fn app() -> impl IntoView {
    view! { <p>"consumer"</p> }
}

#[server(input = GetUrl, prefix = "/api", endpoint = "probe")]
pub async fn probe() -> Result<String, ServerFnError> {
    Ok(String::from("probe"))
}

/// Exercises every builder method on both previews through direct paths.
///
/// This fixture deliberately imports `Handler` by path rather than through a
/// prelude, because that is the shape a prelude-based fixture would hide: a
/// refactor that moved these methods onto a trait would keep compiling for
/// glob-importing callers and break for these. The `leptos_wasi` imports
/// above are exactly the ones this file needed before these calls existed.
pub fn register_everything_wasip2(
    handler: Wasip2Handler,
) -> Result<Wasip2Handler, RegistrationError> {
    let table = RouteTable::discover(app, None, || {})?;
    handler
        .with_server_fn::<Probe>()
        .static_files_handler("/pkg", |_path| None)?
        .static_files_handler_with("/assets", |_| None)?
        .generate_routes(app)?
        .generate_routes_with_discovery_context(app, || {})?
        .generate_routes_with_context(app, || {})?
        .generate_routes_with_exclusions_and_discovery_context(
            app,
            None,
            || {},
        )?
        .generate_routes_with_exclusions_and_context(app, None, || {})?
        .generate_routes_from(&table)
}

/// The Preview 3 half of [`register_everything_wasip2`].
pub fn register_everything_wasip3(
    handler: Wasip3Handler,
) -> Result<Wasip3Handler, RegistrationError> {
    let table = RouteTable::discover(app, None, || {})?;
    handler
        .with_server_fn::<Probe>()
        .static_files_handler("/pkg", |_path| None)?
        .static_files_handler_with("/assets", |_| None)?
        .generate_routes(app)?
        .generate_routes_with_discovery_context(app, || {})?
        .generate_routes_with_context(app, || {})?
        .generate_routes_with_exclusions_and_discovery_context(
            app,
            None,
            || {},
        )?
        .generate_routes_with_exclusions_and_context(app, None, || {})?
        .generate_routes_from(&table)
}
