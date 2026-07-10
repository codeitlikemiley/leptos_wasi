use crate::app::{
    App, AuthTest, CustomTest, FormSubmitTest, GetTest, LargeBodyTest,
    MalformedRedirectTest, MiddlewareErrorTest, MiddlewareHeaderTest,
    PanicTest, PollableDepth, PostTest, StandardContextsVisible, shell,
};
use leptos::{config::get_configuration, prelude::*};
use leptos_router::location::RequestUrl;

const TEST_MAX_REQUEST_BODY_SIZE: usize = 64 * 1024;

fn handler_config() -> leptos_wasi::HandlerConfig {
    leptos_wasi::HandlerConfig::default()
        .with_max_request_body_size(TEST_MAX_REQUEST_BODY_SIZE)
}

fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    if path == "delayed.stream" {
        return Some(delayed_stream());
    }
    if path == "failing.stream" {
        let stream = futures::stream::iter([
            Ok(bytes::Bytes::from_static(b"first-frame\n")),
            Err(throw_error::Error::from(std::io::Error::other(
                "intentional response stream failure",
            ))),
        ]);
        return Some(leptos_wasi::response::Body::Async(Box::pin(stream)));
    }

    let file_path = format!("/static/{path}");
    std::fs::read(file_path)
        .ok()
        .map(|bytes| leptos_wasi::response::Body::Sync(bytes.into()))
}

#[cfg(all(feature = "wasip2", not(feature = "wasip3")))]
fn delayed_stream() -> leptos_wasi::response::Body {
    use futures::StreamExt;

    let first = futures::stream::once(async {
        Ok::<_, throw_error::Error>(bytes::Bytes::from_static(
            b"first-frame\n",
        ))
    });
    let second = futures::stream::once(async {
        leptos_wasi::wasip2::sleep(500_000_000)
            .await
            .map_err(|error| {
                throw_error::Error::from(std::io::Error::other(
                    error.to_string(),
                ))
            })?;
        Ok::<_, throw_error::Error>(bytes::Bytes::from_static(
            b"second-frame\n",
        ))
    });
    leptos_wasi::response::Body::Async(Box::pin(first.chain(second)))
}

#[cfg(feature = "wasip3")]
fn delayed_stream() -> leptos_wasi::response::Body {
    use futures::StreamExt;

    let first = futures::stream::once(async {
        Ok::<_, throw_error::Error>(bytes::Bytes::from_static(
            b"first-frame\n",
        ))
    });
    let second = futures::stream::once(async {
        wasip3::clocks::monotonic_clock::wait_for(500_000_000).await;
        Ok::<_, throw_error::Error>(bytes::Bytes::from_static(
            b"second-frame\n",
        ))
    });
    leptos_wasi::response::Body::Async(Box::pin(first.chain(second)))
}

fn provide_test_context() {
    let standard_contexts_visible = use_context::<RequestUrl>().is_some()
        && use_context::<http::request::Parts>().is_some()
        && use_context::<leptos_wasi::response::ResponseOptions>().is_some();
    provide_context(StandardContextsVisible(standard_contexts_visible));
}

#[cfg(feature = "wasip3")]
mod preview3 {
    use leptos_wasi::wasip3::prelude::{Handler, init_wasip3_spawner};

    use super::*;

    struct LeptosServer;

    impl wasip3::exports::http::handler::Guest for LeptosServer {
        async fn handle(
            request: wasip3::http::types::Request,
        ) -> Result<wasip3::http::types::Response, wasip3::http::types::ErrorCode>
        {
            init_wasip3_spawner().map_err(internal_error)?;

            let conf = get_configuration(None).map_err(internal_error)?;
            let leptos_options = conf.leptos_options;
            let request = wasip3::http_compat::http_from_wasi_request(request)?;

            let handler = Handler::build_with_config(request, handler_config())
                .await
                .map_err(internal_error)?
                .static_files_handler("/static", serve_static_files)
                .map_err(internal_error)?
                .with_server_fn::<GetTest>()
                .with_server_fn::<PollableDepth>()
                .with_server_fn::<PostTest>()
                .with_server_fn::<CustomTest>()
                .with_server_fn::<MiddlewareHeaderTest>()
                .with_server_fn::<AuthTest>()
                .with_server_fn::<MiddlewareErrorTest>()
                .with_server_fn::<LargeBodyTest>()
                .with_server_fn::<PanicTest>()
                .with_server_fn::<FormSubmitTest>()
                .with_server_fn::<MalformedRedirectTest>()
                .generate_routes(App)
                .map_err(internal_error)?;

            handler
                .handle_with_context(
                    move || shell(leptos_options.clone()),
                    provide_test_context,
                )
                .await
                .map_err(internal_error)
        }
    }

    fn internal_error(
        error: impl std::fmt::Display,
    ) -> wasip3::http::types::ErrorCode {
        eprintln!("test application error: {error}");
        wasip3::http::types::ErrorCode::InternalError(None)
    }

    wasip3::http::service::export!(LeptosServer);
}

#[cfg(all(feature = "wasip2", not(feature = "wasip3")))]
mod preview2 {
    use leptos_wasi::wasip2::prelude::{Handler, Mode, init_wasip2_executor};
    use wasi::{
        exports::wasi::http::incoming_handler::{
            Guest, IncomingRequest, ResponseOutparam,
        },
        http::types::ErrorCode,
    };

    use super::*;

    struct LeptosServer;

    impl Guest for LeptosServer {
        fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
            let executor = match init_wasip2_executor(Mode::Stalled) {
                Ok(executor) => executor,
                Err(error) => {
                    eprintln!("failed to initialize test executor: {error}");
                    ResponseOutparam::set(
                        response_out,
                        Err(ErrorCode::InternalError(None)),
                    );
                    return;
                }
            };

            let conf = match get_configuration(None) {
                Ok(conf) => conf,
                Err(error) => {
                    eprintln!("failed to load Leptos configuration: {error}");
                    ResponseOutparam::set(
                        response_out,
                        Err(ErrorCode::InternalError(None)),
                    );
                    return;
                }
            };
            let leptos_options = conf.leptos_options;

            let result = executor.run_until(async move {
                let result: Result<(), Box<dyn std::error::Error>> = async {
                    let handler = Handler::build_with_config(
                        request,
                        response_out,
                        handler_config(),
                    )?
                    .static_files_handler("/static", serve_static_files)?
                    .with_server_fn::<GetTest>()
                    .with_server_fn::<PollableDepth>()
                    .with_server_fn::<PostTest>()
                    .with_server_fn::<CustomTest>()
                    .with_server_fn::<MiddlewareHeaderTest>()
                    .with_server_fn::<AuthTest>()
                    .with_server_fn::<MiddlewareErrorTest>()
                    .with_server_fn::<LargeBodyTest>()
                    .with_server_fn::<PanicTest>()
                    .with_server_fn::<FormSubmitTest>()
                    .with_server_fn::<MalformedRedirectTest>()
                    .generate_routes(App)?;

                    handler
                        .handle_with_context(
                            move || shell(leptos_options.clone()),
                            provide_test_context,
                        )
                        .await?;
                    Ok(())
                }
                .await;
                result
            });

            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    eprintln!("test request failed: {error}");
                }
                Err(error) => {
                    eprintln!("test executor failed: {error}");
                }
            }
        }
    }

    wasi::http::proxy::export!(LeptosServer with_types_in wasi);
}
