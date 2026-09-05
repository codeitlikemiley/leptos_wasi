//! Host-native microbenches for the Phase 6 hot path.
//!
//! Unlike `route_discovery`, this file is wired into CI as a non-gating job
//! (`continue-on-error`). It never constructs WASI host resources: path
//! normalization is pure, header pairing is the guest half of
//! `Response::headers`, and dispatch uses scripted unit pollables.

use bytes::Bytes;
use http::{
    HeaderValue,
    header::{CONTENT_TYPE, LOCATION},
};
use leptos_wasi::{
    __private,
    response::{Body, Response},
};

fn main() {
    divan::main();
}

#[divan::bench]
fn normalize_static_path() -> String {
    __private::normalize_static_path("pkg/images/logo.svg")
        .expect("fixture path should normalize")
}

#[divan::bench]
fn response_header_pairs() -> usize {
    let mut response =
        Response(http::Response::new(Body::Sync(Bytes::from_static(b"ok"))));
    response
        .0
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    response
        .0
        .headers_mut()
        .insert(LOCATION, HeaderValue::from_static("/next"));
    __private::response_header_pairs(&response).len()
}

#[cfg(feature = "wasip2")]
#[divan::bench]
fn dispatch_ready() -> bool {
    __private::bench_dispatch_ready(8, &[0, 3, 7])
}
