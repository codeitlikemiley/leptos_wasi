use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    location::RequestUrl,
    path,
};
use server_fn::codec::{GetUrl, Json};

#[derive(Clone, Copy)]
pub struct StandardContextsVisible(pub bool);

pub fn shell(options: LeptosOptions) -> impl IntoView {
    let is_islands_router_navigation =
        use_context::<IslandsRouterNavigation>().is_some();

    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <HydrationScripts options=options.clone() root="" />
                <MetaTags />
            </head>
            <body data-islands-router-navigation=is_islands_router_navigation.to_string()>
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let fallback = || view! { "Page not found." }.into_view();

    view! {
        <Stylesheet id="leptos" href="/static/app.css" />
        <Router>
            <main>
                <Routes fallback>
                    <Route path=path!("/ssr/async") ssr=SsrMode::Async view=SsrAsyncView />
                    <Route path=path!("/ssr/in-order") ssr=SsrMode::InOrder view=SsrInOrderView />
                    <Route path=path!("/ssr/out-of-order") ssr=SsrMode::OutOfOrder view=SsrOutOfOrderView />
                    <Route path=path!("/ssr/meta") ssr=SsrMode::Async view=SsrMetaView />
                    <Route path=path!("/ssr/query") ssr=SsrMode::Async view=SsrQueryView />
                    <Route path=path!("/ssr/panic") ssr=SsrMode::Async view=SsrPanicView />
                    <Route path=path!("/static-page") ssr=SsrMode::Async view=StaticPageSsrView />
                    <Route path=path!("/*any") view=NotFound />
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn SsrAsyncView() -> impl IntoView {
    let resource = Resource::new(
        || (),
        |_| async { "Async resource resolved".to_string() },
    );
    view! {
        <div>
            <h1>"Async View"</h1>
            <Suspense fallback=move || view! { <p>"Loading..."</p> }>
                <p>{move || resource.get()}</p>
            </Suspense>
        </div>
    }
}

#[component]
fn SsrInOrderView() -> impl IntoView {
    let resource = Resource::new(
        || (),
        |_| async { "InOrder resource resolved".to_string() },
    );
    view! {
        <div>
            <h1>"InOrder View"</h1>
            <Suspense fallback=move || view! { <p>"Loading..."</p> }>
                <p>{move || resource.get()}</p>
            </Suspense>
        </div>
    }
}

#[component]
fn SsrOutOfOrderView() -> impl IntoView {
    let resource = Resource::new(
        || (),
        |_| async { "OutOfOrder resource resolved".to_string() },
    );
    view! {
        <div>
            <h1>"OutOfOrder View"</h1>
            <Suspense fallback=move || view! { <p>"Loading..."</p> }>
                <p>{move || resource.get()}</p>
            </Suspense>
        </div>
    }
}

#[component]
fn SsrMetaView() -> impl IntoView {
    view! {
        <Title text="Meta Test Title" />
        <Meta name="description" content="Meta Test Description" />
        <div>
            <h1>"Meta View"</h1>
        </div>
    }
}

#[component]
fn SsrQueryView() -> impl IntoView {
    let request_url = use_context::<RequestUrl>()
        .map(|url| url.as_ref().to_string())
        .unwrap_or_else(|| "missing request URL".to_string());
    let standard_contexts_visible = use_context::<StandardContextsVisible>()
        .is_some_and(|visible| visible.0);

    view! {
        <div>
            <p id="request-url">{request_url}</p>
            <p id="standard-contexts-visible">
                {standard_contexts_visible.to_string()}
            </p>
        </div>
    }
}

#[component]
fn SsrPanicView() -> impl IntoView {
    let should_panic = true;
    if should_panic {
        panic!("SsrPanicView-panic");
    }
    view! {
        <div>"Will not render"</div>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    if let Some(resp) = use_context::<leptos_wasi::response::ResponseOptions>()
    {
        resp.set_status(leptos_wasi::prelude::StatusCode::NOT_FOUND);
    }
    view! { <h1>"Not Found"</h1> }
}

// Server functions:
#[server(input = GetUrl, prefix = "/api", endpoint = "get_test")]
pub async fn get_test() -> Result<String, ServerFnError> {
    Ok("GET response".to_string())
}

#[server(
    input = GetUrl,
    prefix = "/api",
    endpoint = "middleware_request_header"
)]
pub async fn middleware_request_header() -> Result<String, ServerFnError> {
    let value = use_context::<http::request::Parts>()
        .and_then(|parts| {
            parts
                .headers
                .get("x-leptos-wasi-middleware-request")
                .cloned()
        })
        .and_then(|value| value.to_str().ok().map(str::to_owned))
        .unwrap_or_else(|| "missing".to_string());
    Ok(value)
}

#[server(input = GetUrl, prefix = "/api", endpoint = "pollable_depth")]
pub async fn pollable_depth() -> Result<usize, ServerFnError> {
    #[cfg(feature = "wasip2")]
    let depth = leptos_wasi::__private::wasip2_pollable_queue_depth();
    #[cfg(not(feature = "wasip2"))]
    let depth = 0;
    Ok(depth)
}

#[server(input = Json, prefix = "/api", endpoint = "post_test")]
pub async fn post_test(msg: String) -> Result<String, ServerFnError> {
    Ok(format!("POST response: {}", msg))
}

type AxumRequest = http::Request<axum_core::body::Body>;
type AxumResponse = http::Response<axum_core::body::Body>;

struct ResponseHeaderLayer {
    value: &'static str,
}

struct ResponseHeaderService {
    inner: server_fn::middleware::BoxedService<AxumRequest, AxumResponse>,
    value: &'static str,
}

impl server_fn::middleware::Layer<AxumRequest, AxumResponse>
    for ResponseHeaderLayer
{
    fn layer(
        &self,
        inner: server_fn::middleware::BoxedService<AxumRequest, AxumResponse>,
    ) -> server_fn::middleware::BoxedService<AxumRequest, AxumResponse> {
        server_fn::middleware::BoxedService::new(
            inner.ser,
            ResponseHeaderService {
                inner,
                value: self.value,
            },
        )
    }
}

impl server_fn::middleware::Service<AxumRequest, AxumResponse>
    for ResponseHeaderService
{
    fn run(
        &mut self,
        request: AxumRequest,
        _serialize_error: fn(
            server_fn::error::ServerFnErrorErr,
        ) -> bytes::Bytes,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AxumResponse> + Send>>
    {
        let future = self.inner.run(request);
        let value = self.value;
        Box::pin(async move {
            let mut response = future.await;
            response.headers_mut().append(
                http::HeaderName::from_static("x-middleware-order"),
                http::HeaderValue::from_static(value),
            );
            response
        })
    }
}

struct AuthenticationLayer;

struct AuthenticationService {
    inner: server_fn::middleware::BoxedService<AxumRequest, AxumResponse>,
}

impl server_fn::middleware::Layer<AxumRequest, AxumResponse>
    for AuthenticationLayer
{
    fn layer(
        &self,
        inner: server_fn::middleware::BoxedService<AxumRequest, AxumResponse>,
    ) -> server_fn::middleware::BoxedService<AxumRequest, AxumResponse> {
        server_fn::middleware::BoxedService::new(
            inner.ser,
            AuthenticationService { inner },
        )
    }
}

impl server_fn::middleware::Service<AxumRequest, AxumResponse>
    for AuthenticationService
{
    fn run(
        &mut self,
        request: AxumRequest,
        _serialize_error: fn(
            server_fn::error::ServerFnErrorErr,
        ) -> bytes::Bytes,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AxumResponse> + Send>>
    {
        let is_authorized = request
            .headers()
            .get("x-test-auth")
            .is_some_and(|value| value == "allowed");
        if is_authorized {
            self.inner.run(request)
        } else {
            Box::pin(async {
                http::Response::builder()
                    .status(http::StatusCode::UNAUTHORIZED)
                    .body(axum_core::body::Body::from(
                        "authentication required",
                    ))
                    .expect("the middleware response is valid")
            })
        }
    }
}

struct ErrorLayer;

struct ErrorService;

impl server_fn::middleware::Layer<AxumRequest, AxumResponse> for ErrorLayer {
    fn layer(
        &self,
        inner: server_fn::middleware::BoxedService<AxumRequest, AxumResponse>,
    ) -> server_fn::middleware::BoxedService<AxumRequest, AxumResponse> {
        server_fn::middleware::BoxedService::new(inner.ser, ErrorService)
    }
}

impl server_fn::middleware::Service<AxumRequest, AxumResponse>
    for ErrorService
{
    fn run(
        &mut self,
        request: AxumRequest,
        serialize_error: fn(server_fn::error::ServerFnErrorErr) -> bytes::Bytes,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AxumResponse> + Send>>
    {
        let path = request.uri().path().to_string();
        let body = serialize_error(
            server_fn::error::ServerFnErrorErr::MiddlewareError(
                "intentional middleware failure".to_string(),
            ),
        );
        Box::pin(async move {
            <AxumResponse as server_fn::response::Res>::error_response(
                &path, body,
            )
        })
    }
}

macro_rules! impl_middleware_test_server_fn {
    ($type:ty, $path:literal, $body:literal, $middlewares:expr) => {
        impl server_fn::ServerFn for $type {
            const PATH: &'static str = $path;
            type Client = <PostTest as server_fn::ServerFn>::Client;
            type Server = <PostTest as server_fn::ServerFn>::Server;
            type Protocol = <PostTest as server_fn::ServerFn>::Protocol;
            type Output = String;
            type Error = server_fn::ServerFnError;
            type InputStreamError = server_fn::ServerFnError;
            type OutputStreamError = server_fn::ServerFnError;

            fn middlewares() -> Vec<
                std::sync::Arc<
                    dyn server_fn::middleware::Layer<AxumRequest, AxumResponse>,
                >,
            > {
                $middlewares
            }

            async fn run_body(self) -> Result<Self::Output, Self::Error> {
                Ok($body.to_string())
            }
        }
    };
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct MiddlewareHeaderTest {}

impl_middleware_test_server_fn!(
    MiddlewareHeaderTest,
    "/api/middleware_header_test",
    "middleware response",
    vec![
        std::sync::Arc::new(ResponseHeaderLayer { value: "first" }),
        std::sync::Arc::new(ResponseHeaderLayer { value: "second" }),
    ]
);

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct AuthTest {}

impl_middleware_test_server_fn!(
    AuthTest,
    "/api/auth_test",
    "authenticated response",
    vec![std::sync::Arc::new(AuthenticationLayer)]
);

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct MiddlewareErrorTest {}

impl_middleware_test_server_fn!(
    MiddlewareErrorTest,
    "/api/middleware_error_test",
    "unreachable middleware response",
    vec![std::sync::Arc::new(ErrorLayer)]
);

#[server(input = Json, prefix = "/api", endpoint = "custom_test")]
pub async fn custom_test() -> Result<String, ServerFnError> {
    Ok("Custom response".to_string())
}

#[server(input = Json, prefix = "/api", endpoint = "large_body_test")]
pub async fn large_body_test(data: String) -> Result<usize, ServerFnError> {
    Ok(data.len())
}

#[server(input = Json, prefix = "/api", endpoint = "panic_test")]
pub async fn panic_test() -> Result<String, ServerFnError> {
    panic!("test-panic-endpoint");
}

#[server(prefix = "/api", endpoint = "form_submit_test")]
pub async fn form_submit_test() -> Result<(), ServerFnError> {
    // Does not set Location, returns Ok
    Ok(())
}

#[component]
pub fn StaticPageSsrView() -> impl IntoView {
    view! { <h1>"Static Page SSR View"</h1> }
}

#[server(input = Json, prefix = "/api", endpoint = "malformed_redirect_test")]
pub async fn malformed_redirect_test() -> Result<(), ServerFnError> {
    leptos_wasi::utils::redirect("/target-page\r\nLocation: http://evil.com");
    Ok(())
}
