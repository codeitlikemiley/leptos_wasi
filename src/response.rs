//! Platform-neutral response bodies and Leptos response options.

use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use parking_lot::RwLock;
use server_fn::response::generic::Body as ServerFnBody;
use std::{pin::Pin, sync::Arc};
#[cfg(feature = "wasip2")]
use thiserror::Error;
#[cfg(feature = "wasip2")]
use wasi::http::types::{HeaderError, Headers};

use crate::integration::ExtendResponse;

/// Represents a platform-agnostic HTTP response wrapped with a WASI-compatible [`Body`].
///
/// It supports both [`Body::Sync`] (sending the whole response at once) and
/// [`Body::Async`] (streaming the response).
///
/// # Example
///
/// ```rust
/// use bytes::Bytes;
/// use leptos_wasi::response::{Body, Response};
///
/// let http_res = http::Response::builder()
///     .status(200)
///     .body(Body::Sync(Bytes::from("Hello WASI!")))
///     .unwrap();
/// let response = Response(http_res);
/// ```
pub struct Response(pub http::Response<Body>);

#[cfg(feature = "wasip2")]
impl Response {
    /// Converts the response headers to the WASI Preview 2 native `Headers` type.
    ///
    /// # Errors
    ///
    /// Returns [`ResponseError`] if a header name or value is rejected by
    /// the host.
    pub fn headers(&self) -> Result<Headers, ResponseError> {
        let headers = Headers::new();
        for (name, value) in self.0.headers() {
            headers.append(name.as_ref(), &Vec::from(value.as_bytes()))?;
        }
        Ok(headers)
    }
}

impl<T> From<http::Response<T>> for Response
where
    T: Into<Body>,
{
    fn from(value: http::Response<T>) -> Self {
        Self(value.map(Into::into))
    }
}

/// The response body representation, which supports both synchronous buffer transfers and asynchronous streams.
///
/// # Example
///
/// ```rust
/// use bytes::Bytes;
/// use leptos_wasi::response::Body;
///
/// // Synchronous body
/// let sync_body = Body::Sync(Bytes::from("Sync response data"));
/// ```
pub enum Body {
    /// The response body will be written synchronously.
    Sync(Bytes),

    /// The response body will be written asynchronously,
    /// this execution model is also known as
    /// "streaming".
    Async(
        Pin<
            Box<
                dyn Stream<Item = Result<Bytes, throw_error::Error>>
                    + Send
                    + 'static,
            >,
        >,
    ),
}

impl From<ServerFnBody> for Body {
    fn from(value: ServerFnBody) -> Self {
        match value {
            ServerFnBody::Sync(data) => Self::Sync(data),
            ServerFnBody::Async(stream) => Self::Async(stream),
        }
    }
}

impl From<Bytes> for Body {
    fn from(value: Bytes) -> Self {
        Self::Sync(value)
    }
}

// Support for different server backend body types

// For axum backend, the body type is typically http_body_util::combinators::BoxBody
// with specific error types
impl
    From<
        http_body_util::combinators::BoxBody<
            bytes::Bytes,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    > for Body
{
    fn from(
        value: http_body_util::combinators::BoxBody<
            bytes::Bytes,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    ) -> Self {
        use http_body_util::BodyExt;

        // Convert BoxBody to async stream of bytes
        let stream = async_stream::stream! {
            let mut body = value;
            while let Some(frame_result) = body.frame().await {
                match frame_result {
                    Ok(frame) => {
                        if let Some(data) = frame.data_ref() {
                            yield Ok(data.clone());
                        }
                    }
                    Err(e) => {
                        // Convert Box<dyn Error + Send + Sync> to throw_error::Error
                        // We use std::io::Error as an intermediate since boxed trait objects
                        // don't implement std::error::Error themselves
                        yield Err(throw_error::Error::from(std::io::Error::other(
                            format!("Body frame error: {e}")
                        )));
                    }
                }
            }
        };

        Self::Async(Box::pin(stream))
    }
}

// Handle the axum_core body type which is used by server functions with axum backend
// This corresponds to leptos::server_fn::axum::body::Body in the error messages
impl From<axum_core::body::Body> for Body {
    fn from(value: axum_core::body::Body) -> Self {
        use http_body_util::BodyExt;

        // Convert axum_core::Body to async stream of bytes
        let stream = async_stream::stream! {
            let mut body = value;
            while let Some(frame_result) = body.frame().await {
                match frame_result {
                    Ok(frame) => {
                        if let Some(data) = frame.data_ref() {
                            yield Ok(data.clone());
                        }
                    }
                    Err(e) => {
                        // Convert axum_core::Error to throw_error::Error
                        // axum_core::Error implements std::error::Error so this works directly
                        yield Err(throw_error::Error::from(e));
                    }
                }
            }
        };

        Self::Async(Box::pin(stream))
    }
}

/// This struct lets you define headers and override the status of the Response from an Element or a Server Function
/// Typically contained inside of a `ResponseOptions`. Setting this is useful for cookies and custom responses.
/// Extracted HTTP parts (headers and status code) for configuring response options.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ResponseParts {
    headers: HeaderMap,
    status: Option<StatusCode>,
}

impl ResponseParts {
    /// Returns response headers collected so far.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns mutable access to the response headers.
    #[must_use]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// Returns the configured status override, if present.
    #[must_use]
    pub const fn status(&self) -> Option<StatusCode> {
        self.status
    }

    /// Sets the response status override.
    pub fn set_status(&mut self, status: StatusCode) {
        self.status = Some(status);
    }

    /// Clears the response status override.
    pub fn clear_status(&mut self) {
        self.status = None;
    }

    /// Inserts a header, replacing an existing value with the same name.
    pub fn insert_header(&mut self, key: HeaderName, value: HeaderValue) {
        self.headers.insert(key, value);
    }

    /// Appends a header without removing existing values with the same name.
    pub fn append_header(&mut self, key: HeaderName, value: HeaderValue) {
        self.headers.append(key, value);
    }
}

/// Allows you to override details of the HTTP response like the status code and add Headers/Cookies.
#[derive(Debug, Clone, Default)]
pub struct ResponseOptions(Arc<RwLock<ResponseParts>>);

impl ResponseOptions {
    /// A simpler way to overwrite the contents of `ResponseOptions` with a new `ResponseParts`.
    #[inline]
    pub fn overwrite(&self, parts: ResponseParts) {
        *self.0.write() = parts;
    }
    /// Set the status of the returned Response.
    #[inline]
    pub fn set_status(&self, status: StatusCode) {
        self.0.write().status = Some(status);
    }
    /// Insert a header, overwriting any previous value with the same key.
    #[inline]
    pub fn insert_header(&self, key: HeaderName, value: HeaderValue) {
        self.0.write().headers.insert(key, value);
    }
    /// Append a header, leaving any header with the same key intact.
    #[inline]
    pub fn append_header(&self, key: HeaderName, value: HeaderValue) {
        self.0.write().headers.append(key, value);
    }

    /// Returns a copy of the parts collected so far.
    #[cfg(test)]
    #[inline]
    pub(crate) fn snapshot(&self) -> ResponseParts {
        self.0.read().clone()
    }
}

impl ExtendResponse for Response {
    type ResponseOptions = ResponseOptions;

    fn from_stream(
        stream: impl Stream<Item = String> + Send + 'static,
    ) -> Self {
        let stream = stream.map(|data| {
            Result::<Bytes, throw_error::Error>::Ok(Bytes::from(data))
        });

        Self(http::Response::new(Body::Async(Box::pin(stream))))
    }

    fn extend_response(&mut self, opt: &Self::ResponseOptions) {
        let mut opt = opt.0.write();
        if let Some(status_code) = opt.status {
            *self.0.status_mut() = status_code;
        }
        self.0
            .headers_mut()
            .extend(std::mem::take(&mut opt.headers));
    }

    fn set_default_content_type(&mut self, content_type: &str) {
        let headers = self.0.headers_mut();
        if !headers.contains_key(http::header::CONTENT_TYPE)
            && let Ok(content_type) = HeaderValue::from_str(content_type)
        {
            headers.insert(http::header::CONTENT_TYPE, content_type);
        }
    }
}

#[cfg(feature = "wasip2")]
/// Errors that can occur when converting HTTP headers to WASI Preview 2 headers.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ResponseError {
    /// A header name or value was rejected by the WASI host bindings.
    #[error("failed to parse http::Response's headers")]
    WasiHeaders(#[from] HeaderError),
}

#[cfg(feature = "wasip3")]
impl http_body::Body for Body {
    type Data = Bytes;
    type Error = throw_error::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<
        Option<Result<http_body::Frame<Self::Data>, Self::Error>>,
    > {
        match &mut *self {
            Body::Sync(bytes) => {
                if bytes.is_empty() {
                    std::task::Poll::Ready(None)
                } else {
                    let data = std::mem::take(bytes);
                    std::task::Poll::Ready(Some(Ok(http_body::Frame::data(
                        data,
                    ))))
                }
            }
            Body::Async(stream) => match stream.as_mut().poll_next(cx) {
                std::task::Poll::Ready(Some(item)) => std::task::Poll::Ready(
                    Some(item.map(http_body::Frame::data)),
                ),
                std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
                std::task::Poll::Pending => std::task::Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, Body::Sync(bytes) if bytes.is_empty())
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            Body::Sync(bytes) => {
                http_body::SizeHint::with_exact(bytes.len() as u64)
            }
            Body::Async(_) => http_body::SizeHint::default(),
        }
    }
}

#[expect(
    clippy::panic,
    reason = "a failed invariant in a test should abort the test"
)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_server_fn_response_body_converts() {
        let response = http::Response::new(ServerFnBody::Sync(
            Bytes::from_static(b"generic response"),
        ));
        let response: Response = response.into();

        match response.0.into_body() {
            Body::Sync(bytes) => assert_eq!(bytes, "generic response"),
            Body::Async(_) => panic!("synchronous generic body became async"),
        }
    }

    async fn collect(body: Body) -> Result<Vec<u8>, throw_error::Error> {
        let Body::Async(mut stream) = body else {
            panic!("expected a streaming body");
        };
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk?);
        }
        Ok(collected)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn axum_bodies_stream_every_frame() {
        let body: Body =
            axum_core::body::Body::from("axum streaming body").into();

        assert_eq!(
            collect(body).await.expect("body should not error"),
            b"axum streaming body"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn axum_body_errors_surface_as_stream_errors() {
        use http_body_util::BodyExt;

        let failing =
            http_body_util::StreamBody::new(futures::stream::once(async {
                Err::<http_body::Frame<Bytes>, std::io::Error>(
                    std::io::Error::other("upstream failure"),
                )
            }));
        let body: Body =
            axum_core::body::Body::new(failing.map_err(axum_core::Error::new))
                .into();

        let error = collect(body)
            .await
            .expect_err("a failing frame should end the stream with an error");
        assert!(error.to_string().contains("upstream failure"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn boxed_error_bodies_stream_every_frame() {
        use http_body_util::BodyExt;

        let boxed = http_body_util::Full::new(Bytes::from_static(
            b"boxed streaming body",
        ))
        .map_err(|error: std::convert::Infallible| -> Box<
            dyn std::error::Error + Send + Sync,
        > { match error {} })
        .boxed();
        let body: Body = boxed.into();

        assert_eq!(
            collect(body).await.expect("body should not error"),
            b"boxed streaming body"
        );
    }

    #[test]
    fn response_options_apply_status_and_headers_once() {
        let options = ResponseOptions::default();
        options.set_status(StatusCode::IM_A_TEAPOT);
        options.insert_header(
            HeaderName::from_static("x-first"),
            HeaderValue::from_static("1"),
        );
        options.append_header(
            HeaderName::from_static("x-first"),
            HeaderValue::from_static("2"),
        );

        let mut response =
            Response(http::Response::new(Body::Sync(Bytes::new())));
        response.extend_response(&options);

        assert_eq!(response.0.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(
            response
                .0
                .headers()
                .get_all("x-first")
                .iter()
                .collect::<Vec<_>>(),
            ["1", "2"]
        );

        // Applying the same options again must not duplicate the headers.
        let mut second =
            Response(http::Response::new(Body::Sync(Bytes::new())));
        second.extend_response(&options);
        assert!(second.0.headers().is_empty());
    }

    #[test]
    fn overwrite_replaces_previously_collected_parts() {
        let options = ResponseOptions::default();
        options.set_status(StatusCode::IM_A_TEAPOT);
        options.insert_header(
            HeaderName::from_static("x-stale"),
            HeaderValue::from_static("1"),
        );

        let mut parts = ResponseParts::default();
        parts.set_status(StatusCode::CREATED);
        options.overwrite(parts);

        let snapshot = options.snapshot();
        assert_eq!(snapshot.status(), Some(StatusCode::CREATED));
        assert!(snapshot.headers().is_empty());
    }

    #[test]
    fn default_content_type_never_overrides_an_explicit_one() {
        let mut response =
            Response(http::Response::new(Body::Sync(Bytes::new())));
        response.set_default_content_type("text/html; charset=utf-8");
        response.set_default_content_type("application/json");

        assert_eq!(
            response.0.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );
    }

    #[test]
    fn response_parts_status_can_be_cleared() {
        let mut parts = ResponseParts::default();
        parts.set_status(StatusCode::FOUND);
        parts.clear_status();

        assert_eq!(parts.status(), None);
    }
}
