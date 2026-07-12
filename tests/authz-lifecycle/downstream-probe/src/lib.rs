//! Transparent final-WASIp3 probe placed immediately before the Leptos app.
//!
//! One generic log marker proves that authorization invoked downstream. A
//! dedicated route emits trailers because `leptos_wasi` itself intentionally
//! does not expose a response-trailer API.

#![deny(rustdoc::broken_intra_doc_links)]

use wasi_http_middleware_component_support::{
    replace_response_headers, response_headers, set_header,
};
use wasip3::http::types::{ErrorCode, Headers, Request, Response};
use wasip3::{wit_future, wit_stream};

#[allow(unknown_lints, missing_docs, clippy::same_length_and_capacity)]
mod bindings {
    wasi_http_middleware_component_support::generate_middleware_bindings!(
        "../../../../wasi-auth/legacy/wasi-http-middleware/wit"
    );
}

use bindings::wasi::http::handler;

struct Component;

bindings::export!(Component with_types_in bindings);

impl bindings::exports::wasi::http::handler::Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        eprintln!("authz-lifecycle-probe: downstream-invoked");
        let response = if request.get_path_with_query().as_deref()
            == Some("/__authz_lifecycle/trailers")
        {
            trailer_response()?
        } else {
            handler::handle(request).await?
        };
        let mut headers = response_headers(&response);
        set_header(
            &mut headers,
            "x-authz-lifecycle-downstream",
            b"invoked-once".to_vec(),
        );
        replace_response_headers(response, &headers)
    }
}

fn trailer_response() -> Result<Response, ErrorCode> {
    let body = b"trailer-body\n".to_vec();
    let headers = Headers::from_list(&[
        ("content-type".to_owned(), b"text/plain".to_vec()),
        ("trailer".to_owned(), b"x-authz-lifecycle-check".to_vec()),
    ])
    .map_err(|_| ErrorCode::HttpResponseHeaderSectionSize(None))?;
    let trailers = Headers::from_list(&[(
        "x-authz-lifecycle-check".to_owned(),
        b"preserved".to_vec(),
    )])
    .map_err(|_| ErrorCode::HttpResponseTrailerSectionSize(None))?;
    let (mut body_writer, body_reader) = wit_stream::new();
    let (body_result_writer, body_result) =
        wit_future::new(|| Err(ErrorCode::InternalError(None)));
    wit_bindgen::spawn_local(async move {
        if body_writer.write_all(body).await.is_empty() {
            drop(body_writer);
            let _write_result =
                body_result_writer.write(Ok(Some(trailers))).await;
        }
    });
    let (response, transmission_result) =
        Response::new(headers, Some(body_reader), body_result);
    wit_bindgen::spawn_local(async move {
        let _result = transmission_result.await;
    });
    Ok(response)
}
