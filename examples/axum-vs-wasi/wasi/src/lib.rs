use leptos_wasi::wasip2::prelude::{
    Handler, IncomingRequest, Mode, RegistrationError, ResponseOutparam,
    RouteTable, init_wasip2_executor,
};
use wasi::{
    exports::wasi::http::incoming_handler::Guest, http::types::ErrorCode,
};

use compare_app::{App, FormTest, GetTest, PostTest, leptos_options, shell};

thread_local! {
    static ROUTES: Result<RouteTable, RegistrationError> =
        RouteTable::discover(App, None, || {});
}

struct LeptosServer;

impl Guest for LeptosServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let executor = match init_wasip2_executor(Mode::Stalled) {
            Ok(executor) => executor,
            Err(error) => {
                eprintln!(
                    "failed to initialize the Preview 2 executor: {error}"
                );
                ResponseOutparam::set(
                    response_out,
                    Err(ErrorCode::InternalError(None)),
                );
                return;
            }
        };

        let site_addr = ([127, 0, 0, 1], 3000).into();
        let leptos_options = leptos_options(site_addr);

        let result = executor.run_until(async move {
            let result: Result<(), Box<dyn std::error::Error>> = async {
                let routes = ROUTES.with(Clone::clone)?;
                Handler::build(request, response_out)?
                    .static_files_handler("/static", serve_static_files)?
                    .with_server_fn::<GetTest>()
                    .with_server_fn::<PostTest>()
                    .with_server_fn::<FormTest>()
                    .generate_routes_from(&routes)?
                    .handle_with_context(
                        move || shell(leptos_options.clone()),
                        || {},
                    )
                    .await?;
                Ok(())
            }
            .await;
            result
        });

        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("request failed: {error}"),
            Err(error) => eprintln!("Preview 2 executor failed: {error}"),
        }
    }
}

fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    if path != "hello.txt" {
        return None;
    }
    std::fs::read("/static/hello.txt")
        .ok()
        .map(|bytes| leptos_wasi::response::Body::Sync(bytes.into()))
}

wasi::http::proxy::export!(LeptosServer with_types_in wasi);

#[cfg(test)]
mod tests {
    use compare_app::App;

    #[test]
    fn route_table_is_valid() {
        leptos_wasi::validate_route_table(App, None, || {})
            .expect("route table should be valid");
    }
}
