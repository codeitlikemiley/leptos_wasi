//! Runtime-specific request conversion helpers.

/// WASI Preview 2 request conversion.
#[cfg(feature = "wasip2")]
pub mod p2 {
    use bytes::Bytes;
    use http::{Uri, uri::Parts};
    use thiserror::Error;
    use wasi::{
        http::types::{IncomingBody, IncomingRequest, Method, Scheme},
        io::streams::StreamError,
    };

    /// Converts a WASI Preview 2 request into an `http` request while enforcing
    /// the configured body limit.
    pub fn from_wasi_request(
        request: IncomingRequest,
        max_body_size: usize,
    ) -> Result<http::Request<Bytes>, RequestError> {
        let parts = request_parts(&request)?;
        super::super::handler::validate_content_length(
            &parts.headers,
            max_body_size,
        )?;

        let incoming_body = request
            .consume()
            .map_err(|()| RequestError::BodyAlreadyConsumed)?;
        let body_stream = match incoming_body.stream() {
            Ok(stream) => stream,
            Err(()) => {
                IncomingBody::finish(incoming_body);
                return Err(RequestError::BodyStreamUnavailable);
            }
        };
        let mut body = Vec::with_capacity(crate::CHUNK_BYTE_SIZE);
        let collected = loop {
            match body_stream.blocking_read(crate::CHUNK_BYTE_SIZE as u64) {
                Err(StreamError::Closed) => break Ok(()),
                Err(error @ StreamError::LastOperationFailed(_)) => {
                    break Err(error.into());
                }
                Ok(data) => {
                    if body.len().saturating_add(data.len()) > max_body_size {
                        break Err(RequestError::BodyTooLarge(max_body_size));
                    }
                    body.extend(data);
                }
            }
        };

        drop(body_stream);
        IncomingBody::finish(incoming_body);
        collected?;
        Ok(http::Request::from_parts(parts, Bytes::from(body)))
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
        /// The incoming body resource had already been consumed.
        #[error("incoming request body was already consumed")]
        BodyAlreadyConsumed,
        /// A body stream could not be opened.
        #[error("incoming request body stream is unavailable")]
        BodyStreamUnavailable,
        /// Header collection was unavailable while building the request.
        #[error("request headers are unavailable")]
        InvalidHeaders,
        /// Request policy validation failed.
        #[error(transparent)]
        Policy(#[from] super::super::handler::RequestPolicyError),
    }

    /// Converts a WASI Preview 2 method into an `http` method.
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
}
