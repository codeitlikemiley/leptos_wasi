//! Utilities for manipulating Leptos responses from reactive context.

use crate::response::ResponseOptions;
use http::{HeaderName, HeaderValue, StatusCode, header, request::Parts};
use leptos::prelude::use_context;
use server_fn::redirect::REDIRECT_HEADER;

/// Allows returning an HTTP redirection from components.
///
/// Inspects the current Leptos context for `Parts` and `ResponseOptions` to insert the
/// `Location` header or set a 302 status code.
///
/// # Example
///
/// ```ignore
/// use leptos_wasi::utils::redirect;
/// use leptos::prelude::*;
///
/// #[component]
/// fn RedirectButton() -> impl IntoView {
///     let on_click = |_| {
///         redirect("/target-page");
///     };
///     view! { <button on:click=on_click>"Go"</button> }
/// }
/// ```
pub fn redirect(path: &str) {
    if let (Some(req), Some(res)) =
        (use_context::<Parts>(), use_context::<ResponseOptions>())
    {
        // insert the Location header in any case
        match header::HeaderValue::from_str(path) {
            Ok(value) => {
                res.insert_header(header::LOCATION, value);
            }
            Err(e) => {
                eprintln!("Invalid redirect path: {path}, error: {e:?}");
                res.set_status(StatusCode::BAD_REQUEST);
                return;
            }
        }

        let accepts_html = req
            .headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"));
        if accepts_html {
            // if the request accepts text/html, it's a plain form request and needs
            // to have the 302 code set
            res.set_status(StatusCode::FOUND);
        } else {
            // otherwise, we sent it from the server fn client and actually don't want
            // to set a real redirect, as this will break the ability to return data
            // instead, set the REDIRECT_HEADER to indicate that the client should redirect
            res.insert_header(
                HeaderName::from_static(REDIRECT_HEADER),
                HeaderValue::from_static(""),
            );
        }
    } else {
        eprintln!(
            "Couldn't retrieve either Parts or ResponseOptions while trying \
             to redirect()."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::redirect;
    use crate::response::{ResponseOptions, ResponseParts};
    use http::{HeaderValue, Request, StatusCode, header};
    use leptos::prelude::{Owner, provide_context};

    fn redirect_with(
        accept: Option<&'static str>,
        path: &str,
    ) -> ResponseParts {
        let mut builder = Request::builder().uri("/current");
        if let Some(accept) = accept {
            builder = builder.header(header::ACCEPT, accept);
        }
        let (parts, ()) = builder
            .body(())
            .expect("test request should be valid")
            .into_parts();

        let owner = Owner::new();
        let options = owner.with(|| {
            let options = ResponseOptions::default();
            provide_context(parts);
            provide_context(options.clone());
            redirect(path);
            options
        });
        options.snapshot()
    }

    #[test]
    fn html_requests_receive_a_found_status_and_location() {
        let parts = redirect_with(Some("text/html"), "/target-page");

        assert_eq!(parts.status(), Some(StatusCode::FOUND));
        assert_eq!(
            parts.headers().get(header::LOCATION),
            Some(&HeaderValue::from_static("/target-page"))
        );
    }

    #[test]
    fn client_requests_receive_the_redirect_header_instead_of_a_status() {
        let parts = redirect_with(Some("application/json"), "/target-page");

        assert_eq!(parts.status(), None);
        assert_eq!(
            parts.headers().get(header::LOCATION),
            Some(&HeaderValue::from_static("/target-page"))
        );
        assert!(
            parts
                .headers()
                .contains_key(server_fn::redirect::REDIRECT_HEADER)
        );
    }

    #[test]
    fn requests_without_an_accept_header_use_the_client_redirect_protocol() {
        let parts = redirect_with(None, "/target-page");

        assert_eq!(parts.status(), None);
        assert!(
            parts
                .headers()
                .contains_key(server_fn::redirect::REDIRECT_HEADER)
        );
    }

    #[test]
    fn header_injection_attempts_are_rejected_with_bad_request() {
        let parts = redirect_with(
            Some("text/html"),
            "/target-page\r\nLocation: http://evil.example.com",
        );

        assert_eq!(parts.status(), Some(StatusCode::BAD_REQUEST));
        assert!(!parts.headers().contains_key(header::LOCATION));
    }

    #[test]
    fn redirecting_without_context_does_not_panic() {
        let owner = Owner::new();
        owner.with(|| redirect("/target-page"));
    }
}
