//! Shared Leptos request handling and runtime-specific WASI HTTP adapters.
//!
//! The submodules follow a request's shape: [`policy`] decides whether it is
//! allowed at all, [`core`] accumulates what will answer it, [`render`] turns
//! that into a response, and [`wasip2`]/[`wasip3`] carry it over the host's
//! own HTTP types. [`builder`] holds the registration methods both previews
//! share.
//!
//! Every item shared across submodules is `pub(super)` - the same access they
//! all had while this was a single file. Keep the tree one level deep: that
//! is what makes `pub(super)` mean `crate::handler` everywhere.

mod builder;
mod core;
mod http_util;
mod policy;
mod render;
mod routes;
mod server_fns;
#[cfg(test)]
mod test_support;
mod trace;

#[cfg(feature = "wasip2")]
pub mod wasip2;
#[cfg(feature = "wasip3")]
pub mod wasip3;

// Reached from `crate::request::p2`, which is the only consumer outside
// this module tree; Preview 3 calls it directly from `handler::wasip3`.
#[cfg(feature = "wasip2")]
pub(crate) use policy::validate_content_length;
pub use policy::{
    DEFAULT_MAX_REQUEST_BODY_SIZE, HandlerConfig, RegistrationError,
    RequestPolicyError,
};
pub use routes::{RouteTable, validate_route_table};
