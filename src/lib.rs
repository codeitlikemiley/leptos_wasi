//! Leptos server integration for WASI HTTP components.
//!
//! Rust currently compiles both supported component models through the
//! `wasm32-wasip2` target. Enable `wasip2` for synchronous Preview 2 host
//! bindings, `wasip3` for async Preview 3 bindings, or both when a consumer
//! needs to compile both adapters.

#[cfg(not(any(feature = "wasip2", feature = "wasip3")))]
compile_error!("enable at least one of the `wasip2` or `wasip3` features");

mod executor;
mod handler;
mod integration;
mod request;
pub mod response;
mod static_files;
pub mod utils;

pub use executor::ExecutorError;
pub use handler::{
    DEFAULT_MAX_REQUEST_BODY_SIZE, HandlerConfig, RegistrationError,
    RequestPolicyError, validate_route_table,
};

/// Implementation details required by generated public bounds.
///
/// This module is not part of the stable API. Its traits are blanket
/// implemented and cannot be implemented independently by consumers.
#[doc(hidden)]
pub mod __private {
    /// Projects the request and response body types from a server backend.
    pub trait ServerWithBody<Error, InputStreamError, OutputStreamError>:
        server_fn::server::Server<
            Error,
            InputStreamError,
            OutputStreamError,
            Request = http::Request<Self::ReqBody>,
            Response = http::Response<Self::ResBody>,
        >
    {
        /// Request body associated with the backend.
        type ReqBody;
        /// Response body associated with the backend.
        type ResBody;
    }

    impl<S, Error, InputStreamError, OutputStreamError, ReqBody, ResBody>
        ServerWithBody<Error, InputStreamError, OutputStreamError> for S
    where
        S: server_fn::server::Server<
                Error,
                InputStreamError,
                OutputStreamError,
                Request = http::Request<ReqBody>,
                Response = http::Response<ResBody>,
            >,
    {
        type ReqBody = ReqBody;
        type ResBody = ResBody;
    }

    /// Returns the current Preview 2 pollable queue depth for release probes.
    #[cfg(feature = "wasip2")]
    #[doc(hidden)]
    #[must_use]
    pub fn wasip2_pollable_queue_depth() -> usize {
        crate::executor::pollable_queue_depth()
    }
}

/// Shared response and configuration types.
pub mod prelude {
    pub use crate::{
        ExecutorError, HandlerConfig, RegistrationError, RequestPolicyError,
        response::{Body, ResponseOptions, ResponseParts},
        utils::redirect,
    };
    pub use http::StatusCode;
}

/// WASI Preview 2 integration.
#[cfg(feature = "wasip2")]
pub mod wasip2 {
    pub use crate::{
        executor::{
            Executor, ExecutorError, Mode, WaitPoll, init_wasip2_executor,
            sleep,
        },
        handler::wasip2::{Handler, HandlerError},
    };

    /// WASI Preview 2 request conversion helpers.
    pub mod request {
        pub use crate::request::p2::{
            RequestError, from_wasi_request, method_wasi_to_http,
            scheme_wasi_to_http,
        };
    }

    /// Commonly used WASI Preview 2 integration types.
    pub mod prelude {
        pub use super::{
            Executor as WasiExecutor, Handler, HandlerError, Mode,
            init_wasip2_executor,
        };
        pub use crate::prelude::*;
        pub use wasi::exports::wasi::http::incoming_handler::{
            IncomingRequest, ResponseOutparam,
        };
    }
}

/// WASI Preview 3 integration.
#[cfg(feature = "wasip3")]
pub mod wasip3 {
    pub use crate::{
        executor::{Wasip3Executor, init_wasip3_spawner},
        handler::wasip3::{Handler, HandlerError},
    };

    /// Commonly used WASI Preview 3 integration types.
    pub mod prelude {
        pub use super::{Handler, HandlerError, init_wasip3_spawner};
        pub use crate::prelude::*;
        pub use ::wasip3::http::types::Request as IncomingRequest;
    }
}

/// Chunk size used when reading WASI Preview 2 streams.
#[cfg(feature = "wasip2")]
pub(crate) const CHUNK_BYTE_SIZE: usize = 8192;
