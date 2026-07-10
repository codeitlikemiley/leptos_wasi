#![deny(warnings)]

use leptos_wasi::{
    ExecutorError,
    wasip3::prelude::{
        Handler, HandlerConfig, HandlerError, IncomingRequest,
        init_wasip3_spawner,
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
