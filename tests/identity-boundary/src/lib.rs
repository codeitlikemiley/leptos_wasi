//! The portable middleware envelope and wasi-auth's HMAC trusted context share
//! the `x-wasi-auth-context` header name and nothing else.
//!
//! `wasi-http-metadata::VerifiedAuthContext` is the unsigned `AuthContextV1`
//! installed by composed `authn-policy` or `accept_trusted_ingress`.
//! `wasi_auth::context::VerifiedRequestContext` is reconstructed only by
//! `TrustedContextCodec` after an HMAC-SHA-256 check bound to audience,
//! method, path, and request id. A terminal must not treat one as the other.

#[cfg(test)]
mod tests {
    use http::{Method, Request, header::AUTHORIZATION};
    use wasi_auth::http::TrustedContextCodec;
    use wasi_http_authn::{TrustedIngressConfig, accept_trusted_ingress};
    use wasi_http_metadata::{
        AUTH_CONTEXT_HEADER, AuthContextV1, VerifiedAuthContext,
        encode_auth_context,
    };

    const SERVICE_ID: &str = "leptos-wasi-counter";
    const AUDIENCE: &str = "api://leptos-wasi-counter";

    fn portable_context() -> AuthContextV1 {
        AuthContextV1::anonymous(SERVICE_ID, [AUDIENCE])
            .expect("anonymous portable context should be valid")
    }

    fn hmac_shaped_envelope() -> &'static str {
        // Wire shape of `TrustedContextCodec::seal`: base64url(payload) + '.' +
        // base64url(mac). Not a real signature; the portable decoder must reject
        // the shape before any field is interpreted.
        "eyJ2ZXJzaW9uIjoxfQ.dGVzdA"
    }

    #[test]
    fn header_names_collide_but_codecs_reject_each_other() {
        let portable = encode_auth_context(&portable_context())
            .expect("portable context should encode");
        let portable_value =
            portable.to_str().expect("portable header is text");
        assert!(
            !portable_value.contains('.'),
            "portable AuthContextV1 is a single base64url blob, got {portable_value}"
        );

        let codec = TrustedContextCodec::new(AUDIENCE, [7; 32])
            .expect("non-zero HMAC key should be accepted");
        let opened = codec.open(
            portable_value,
            &Method::GET,
            "/",
            "request-id",
            1_700_000_000,
        );
        assert!(
            opened.is_err(),
            "HMAC codec must reject an unsigned portable envelope"
        );

        let mut hmac_request = Request::builder()
            .uri("/")
            .header(AUTH_CONTEXT_HEADER, hmac_shaped_envelope())
            .body(())
            .expect("request should build");
        let config = TrustedIngressConfig::new(SERVICE_ID, [AUDIENCE])
            .expect("trusted ingress config should be valid");
        assert!(
            accept_trusted_ingress(&config, &mut hmac_request).is_err(),
            "portable ingress must reject an HMAC-shaped envelope"
        );
        assert!(
            hmac_request
                .extensions()
                .get::<VerifiedAuthContext>()
                .is_none(),
            "a rejected HMAC envelope must not install portable identity"
        );
        assert!(
            !hmac_request.headers().contains_key(AUTH_CONTEXT_HEADER),
            "rejection still strips the shared header"
        );
    }

    #[test]
    fn portable_ingress_installs_only_its_own_context_type() {
        let mut request = Request::builder()
            .uri("/")
            .header(
                AUTH_CONTEXT_HEADER,
                encode_auth_context(&portable_context())
                    .expect("portable context should encode"),
            )
            .header(AUTHORIZATION, "Bearer spoofed")
            .body(())
            .expect("request should build");
        let config = TrustedIngressConfig::new(SERVICE_ID, [AUDIENCE])
            .expect("trusted ingress config should be valid");
        // A surviving bearer is an invalid boundary even when the portable
        // envelope itself is well formed. That rejection must not fall open.
        assert!(accept_trusted_ingress(&config, &mut request).is_err());
        assert!(request.extensions().get::<VerifiedAuthContext>().is_none());

        let mut clean = Request::builder()
            .uri("/")
            .header(
                AUTH_CONTEXT_HEADER,
                encode_auth_context(&portable_context())
                    .expect("portable context should encode"),
            )
            .body(())
            .expect("request should build");
        let verified = accept_trusted_ingress(&config, &mut clean)
            .expect("portable envelope should be accepted");
        assert_eq!(verified.context().service_id(), SERVICE_ID);
        let installed = clean
            .extensions()
            .get::<VerifiedAuthContext>()
            .expect("portable context should be installed");
        assert_eq!(installed.context().service_id(), SERVICE_ID);
        assert!(!clean.headers().contains_key(AUTHORIZATION));
        assert!(!clean.headers().contains_key(AUTH_CONTEXT_HEADER));
    }
}
