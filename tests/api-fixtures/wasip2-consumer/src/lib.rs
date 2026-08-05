#![deny(warnings)]

use leptos::prelude::*;
use server_fn::codec::GetUrl;

use leptos_wasi::ExecutorError;
use leptos_wasi::wasip2::prelude::{
    Handler, HandlerConfig, HandlerError, IncomingRequest, Mode,
    RegistrationError, ResponseOutparam, WasiExecutor, init_wasip2_executor,
};

pub fn build_default(
    request: IncomingRequest,
    response_out: ResponseOutparam,
) -> Result<Handler, HandlerError> {
    Handler::build(request, response_out)
}

pub fn build_configured(
    request: IncomingRequest,
    response_out: ResponseOutparam,
    config: HandlerConfig,
) -> Result<Handler, HandlerError> {
    Handler::build_with_config(request, response_out, config)
}

pub fn initialize_executor(mode: Mode) -> Result<WasiExecutor, ExecutorError> {
    init_wasip2_executor(mode)
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
