#![deny(warnings)]

use leptos_wasi::ExecutorError;
use leptos_wasi::wasip2::prelude::{
    Handler, HandlerConfig, HandlerError, IncomingRequest, Mode,
    ResponseOutparam, WasiExecutor, init_wasip2_executor,
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
