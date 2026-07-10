#![recursion_limit = "256"]

#[path = "../../../examples/counter/src/app.rs"]
mod app;

use std::sync::{Arc, OnceLock};

use app::{App, IncrementCount, shell};
use leptos::{config::get_configuration, prelude::use_context};
use leptos_wasi::wasip3::prelude::{Handler, init_wasip3_spawner};
use leptos_wasi_authz::{
    RequireAuthLayer, ResponseStatusSink, authorize_current,
    provide_auth_context, provide_response_status_sink,
};
use server_fn::{ServerFn, ServerFnError};
use wasi_authz_client::{
    AuthzenClient, AuthzenEndpoint, BearerAuthTransport,
    wasip3::Wasip3Transport,
};
use wasi_authz_contract::{
    Action, AttributeNameV1, AttributeProvenanceV1, AttributeStringV1,
    AttributeV1, AttributeValueV1, AttributesV1, Resource,
};
use wasip3::http::types::{ErrorCode, Request, Response};

const SERVICE_ID: &str = "leptos-wasi-counter";
const EXPECTED_AUDIENCE: &str = "api://leptos-wasi-counter";
const CEDAR_ENDPOINT_ENV: &str = "WASI_AUTHZ_CEDAR_ENDPOINT";
const CEDAR_BEARER_ENV: &str = "WASI_AUTHZ_PDP_BEARER_TOKEN";
const SPICEDB_ENDPOINT_ENV: &str = "WASI_AUTHZ_SPICEDB_ENDPOINT";
const SPICEDB_BEARER_ENV: &str = "WASI_AUTHZ_SPICEDB_PDP_BEARER_TOKEN";

type AxumRequest = http::Request<axum_core::body::Body>;
type AxumResponse = http::Response<axum_core::body::Body>;
type AuthorizationLayer = RequireAuthLayer;

static AUTHORIZATION_LAYER: OnceLock<AuthorizationLayer> = OnceLock::new();
static AUTHORIZATION_PROVIDER: OnceLock<
    Result<AuthorizationProviders, AuthorizationInitializationError>,
> = OnceLock::new();

type AuthorizationClient = AuthzenClient<BearerAuthTransport<Wasip3Transport>>;

struct AuthorizationProviders {
    cedar: AuthorizationClient,
    spicedb: AuthorizationClient,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ProtectedIncrementCount {
    current: u32,
}

impl ServerFn for ProtectedIncrementCount {
    const PATH: &'static str = <IncrementCount as ServerFn>::PATH;
    type Client = <IncrementCount as ServerFn>::Client;
    type Server = <IncrementCount as ServerFn>::Server;
    type Protocol = <IncrementCount as ServerFn>::Protocol;
    type Output = u32;
    type Error = ServerFnError;
    type InputStreamError = ServerFnError;
    type OutputStreamError = ServerFnError;

    fn middlewares()
    -> Vec<Arc<dyn server_fn::middleware::Layer<AxumRequest, AxumResponse>>>
    {
        AUTHORIZATION_LAYER
            .get()
            .cloned()
            .map(|layer| {
                Arc::new(layer)
                    as Arc<
                        dyn server_fn::middleware::Layer<
                                AxumRequest,
                                AxumResponse,
                            >,
                    >
            })
            .into_iter()
            .collect()
    }

    async fn run_body(self) -> Result<Self::Output, Self::Error> {
        let action = Action::new("counter.increment").map_err(|_| {
            configuration_error("invalid authorization action configuration")
        })?;
        let resource = counter_resource()?;
        let provider = AUTHORIZATION_PROVIDER
            .get()
            .and_then(|provider| provider.as_ref().ok())
            .ok_or_else(|| {
                configuration_error("authorization provider is unavailable")
            })?;
        // Cedar and SpiceDB are independent policy checks. Running them
        // concurrently removes one serial provider round-trip while keeping
        // both decisions mandatory before the mutation.
        let cedar = authorize_current(&provider.cedar, action.clone(), resource.clone());
        let spicedb = authorize_current(&provider.spicedb, action, resource);
        let (cedar, spicedb) = futures::join!(cedar, spicedb);
        if cedar.is_err() {
            diagnostic_stage("terminal_cedar");
        }
        if spicedb.is_err() {
            diagnostic_stage("terminal_spicedb");
        }
        cedar?;
        spicedb?;
        self.current.checked_add(1).ok_or_else(|| {
            ServerFnError::ServerError(
                "counter reached its maximum value".to_string(),
            )
        })
    }
}

struct LeptosServer;

impl wasip3::exports::http::handler::Guest for LeptosServer {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        init_wasip3_spawner().map_err(internal_error)?;
        initialize_authorization().map_err(internal_error)?;

        let conf = get_configuration(None).map_err(internal_error)?;
        let leptos_options = conf.leptos_options;
        let request = wasip3::http_compat::http_from_wasi_request(request)?;

        Handler::build(request)
            .await
            .map_err(internal_error)?
            .static_files_handler("/pkg", serve_static_files)
            .map_err(internal_error)?
            .with_server_fn::<ProtectedIncrementCount>()
            .generate_routes(App)
            .map_err(internal_error)?
            .handle_with_context(
                move || shell(leptos_options.clone()),
                provide_fixture_context,
            )
            .await
            .map_err(internal_error)
    }
}

fn initialize_authorization() -> Result<(), AuthorizationInitializationError> {
    if AUTHORIZATION_LAYER.get().is_some()
        && AUTHORIZATION_PROVIDER.get().is_some_and(Result::is_ok)
    {
        return Ok(());
    }
    let layer = RequireAuthLayer::new(SERVICE_ID)
        .and_then(|layer| layer.with_expected_audiences([EXPECTED_AUDIENCE]))
        .map_err(|_| AuthorizationInitializationError)?;
    let _ = AUTHORIZATION_LAYER.set(layer);
    AUTHORIZATION_PROVIDER
        .get_or_init(load_authorization_provider)
        .as_ref()
        .map_err(|error| *error)?;
    if AUTHORIZATION_LAYER.get().is_some()
        && AUTHORIZATION_PROVIDER.get().is_some_and(Result::is_ok)
    {
        Ok(())
    } else {
        Err(AuthorizationInitializationError)
    }
}

fn load_authorization_provider()
-> Result<AuthorizationProviders, AuthorizationInitializationError> {
    let environment = wasip3::cli::environment::get_environment();
    let cedar =
        load_client(&environment, CEDAR_ENDPOINT_ENV, CEDAR_BEARER_ENV)?;
    let spicedb =
        load_client(&environment, SPICEDB_ENDPOINT_ENV, SPICEDB_BEARER_ENV)?;
    Ok(AuthorizationProviders { cedar, spicedb })
}

fn load_client(
    environment: &[(String, String)],
    endpoint_key: &str,
    bearer_key: &str,
) -> Result<AuthorizationClient, AuthorizationInitializationError> {
    let endpoint = unique_environment_value(environment, endpoint_key)?;
    let bearer = unique_environment_value(environment, bearer_key)?;
    let endpoint = AuthzenEndpoint::new_loopback_for_dev(endpoint)
        .map_err(|_| AuthorizationInitializationError)?;
    let transport = BearerAuthTransport::new(Wasip3Transport::new(), bearer)
        .map_err(|_| AuthorizationInitializationError)?;
    Ok(AuthzenClient::new(endpoint, transport))
}

fn unique_environment_value<'a>(
    environment: &'a [(String, String)],
    key: &str,
) -> Result<&'a str, AuthorizationInitializationError> {
    let mut values = environment
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str());
    let value = values.next().ok_or(AuthorizationInitializationError)?;
    if values.next().is_some() {
        return Err(AuthorizationInitializationError);
    }
    Ok(value)
}

fn diagnostic_stage(stage: &'static str) {
    if std::env::var("WASI_MIDDLEWARE_DIAGNOSTICS").as_deref() == Ok("true") {
        eprintln!("wasi.middleware stage={stage}");
    }
}

fn counter_resource() -> Result<Resource, ServerFnError> {
    let attribute = AttributeV1::new(
        AttributeNameV1::new("classification")
            .map_err(|_| configuration_error("invalid resource attribute"))?,
        AttributeValueV1::String(
            AttributeStringV1::new("internal")
                .map_err(|_| configuration_error("invalid resource value"))?,
        ),
        AttributeProvenanceV1::ResourceStore,
    )
    .map_err(|_| configuration_error("invalid resource attribute"))?;
    let mut attributes = AttributesV1::new();
    attributes
        .insert(attribute)
        .map_err(|_| configuration_error("invalid resource attributes"))?;
    Resource::new("counter", "session-counter")
        .map(|resource| resource.with_attributes(attributes))
        .map_err(|_| {
            configuration_error("invalid authorization resource configuration")
        })
}

fn provide_fixture_context() {
    let authentication = provide_auth_context();
    let Some(options) = use_context::<leptos_wasi::response::ResponseOptions>()
    else {
        return;
    };
    if authentication.is_err() {
        options.set_status(http::StatusCode::SERVICE_UNAVAILABLE);
        options.insert_header(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
    } else if let Ok(authentication) = &authentication {
        let state = match authentication.state() {
            leptos_wasi_authz::AuthStateV1::Anonymous => "anonymous",
            leptos_wasi_authz::AuthStateV1::Authenticated => "authenticated",
            _ => "unsupported",
        };
        options.insert_header(
            http::HeaderName::from_static("x-leptos-auth-context-state"),
            http::HeaderValue::from_static(state),
        );
    }
    provide_response_status_sink(ResponseStatusSink::new(
        move |status, headers| {
            options.set_status(status);
            for (name, value) in headers {
                options.insert_header(name.clone(), value.clone());
            }
        },
    ));
}

fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    std::fs::read(format!("/site/pkg/{path}"))
        .ok()
        .map(|bytes| leptos_wasi::response::Body::Sync(bytes.into()))
}

fn configuration_error(message: &str) -> ServerFnError {
    ServerFnError::ServerError(message.to_string())
}

fn internal_error<E>(_error: E) -> ErrorCode {
    eprintln!("leptos_wasi authz fixture internal failure");
    ErrorCode::InternalError(None)
}

wasip3::http::service::export!(LeptosServer);

#[derive(Clone, Copy, Debug)]
struct AuthorizationInitializationError;
