#![deny(warnings)]

use leptos_wasi::{
    ExecutorError, HandlerConfig,
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
