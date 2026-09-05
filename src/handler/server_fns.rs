//! Server-function plumbing: the boxed handler the core stores, the body
//! projections its bounds need, and the redirect policy applied to what a
//! server function returns.

use std::{future::Future, marker::PhantomData, pin::Pin};

use bytes::Bytes;
use http::{HeaderValue, Request, StatusCode, header::LOCATION};
use server_fn::{ServerFn, error::ServerFnErrorErr, middleware::Service};

use super::http_util::sanitize_referrer;
use crate::{__private::ServerWithBody, response::Body};

pub(super) type ServerFnHandler = Box<
    dyn Fn(
            Request<Bytes>,
        )
            -> Pin<Box<dyn Future<Output = http::Response<Body>> + Send>>
        + Send,
>;

pub(super) type ReqBody<T> = <<T as ServerFn>::Server as ServerWithBody<
    <T as ServerFn>::Error,
    <T as ServerFn>::InputStreamError,
    <T as ServerFn>::OutputStreamError,
>>::ReqBody;

pub(super) type ResBody<T> = <<T as ServerFn>::Server as ServerWithBody<
    <T as ServerFn>::Error,
    <T as ServerFn>::InputStreamError,
    <T as ServerFn>::OutputStreamError,
>>::ResBody;

pub(super) struct TypedServerFnService<T>(PhantomData<fn() -> T>);

impl<T> Default for TypedServerFnService<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T> Service<Request<ReqBody<T>>, http::Response<ResBody<T>>>
    for TypedServerFnService<T>
where
    T: ServerFn + 'static,
    T::Server:
        ServerWithBody<T::Error, T::InputStreamError, T::OutputStreamError>,
    ReqBody<T>: Send + 'static,
    ResBody<T>: Send + 'static,
{
    fn run(
        &mut self,
        request: Request<ReqBody<T>>,
        _serialize_error: fn(ServerFnErrorErr) -> Bytes,
    ) -> Pin<
        Box<dyn Future<Output = http::Response<ResBody<T>>> + Send + 'static>,
    > {
        Box::pin(T::run_on_server(request))
    }
}

pub(super) fn apply_server_fn_redirect(
    response: &mut http::Response<Body>,
    accepts_html: bool,
    referrer: Option<HeaderValue>,
) {
    let mut redirect_target = None;
    let status = response.status();
    let promotable = status.is_success() || status.is_redirection();
    if accepts_html
        && promotable
        && !response.headers().contains_key(LOCATION)
        && let Some(referrer) = referrer
    {
        *response.status_mut() = StatusCode::FOUND;
        redirect_target = sanitize_referrer(&referrer)
            .or_else(|| Some(HeaderValue::from_static("/")));
    }
    if redirect_target.is_none()
        && status.is_redirection()
        && let Some(location) = response.headers().get(LOCATION).cloned()
    {
        redirect_target = sanitize_referrer(&location)
            .or_else(|| Some(HeaderValue::from_static("/")));
    }
    if let Some(target) = redirect_target {
        response.headers_mut().insert(LOCATION, target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redirected(
        status: StatusCode,
        location: Option<&'static str>,
        accepts_html: bool,
        referrer: Option<&'static str>,
    ) -> (StatusCode, Option<String>) {
        let mut response =
            http::Response::new(Body::Sync(Bytes::from_static(b"body")));
        *response.status_mut() = status;
        if let Some(location) = location {
            response
                .headers_mut()
                .insert(LOCATION, HeaderValue::from_static(location));
        }
        apply_server_fn_redirect(
            &mut response,
            accepts_html,
            referrer.map(HeaderValue::from_static),
        );
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().expect("ascii").to_owned());
        (response.status(), location)
    }

    #[test]
    fn html_form_posts_redirect_back_to_a_same_origin_referrer() {
        assert_eq!(
            redirected(
                StatusCode::OK,
                None,
                true,
                Some("http://127.0.0.1/previous-page")
            ),
            (StatusCode::FOUND, Some("/previous-page".to_owned()))
        );
    }

    #[test]
    fn html_form_posts_fall_back_to_root_for_unusable_referrers() {
        assert_eq!(
            redirected(
                StatusCode::OK,
                None,
                true,
                Some("http://127.0.0.1/%5Cevil.example.com")
            ),
            (StatusCode::FOUND, Some("/".to_owned()))
        );
    }

    #[test]
    fn cross_origin_locations_are_reduced_to_their_path() {
        assert_eq!(
            redirected(
                StatusCode::FOUND,
                Some("https://evil.example.com/take-over"),
                false,
                None
            ),
            (StatusCode::FOUND, Some("/take-over".to_owned()))
        );
    }

    #[test]
    fn api_clients_keep_an_explicit_same_origin_location() {
        assert_eq!(
            redirected(StatusCode::OK, Some("/dashboard"), false, None),
            (StatusCode::OK, Some("/dashboard".to_owned()))
        );
    }

    #[test]
    fn responses_without_a_location_are_left_alone() {
        assert_eq!(
            redirected(StatusCode::OK, None, false, None),
            (StatusCode::OK, None)
        );
    }

    #[test]
    fn html_error_responses_keep_their_status_despite_a_referrer() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert_eq!(
                redirected(
                    status,
                    None,
                    true,
                    Some("http://127.0.0.1/previous-page")
                ),
                (status, None),
                "{status} must not be promoted to a redirect"
            );
        }
    }

    #[test]
    fn created_responses_keep_an_absolute_location() {
        assert_eq!(
            redirected(
                StatusCode::CREATED,
                Some("https://cdn.example.com/new"),
                true,
                Some("http://127.0.0.1/form")
            ),
            (
                StatusCode::CREATED,
                Some("https://cdn.example.com/new".to_owned())
            )
        );
    }

    #[test]
    fn explicit_root_location_is_not_overwritten_by_referer() {
        assert_eq!(
            redirected(
                StatusCode::OK,
                Some("/"),
                true,
                Some("http://127.0.0.1/previous-page")
            ),
            (StatusCode::OK, Some("/".to_owned()))
        );
    }
}
