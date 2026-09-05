//! Request-shape helpers: standard Leptos contexts, content negotiation,
//! and referrer sanitizing.

use http::{
    HeaderMap, HeaderValue, Request, Uri, header::ACCEPT, request::Parts,
};
use leptos::prelude::provide_context;
use leptos_router::{
    components::provide_server_redirect, location::RequestUrl,
};

use crate::{response::ResponseOptions, utils::redirect};

pub(super) const ISLANDS_ROUTER_HEADER: &str = "Islands-Router";

pub(super) fn provide_standard_contexts(
    parts: Parts,
    response: ResponseOptions,
) {
    let request_url = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    provide_context(RequestUrl::new(request_url));
    provide_context(parts);
    provide_context(response);
    provide_server_redirect(redirect);
    leptos::nonce::provide_nonce();
}

/// Whether `Accept` includes HTML at a positive q-value.
///
/// Unlike axum's `contains("text/html")` check, this parses each media range
/// and rejects `text/html;q=0`. A quality of zero means "not acceptable".
pub(crate) fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|media_range| {
            let mut fields = media_range.split(';');
            let media_type = fields.next().unwrap_or_default().trim();
            let quality = fields
                .filter_map(|field| field.trim().strip_prefix("q="))
                .find_map(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            quality > 0.0
                && matches!(media_type, "text/html" | "application/xhtml+xml")
        })
}

pub(super) fn is_islands_router_navigation<B>(request: &Request<B>) -> bool {
    cfg!(feature = "islands-router")
        && request.headers().contains_key(ISLANDS_ROUTER_HEADER)
}

pub(super) fn sanitize_referrer(referrer: &HeaderValue) -> Option<HeaderValue> {
    let value = referrer.to_str().ok()?;
    let uri = value.parse::<Uri>().ok()?;
    let path = uri.path_and_query()?.as_str();
    if path.starts_with("/\\")
        || path.contains('\\')
        || path.contains("%5c")
        || path.contains("%5C")
    {
        return None;
    }
    if path.starts_with('/') && !path.starts_with("//") {
        HeaderValue::from_str(path).ok()
    } else {
        None
    }
}

/// Copies the non-standard `Referrer` spelling into `Referer` when absent.
///
/// `server_fn`'s `form-redirects` feature only reads `Referer`. Without this
/// mirror, a client that sends only `Referrer` gets `Location: /` from the
/// framework before [`super::server_fns::apply_server_fn_redirect`] runs, and
/// an explicit root location is intentionally left alone.
pub(super) fn mirror_referrer_spelling(headers: &mut HeaderMap) {
    if headers.get(http::header::REFERER).is_none()
        && let Some(alternate) = headers.get("referrer").cloned()
    {
        headers.insert(http::header::REFERER, alternate);
    }
}

#[cfg(test)]
mod tests {
    use http::{
        HeaderMap, HeaderValue,
        header::{ACCEPT, REFERER},
    };

    use super::{accepts_html, mirror_referrer_spelling, sanitize_referrer};

    #[test]
    fn html_with_zero_quality_is_not_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html;q=0"));
        assert!(!accepts_html(&headers));
    }

    fn accept(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn browser_navigation_accept_headers_are_html() {
        assert!(accepts_html(&accept(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        assert!(accepts_html(&accept("application/xhtml+xml")));
        assert!(accepts_html(&accept(" text/html ; charset=utf-8")));
    }

    #[test]
    fn non_navigation_accept_headers_are_not_html() {
        assert!(!accepts_html(&accept("application/json")));
        assert!(!accepts_html(&accept("*/*")));
        assert!(!accepts_html(&HeaderMap::new()));
        assert!(!accepts_html(&accept("text/html;q=0.000")));
    }

    #[test]
    fn accept_html_is_detected_across_repeated_headers() {
        let mut headers = HeaderMap::new();
        headers.append(ACCEPT, HeaderValue::from_static("application/json"));
        headers.append(ACCEPT, HeaderValue::from_static("text/html"));

        assert!(accepts_html(&headers));
    }

    fn sanitized(value: &'static str) -> Option<String> {
        sanitize_referrer(&HeaderValue::from_static(value))
            .map(|value| value.to_str().expect("ascii").to_owned())
    }

    #[test]
    fn referrers_are_reduced_to_same_origin_paths() {
        assert_eq!(
            sanitized("http://127.0.0.1/previous-page"),
            Some("/previous-page".to_owned())
        );
        assert_eq!(
            sanitized("https://malicious.example.com/steal?a=1"),
            Some("/steal?a=1".to_owned())
        );
        assert_eq!(
            sanitized("/relative/page"),
            Some("/relative/page".to_owned())
        );
    }

    #[test]
    fn protocol_relative_and_backslash_referrers_are_rejected() {
        assert_eq!(sanitized("//evil.example.com/path"), None);
        assert_eq!(sanitized("http://127.0.0.1/\\evil.example.com"), None);
        assert_eq!(sanitized("http://127.0.0.1/%5Cevil.example.com"), None);
        assert_eq!(sanitized("http://127.0.0.1/%5cevil.example.com"), None);
        assert_eq!(sanitized("mailto:someone@example.com"), None);
    }

    #[test]
    fn referrer_spelling_is_mirrored_into_referer_when_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "referrer",
            HeaderValue::from_static("http://127.0.0.1/other-page"),
        );
        mirror_referrer_spelling(&mut headers);
        assert_eq!(
            headers.get(REFERER).and_then(|value| value.to_str().ok()),
            Some("http://127.0.0.1/other-page")
        );
    }

    #[test]
    fn existing_referer_is_not_replaced_by_referrer_spelling() {
        let mut headers = HeaderMap::new();
        headers.insert(REFERER, HeaderValue::from_static("/previous-page"));
        headers.insert(
            "referrer",
            HeaderValue::from_static("http://127.0.0.1/other-page"),
        );
        mirror_referrer_spelling(&mut headers);
        assert_eq!(
            headers.get(REFERER).and_then(|value| value.to_str().ok()),
            Some("/previous-page")
        );
    }
}
