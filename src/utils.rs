//! Utilities for manipulating Leptos responses from reactive context.

use crate::response::ResponseOptions;
use http::{HeaderName, HeaderValue, StatusCode, header, request::Parts};
use leptos::prelude::use_context;
use server_fn::redirect::REDIRECT_HEADER;

fn escaped_redirect_path(path: &str) -> impl std::fmt::Display + '_ {
    path.escape_debug()
}

/// Writes a redirect through [`ResponseOptions`].
///
/// Inspects the current Leptos context for `Parts` and `ResponseOptions`
/// and either inserts `Location` or sets a 302 status. This path is merged
/// into the response after the server-function `Location` sanitizer, so an
/// absolute off-origin URL is sent as written. Applications that build a target
/// from request data should validate it first.
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
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    path = %escaped_redirect_path(path),
                    error = ?e,
                    "Invalid redirect path"
                );
                #[cfg(not(feature = "tracing"))]
                eprintln!(
                    "Invalid redirect path: {}, error: {e:?}",
                    escaped_redirect_path(path)
                );
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
        #[cfg(feature = "tracing")]
        tracing::warn!(
            path = %escaped_redirect_path(path),
            "Couldn't retrieve either Parts or ResponseOptions while trying to redirect()"
        );
        #[cfg(not(feature = "tracing"))]
        eprintln!(
            "Couldn't retrieve either Parts or ResponseOptions while trying \
             to redirect({}).",
            escaped_redirect_path(path)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{escaped_redirect_path, redirect};
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

    #[test]
    fn injected_redirect_paths_log_as_a_single_escaped_line() {
        let path = "/x\r\nLocation: evil";
        let logged = escaped_redirect_path(path).to_string();
        assert_eq!(logged.lines().count(), 1);
        assert!(!logged.contains('\r'), "{logged:?} must not contain CR");
        assert!(!logged.contains('\n'), "{logged:?} must not contain LF");
        assert!(logged.contains("\\r\\n"), "{logged:?}");
    }
}
