//! Runtime-specific request conversion helpers.

/// WASI Preview 2 request conversion.
#[cfg(feature = "wasip2")]
pub mod p2 {
    use bytes::Bytes;
    use http::{StatusCode, Uri, uri::Parts};
    use thiserror::Error;
    use wasi::{
        clocks::monotonic_clock::subscribe_duration,
        http::types::{IncomingBody, IncomingRequest, Method, Scheme},
        io::{poll::poll, streams::StreamError},
    };

    /// Converts a WASI Preview 2 request into an `http` request while enforcing
    /// the configured body limit.
    ///
    /// # Errors
    ///
    /// Returns [`RequestError::Policy`] if the body breaches
    /// `max_body_size`, and the conversion variants of
    /// [`RequestError`] if the method, scheme, or headers are unusable.
    pub fn from_wasi_request(
        request: IncomingRequest,
        max_body_size: usize,
    ) -> Result<http::Request<Bytes>, RequestError> {
        from_wasi_request_with_deadline(request, max_body_size, None)
    }

    /// Converts a WASI Preview 2 request, optionally abandoning a body that
    /// takes longer than `timeout_ns` to arrive in full.
    ///
    /// Without a budget this blocks on the input stream exactly as before.
    /// With one, the stream and a monotonic timer are polled together, so a
    /// client that stalls or trickles cannot hold the instance indefinitely.
    /// The budget spans the whole body rather than the gap between chunks, so
    /// a client feeding one byte per interval is bounded too.
    pub fn from_wasi_request_with_deadline(
        request: IncomingRequest,
        max_body_size: usize,
        timeout_ns: Option<u64>,
    ) -> Result<http::Request<Bytes>, RequestError> {
        let parts = request_parts(&request)?;
        let body = collect_wasi_body(
            request,
            &parts.headers,
            max_body_size,
            timeout_ns,
        )?;
        Ok(http::Request::from_parts(parts, body))
    }

    /// Collects the incoming body after [`request_parts`] has already run.
    ///
    /// The handler calls this so conversion of method, URI, and headers
    /// happens once. [`from_wasi_request_with_deadline`] still composes both
    /// steps for callers that want a complete `http::Request`.
    ///
    /// GET and HEAD skip `consume` / `stream` / `finish`. Those methods are
    /// specified without a body, and opening the stream is host work that
    /// never produces bytes here.
    ///
    /// # Errors
    ///
    /// Returns [`RequestError::Policy`] if the declared length is unusable
    /// or over the limit, [`RequestError::BodyTooLarge`] if the collected
    /// body exceeds the limit, [`RequestError::BodyReadTimeout`] if the
    /// budget expires, and the body-resource variants if the stream cannot
    /// be opened or read.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "owns the host resource handle; dropping it releases it"
    )]
    pub fn collect_wasi_body(
        request: IncomingRequest,
        headers: &http::HeaderMap,
        max_body_size: usize,
        timeout_ns: Option<u64>,
    ) -> Result<Bytes, RequestError> {
        let declared =
            crate::handler::validate_content_length(headers, max_body_size)?;

        if matches!(request.method(), Method::Get | Method::Head) {
            return Ok(Bytes::new());
        }

        let incoming_body = request
            .consume()
            .map_err(|()| RequestError::BodyAlreadyConsumed)?;
        let Ok(body_stream) = incoming_body.stream() else {
            IncomingBody::finish(incoming_body);
            return Err(RequestError::BodyStreamUnavailable);
        };
        // One timer for the whole body. Subscribing once rather than per
        // chunk is what makes this a total budget instead of an idle one: a
        // client feeding a byte at a time can never refresh it.
        let deadline = timeout_ns.map(subscribe_duration);
        let readable = deadline.as_ref().map(|_| body_stream.subscribe());

        let mut body = Vec::new();
        let mut consecutive_empty = 0_u32;
        let collected = loop {
            if let Some(deadline) = &deadline
                && deadline.ready()
            {
                break Err(RequestError::BodyReadTimeout(
                    timeout_ns.unwrap_or_default(),
                ));
            }

            // With no budget this is the original blocking read. With one, the
            // non-blocking read plus a two-way poll lets the timer win a race
            // that `blocking_read` would otherwise never return from.
            let read = match (&deadline, &readable) {
                (Some(deadline), Some(readable)) => {
                    match body_stream.read(crate::CHUNK_BYTE_SIZE as u64) {
                        Ok(data) if data.is_empty() => {
                            if let Err(error) =
                                note_empty_body_read(&mut consecutive_empty)
                            {
                                break Err(error);
                            }
                            let ready = poll(&[readable, deadline]);
                            if ready.contains(&1) {
                                break Err(RequestError::BodyReadTimeout(
                                    timeout_ns.unwrap_or_default(),
                                ));
                            }
                            continue;
                        }
                        other => other,
                    }
                }
                _ => body_stream.blocking_read(crate::CHUNK_BYTE_SIZE as u64),
            };

            match read {
                Err(StreamError::Closed) => break Ok(()),
                Err(error @ StreamError::LastOperationFailed(_)) => {
                    break Err(error.into());
                }
                Ok(data) => {
                    reset_empty_body_reads(&mut consecutive_empty);
                    if body.capacity() == 0
                        && let Some(declared) = declared
                    {
                        body.reserve(declared.min(max_body_size));
                    }
                    if body.len().saturating_add(data.len()) > max_body_size {
                        break Err(RequestError::BodyTooLarge(max_body_size));
                    }
                    body.extend(data);
                }
            }
        };
        drop(readable);
        drop(deadline);

        drop(body_stream);
        IncomingBody::finish(incoming_body);
        collected?;
        if !declared_length_matches(declared, body.len()) {
            return Err(RequestError::Policy(
                crate::handler::RequestPolicyError::InvalidContentLength,
            ));
        }
        Ok(Bytes::from(body))
    }

    /// Consecutive empty non-blocking reads before the stream is treated as
    /// unavailable rather than polled until the deadline.
    const EMPTY_BODY_READ_CAP: u32 = 64;

    fn note_empty_body_read(
        consecutive_empty: &mut u32,
    ) -> Result<(), RequestError> {
        *consecutive_empty = consecutive_empty.saturating_add(1);
        if *consecutive_empty >= EMPTY_BODY_READ_CAP {
            Err(RequestError::BodyStreamUnavailable)
        } else {
            Ok(())
        }
    }

    fn reset_empty_body_reads(consecutive_empty: &mut u32) {
        *consecutive_empty = 0;
    }

    const fn declared_length_matches(
        declared: Option<usize>,
        actual: usize,
    ) -> bool {
        match declared {
            None => true,
            Some(declared) => declared == actual,
        }
    }

    pub(crate) fn request_parts(
        request: &IncomingRequest,
    ) -> Result<http::request::Parts, RequestError> {
        let mut builder = http::Request::builder();
        let headers = request.headers();
        for (name, value) in headers.entries() {
            builder = builder.header(name, value);
        }
        drop(headers);

        let mut uri = Parts::default();
        uri.scheme = request.scheme().map(scheme_wasi_to_http).transpose()?;
        uri.authority = request
            .authority()
            .map(|authority| {
                http::uri::Authority::from_maybe_shared(authority.into_bytes())
            })
            .transpose()
            .map_err(http::Error::from)?;
        uri.path_and_query = request
            .path_with_query()
            .map(|path| {
                http::uri::PathAndQuery::from_maybe_shared(path.into_bytes())
            })
            .transpose()
            .map_err(http::Error::from)?;

        let request = builder
            .method(method_wasi_to_http(request.method())?)
            .uri(Uri::from_parts(uri).map_err(http::Error::from)?)
            .body(())?;
        Ok(request.into_parts().0)
    }

    /// Errors converting a WASI Preview 2 incoming request.
    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum RequestError {
        /// Standard HTTP request construction failed.
        #[error("failed to convert WASI bindings to HTTP types")]
        Http(#[from] http::Error),
        /// The WASI body stream failed.
        #[error("error while processing the WASI HTTP body stream")]
        WasiIo(#[from] StreamError),
        /// The request body exceeded the configured limit.
        #[error("request body exceeds limit of {0} bytes")]
        BodyTooLarge(usize),
        /// The request body did not arrive in full within its budget.
        #[error("request body exceeded its {0} ns read budget")]
        BodyReadTimeout(u64),
        /// The incoming body resource had already been consumed.
        #[error("incoming request body was already consumed")]
        BodyAlreadyConsumed,
        /// A body stream could not be opened.
        #[error("incoming request body stream is unavailable")]
        BodyStreamUnavailable,
        /// Header collection was unavailable while building the request.
        ///
        /// Retained for patch compatibility against 0.4.1; current construction
        /// maps header failures through [`Self::Http`] instead.
        #[error("request headers are unavailable")]
        InvalidHeaders,
        /// Request policy validation failed.
        #[error(transparent)]
        Policy(#[from] crate::handler::RequestPolicyError),
    }

    impl RequestError {
        /// Status to send the client instead of failing the handler.
        ///
        /// Malformed method, URI, headers, or a body I/O failure from a
        /// broken client become 400. A consumed or unavailable body stream is
        /// a host/guest bug and stays an error.
        #[must_use]
        pub const fn client_status(&self) -> Option<StatusCode> {
            match self {
                Self::Http(_) | Self::WasiIo(_) => {
                    Some(StatusCode::BAD_REQUEST)
                }
                Self::BodyAlreadyConsumed
                | Self::BodyStreamUnavailable
                | Self::BodyTooLarge(_)
                | Self::BodyReadTimeout(_)
                | Self::InvalidHeaders
                | Self::Policy(_) => None,
            }
        }
    }

    /// Converts a WASI Preview 2 method into an `http` method.
    ///
    /// # Errors
    ///
    /// Returns an error if an extension method is not a valid HTTP token.
    pub fn method_wasi_to_http(
        value: Method,
    ) -> Result<http::Method, http::Error> {
        match value {
            Method::Connect => Ok(http::Method::CONNECT),
            Method::Delete => Ok(http::Method::DELETE),
            Method::Get => Ok(http::Method::GET),
            Method::Head => Ok(http::Method::HEAD),
            Method::Options => Ok(http::Method::OPTIONS),
            Method::Patch => Ok(http::Method::PATCH),
            Method::Post => Ok(http::Method::POST),
            Method::Put => Ok(http::Method::PUT),
            Method::Trace => Ok(http::Method::TRACE),
            Method::Other(method) => {
                http::Method::from_bytes(method.as_bytes())
                    .map_err(http::Error::from)
            }
        }
    }

    /// Converts a WASI Preview 2 scheme into an `http` scheme.
    ///
    /// # Errors
    ///
    /// Returns an error if an extension scheme is not a valid URI scheme.
    pub fn scheme_wasi_to_http(
        value: Scheme,
    ) -> Result<http::uri::Scheme, http::Error> {
        match value {
            Scheme::Http => Ok(http::uri::Scheme::HTTP),
            Scheme::Https => Ok(http::uri::Scheme::HTTPS),
            Scheme::Other(scheme) => {
                http::uri::Scheme::try_from(scheme.as_bytes())
                    .map_err(http::Error::from)
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Method, Scheme, method_wasi_to_http, scheme_wasi_to_http};

        #[test]
        fn standard_methods_map_one_to_one() {
            let mapped = [
                (Method::Connect, http::Method::CONNECT),
                (Method::Delete, http::Method::DELETE),
                (Method::Get, http::Method::GET),
                (Method::Head, http::Method::HEAD),
                (Method::Options, http::Method::OPTIONS),
                (Method::Patch, http::Method::PATCH),
                (Method::Post, http::Method::POST),
                (Method::Put, http::Method::PUT),
                (Method::Trace, http::Method::TRACE),
            ];

            for (wasi, expected) in mapped {
                assert_eq!(
                    method_wasi_to_http(wasi).expect("standard method"),
                    expected
                );
            }
        }

        #[test]
        fn extension_methods_are_preserved() {
            assert_eq!(
                method_wasi_to_http(Method::Other("PROPFIND".to_owned()))
                    .expect("extension method"),
                "PROPFIND"
            );
        }

        #[test]
        fn methods_with_invalid_tokens_are_rejected() {
            for method in ["", "GET POST", "GET\r\nX: 1"] {
                assert!(
                    method_wasi_to_http(Method::Other(method.to_owned()))
                        .is_err(),
                    "method `{method}` should be rejected"
                );
            }
        }

        #[test]
        fn standard_schemes_map_one_to_one() {
            assert_eq!(
                scheme_wasi_to_http(Scheme::Http).expect("http scheme"),
                http::uri::Scheme::HTTP
            );
            assert_eq!(
                scheme_wasi_to_http(Scheme::Https).expect("https scheme"),
                http::uri::Scheme::HTTPS
            );
        }

        #[test]
        fn extension_schemes_are_preserved_and_validated() {
            assert_eq!(
                scheme_wasi_to_http(Scheme::Other("ws".to_owned()))
                    .expect("extension scheme")
                    .as_str(),
                "ws"
            );
            assert!(
                scheme_wasi_to_http(Scheme::Other("not a scheme".to_owned()))
                    .is_err()
            );
        }

        #[test]
        fn empty_body_reads_are_capped_before_the_stream_spins() {
            let mut consecutive_empty = 0;
            for _ in 1..super::EMPTY_BODY_READ_CAP {
                super::note_empty_body_read(&mut consecutive_empty)
                    .expect("empty reads below the cap should retry");
            }
            assert!(
                matches!(
                    super::note_empty_body_read(&mut consecutive_empty),
                    Err(super::RequestError::BodyStreamUnavailable)
                ),
                "the {cap}th empty read must fail",
                cap = super::EMPTY_BODY_READ_CAP
            );
        }

        #[test]
        fn a_byte_resets_the_empty_body_read_cap() {
            let mut consecutive_empty = super::EMPTY_BODY_READ_CAP - 1;
            super::reset_empty_body_reads(&mut consecutive_empty);
            super::note_empty_body_read(&mut consecutive_empty)
                .expect("a reset counter must accept another empty read");
            assert_eq!(consecutive_empty, 1);
        }

        #[test]
        fn http_and_wasi_io_errors_are_client_failures() {
            let http = http::Request::builder()
                .header("\0", "x")
                .body(())
                .expect_err("nul is not a header name");
            assert_eq!(
                super::RequestError::from(http).client_status(),
                Some(http::StatusCode::BAD_REQUEST)
            );
            assert_eq!(
                super::RequestError::BodyAlreadyConsumed.client_status(),
                None
            );
            assert_eq!(
                super::RequestError::BodyStreamUnavailable.client_status(),
                None
            );
        }

        #[test]
        fn a_declared_content_length_must_match_the_collected_body() {
            assert!(super::declared_length_matches(None, 0));
            assert!(super::declared_length_matches(Some(3), 3));
            assert!(!super::declared_length_matches(Some(5), 3));
        }
    }
}
