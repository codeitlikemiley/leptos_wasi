use spin_sdk::wasip3::http::types::Response;
use spin_sdk::{
    http::{
        EmptyBody, HeaderMap, HeaderValue, IntoResponse, Request, Result,
        StatusCode, next,
    },
    http_service,
};

const FIXTURE_VALUE: HeaderValue = HeaderValue::from_static("wasip3-vnext");

#[http_service]
async fn handle(mut request: Request) -> Result<Response> {
    if request.uri().path().starts_with("/api/")
        && !has_test_authorization(&request)
    {
        let mut headers = HeaderMap::new();
        headers.insert("x-leptos-wasi-middleware", FIXTURE_VALUE.clone());
        headers.insert("www-authenticate", HeaderValue::from_static("Bearer"));
        return Ok((StatusCode::UNAUTHORIZED, headers, EmptyBody::new())
            .into_response()?);
    }

    // The current RC http-compat bridge rebuilds the forwarded request. `host`
    // is represented by the WASI request authority, and hop-by-hop fields are
    // forbidden in a new WASI fields resource, so do not copy them.
    for name in [
        "connection",
        "host",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        request.headers_mut().remove(name);
    }
    request
        .headers_mut()
        .insert("x-leptos-wasi-middleware-request", FIXTURE_VALUE.clone());

    let mut response = next(request).await?;
    response
        .headers_mut()
        .insert("x-leptos-wasi-middleware", FIXTURE_VALUE.clone());
    Ok(response.into_response()?)
}

fn has_test_authorization(request: &Request) -> bool {
    let mut values = request.headers().get_all("authorization").iter();
    let value = values.next();
    values.next().is_none()
        && value.is_some_and(|value| value == "Bearer allow")
}
