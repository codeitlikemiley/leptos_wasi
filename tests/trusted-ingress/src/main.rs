//! Native reference ingress for trusted-boundary integration tests.

use std::{
    array,
    collections::BTreeMap,
    convert::Infallible,
    error::Error,
    fs,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    process,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Request, Response, StatusCode, Uri,
    header::{
        AUTHORIZATION, CONTENT_TYPE, HOST, RETRY_AFTER, WWW_AUTHENTICATE,
    },
};
use http_body_util::{BodyExt as _, Full, Limited, combinators::BoxBody};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use serde::Deserialize;
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Sleep, timeout},
};
use wasi_http_metadata::{
    AuthContextV1, REQUEST_ID_HEADER, encode_auth_context,
    strip_reserved_auth_headers,
};
use wasi_http_policy_core::{
    AuthDecision, AuthnRequestV1, CorsConfig, RequestIdPolicy,
    apply_security_headers, authorization_value, parse_authn_response,
};

type BoxError = Box<dyn Error + Send + Sync>;
type ProxyBody = BoxBody<Bytes, BoxError>;
type HttpClient = Client<HttpConnector, ProxyBody>;

const BROKER_BODY_LIMIT: usize = 64 * 1024;
const STAGE_COUNT: usize = 10;
const ERROR_COUNT: usize = 8;

#[derive(Clone)]
struct Ingress {
    authn_client: HttpClient,
    terminal_client: HttpClient,
    terminals: Arc<[Terminal]>,
    broker_uri: Uri,
    service_id: Arc<str>,
    audiences: Arc<[String]>,
    cors: CorsConfig,
    request_ids: Arc<AtomicU64>,
    terminal_cursor: Arc<AtomicU64>,
    route_policy: Arc<BTreeMap<String, RouteClass>>,
    profile: PolicyProfile,
    gates: Gates,
    deadlines: Deadlines,
    metrics: Arc<Metrics>,
}

#[derive(Clone, Copy)]
struct Deadlines {
    queue: Duration,
    authentication: Duration,
    terminal_first_byte: Duration,
    stream_idle: Duration,
    stream_total: Duration,
    health_interval: Duration,
}

#[derive(Clone)]
struct Terminal {
    origin: Arc<str>,
    gate: Arc<AdmissionGate>,
    healthy: Arc<AtomicBool>,
    failures: Arc<AtomicU64>,
    successes: Arc<AtomicU64>,
}

#[derive(Clone)]
struct Gates {
    public: Arc<AdmissionGate>,
    authn_only: Arc<AdmissionGate>,
    cedar: Arc<AdmissionGate>,
    relationship: Arc<AdmissionGate>,
    hybrid: Arc<AdmissionGate>,
    broker: Arc<AdmissionGate>,
}

struct AdmissionGate {
    permits: Arc<Semaphore>,
    active: AtomicU64,
    queued: AtomicU64,
    max_active_observed: AtomicU64,
    max_queued_observed: AtomicU64,
    max_queue: u64,
}

struct AdmissionLease {
    gate: Arc<AdmissionGate>,
    _permit: OwnedSemaphorePermit,
}

struct RequestGuard {
    metrics: Arc<Metrics>,
}

struct GuardedBody {
    inner: Pin<Box<ProxyBody>>,
    idle: Pin<Box<Sleep>>,
    total: Pin<Box<Sleep>>,
    _leases: Vec<AdmissionLease>,
    _request: RequestGuard,
    metrics: Arc<Metrics>,
    body_started: Instant,
    request_started: Instant,
    idle_duration: Duration,
    completed: bool,
}

struct Metrics {
    active_requests: AtomicU64,
    overload_rejections: AtomicU64,
    cancellations: AtomicU64,
    client_disconnects: AtomicU64,
    stages: [LatencyMetric; STAGE_COUNT],
    errors: [AtomicU64; ERROR_COUNT],
}

struct LatencyMetric {
    count: AtomicU64,
    total_micros: AtomicU64,
    max_micros: AtomicU64,
    buckets: [AtomicU64; 32],
}

#[derive(Clone, Copy, Debug)]
enum Rejection {
    InvalidRequest,
    Unauthenticated,
    Unavailable,
    Overloaded,
}

#[derive(Clone, Copy, Debug)]
enum PolicyProfile {
    ProxyBaseline,
    EdgeAnonymous,
    EdgeAuthenticated,
}

#[derive(Clone, Copy, Debug)]
enum RouteClass {
    Public,
    AuthnOnly,
    Cedar,
    Relationship,
    Hybrid,
}

#[derive(Deserialize)]
struct RoutePolicyDocument {
    route: Vec<RoutePolicyEntry>,
}

#[derive(Deserialize)]
struct RoutePolicyEntry {
    path: String,
    class: String,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Stage {
    RequestTotal,
    Policy,
    AuthnQueue,
    AuthnSend,
    AuthnBody,
    TerminalQueue,
    TerminalSend,
    TerminalFirstByte,
    TerminalBody,
    ResponseTotal,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum ErrorClass {
    Overload,
    AuthnTimeout,
    AuthnTransport,
    AuthnContract,
    TerminalTimeout,
    TerminalTransport,
    StreamTimeout,
    Protocol,
}

impl Rejection {
    const fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Unavailable | Self::Overloaded => {
                StatusCode::SERVICE_UNAVAILABLE
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let ingress = Ingress::from_environment()?;
    ingress.spawn_health_checks();
    if let Some(address) = diagnostics_address()? {
        let metrics = Arc::clone(&ingress.metrics);
        let gates = ingress.gates.clone();
        let terminals = Arc::clone(&ingress.terminals);
        tokio::spawn(async move {
            if serve_diagnostics(address, metrics, gates, terminals)
                .await
                .is_err()
            {
                eprintln!("trusted.ingress stage=diagnostics");
            }
        });
    }
    let address = environment("TRUSTED_INGRESS_LISTEN_ADDR")?
        .parse::<SocketAddr>()
        .context("invalid trusted-ingress listen address")?;
    let listener = TcpListener::bind(address)
        .await
        .context("failed to bind trusted-ingress listener")?;
    eprintln!("trusted.ingress ready");

    loop {
        let (stream, _) =
            listener.accept().await.context("ingress accept failed")?;
        stream
            .set_nodelay(true)
            .context("failed to enable TCP_NODELAY")?;
        let ingress = ingress.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let ingress = ingress.clone();
                async move { Ok::<_, Infallible>(ingress.handle(request).await) }
            });
            if Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
                .is_err()
            {
                ingress_connection_failure();
            }
        });
    }
}

impl Ingress {
    fn from_environment() -> Result<Self> {
        let origins = terminal_origins()?;
        let terminal_limit =
            environment_usize("TRUSTED_INGRESS_TERMINAL_MAX_IN_FLIGHT", 64)?;
        let terminal_queue =
            environment_u64("TRUSTED_INGRESS_TERMINAL_MAX_QUEUED", 64)?;
        let terminals = origins
            .into_iter()
            .map(|origin| Terminal {
                origin: origin.into(),
                gate: AdmissionGate::shared(terminal_limit, terminal_queue),
                healthy: Arc::new(AtomicBool::new(true)),
                failures: Arc::new(AtomicU64::new(0)),
                successes: Arc::new(AtomicU64::new(0)),
            })
            .collect::<Vec<_>>();
        let broker_uri = environment("TRUSTED_INGRESS_AUTHN_BROKER_URL")?
            .parse::<Uri>()
            .context("invalid authentication broker URL")?;
        let service_id = environment("TRUSTED_INGRESS_SERVICE_ID")?;
        let audiences = environment("TRUSTED_INGRESS_AUDIENCES")?
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let deployment = AuthContextV1::anonymous(service_id, audiences)
            .context("invalid trusted-ingress deployment identity")?;
        let cors_origin = environment("TRUSTED_INGRESS_CORS_ORIGIN")?;
        let cors = CorsConfig::new(
            [cors_origin],
            ["GET", "HEAD", "POST", "OPTIONS"],
            ["content-type", "authorization", "x-request-id"],
            false,
        )
        .context("invalid trusted-ingress CORS policy")?;
        let profile = PolicyProfile::from_environment()?;
        let route_policy = load_route_policy()?;
        let authn_client = pooled_client(128);
        let terminal_client =
            pooled_client(terminal_limit.saturating_mul(terminals.len()));
        Ok(Self {
            authn_client,
            terminal_client,
            terminals: terminals.into(),
            broker_uri,
            service_id: deployment.service_id().into(),
            audiences: deployment.audiences().to_vec().into(),
            cors,
            request_ids: Arc::new(AtomicU64::new(1)),
            terminal_cursor: Arc::new(AtomicU64::new(0)),
            route_policy: Arc::new(route_policy),
            profile,
            gates: Gates::from_environment()?,
            deadlines: Deadlines::from_environment()?,
            metrics: Arc::new(Metrics::new()),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the reference proxy keeps its security boundary visible in one request flow"
    )]
    async fn handle(
        &self,
        mut request: Request<Incoming>,
    ) -> Response<ProxyBody> {
        let request_started = Instant::now();
        self.metrics.active_requests.fetch_add(1, Ordering::Relaxed);
        let request_guard = RequestGuard {
            metrics: Arc::clone(&self.metrics),
        };
        let request_id = self.request_id(request.headers());
        let Ok(request_id) = request_id else {
            self.metrics.error(ErrorClass::Protocol);
            return self.reject(Rejection::Unavailable, None, None);
        };
        let route_class = self.route_class(request.uri().path());
        let route_gate = self.gates.for_class(route_class);
        let route_lease = match route_gate
            .acquire(&self.metrics, Stage::TerminalQueue, self.deadlines.queue)
            .await
        {
            Ok(lease) => lease,
            Err(rejection) => {
                drop(request_guard);
                return self.reject(rejection, Some(&request_id), None);
            }
        };

        let policy_started = Instant::now();
        let cors = match self.evaluate_cors(&request) {
            Ok(cors) => cors,
            Err(rejection) => {
                drop(route_lease);
                drop(request_guard);
                return self.reject(rejection, Some(&request_id), None);
            }
        };
        self.metrics.record(Stage::Policy, policy_started.elapsed());
        if let Some(status) = cors
            .as_ref()
            .and_then(wasi_http_policy_core::CorsDecision::status)
        {
            let mut response = empty_response(status);
            if let Some(cors) = &cors {
                merge_headers(response.headers_mut(), cors.response_headers());
            }
            self.finish_response(&mut response, &request_id);
            drop(route_lease);
            drop(request_guard);
            return response;
        }

        let authorization = authorization_value(request.headers())
            .map(|value| value.map(str::to_owned));
        request.headers_mut().remove(AUTHORIZATION);
        strip_reserved_auth_headers(request.headers_mut());
        let Ok(authorization) = authorization else {
            drop(route_lease);
            drop(request_guard);
            return self.reject(
                Rejection::InvalidRequest,
                Some(&request_id),
                cors.as_ref()
                    .map(wasi_http_policy_core::CorsDecision::response_headers),
            );
        };
        let context = match self
            .promote_context(authorization.as_deref(), &request_id)
            .await
        {
            Ok(context) => context,
            Err(rejection) => {
                drop(route_lease);
                drop(request_guard);
                return self.reject(
                    rejection,
                    Some(&request_id),
                    cors.as_ref().map(
                        wasi_http_policy_core::CorsDecision::response_headers,
                    ),
                );
            }
        };
        if self
            .install_trusted_headers(&mut request, &context, &request_id)
            .is_err()
        {
            drop(route_lease);
            drop(request_guard);
            return self.reject(
                Rejection::Unavailable,
                Some(&request_id),
                cors.as_ref()
                    .map(wasi_http_policy_core::CorsDecision::response_headers),
            );
        }
        request.headers_mut().remove(HOST);

        let Some(terminal) = self.select_terminal() else {
            drop(route_lease);
            drop(request_guard);
            self.metrics.error(ErrorClass::TerminalTransport);
            return self.reject(
                Rejection::Unavailable,
                Some(&request_id),
                cors.as_ref()
                    .map(wasi_http_policy_core::CorsDecision::response_headers),
            );
        };
        let terminal_lease = match Arc::clone(&terminal.gate)
            .acquire(&self.metrics, Stage::TerminalQueue, self.deadlines.queue)
            .await
        {
            Ok(lease) => lease,
            Err(rejection) => {
                drop(route_lease);
                drop(request_guard);
                return self.reject(
                    rejection,
                    Some(&request_id),
                    cors.as_ref().map(
                        wasi_http_policy_core::CorsDecision::response_headers,
                    ),
                );
            }
        };
        let Ok(uri) = downstream_uri(&terminal.origin, request.uri()) else {
            drop(terminal_lease);
            drop(route_lease);
            drop(request_guard);
            return self.reject(
                Rejection::InvalidRequest,
                Some(&request_id),
                cors.as_ref()
                    .map(wasi_http_policy_core::CorsDecision::response_headers),
            );
        };
        *request.uri_mut() = uri;
        let send_started = Instant::now();
        let response = timeout(
            self.deadlines.terminal_first_byte,
            self.terminal_client.request(request.map(box_incoming)),
        )
        .await;
        self.metrics
            .record(Stage::TerminalSend, send_started.elapsed());
        let mut response = match response {
            Ok(Ok(response)) => {
                terminal.mark_success();
                self.metrics
                    .record(Stage::TerminalFirstByte, send_started.elapsed());
                response
            }
            Ok(Err(_)) => {
                terminal.mark_failure();
                self.metrics.error(ErrorClass::TerminalTransport);
                drop(terminal_lease);
                drop(route_lease);
                drop(request_guard);
                return self.reject(
                    Rejection::Unavailable,
                    Some(&request_id),
                    cors.as_ref().map(
                        wasi_http_policy_core::CorsDecision::response_headers,
                    ),
                );
            }
            Err(_) => {
                terminal.mark_failure();
                self.metrics.error(ErrorClass::TerminalTimeout);
                drop(terminal_lease);
                drop(route_lease);
                drop(request_guard);
                return self.reject(
                    Rejection::Unavailable,
                    Some(&request_id),
                    cors.as_ref().map(
                        wasi_http_policy_core::CorsDecision::response_headers,
                    ),
                );
            }
        };
        strip_reserved_auth_headers(response.headers_mut());
        if let Some(cors) = &cors {
            merge_headers(response.headers_mut(), cors.response_headers());
        }
        self.finish_response_headers(response.headers_mut(), &request_id);
        self.metrics
            .record(Stage::RequestTotal, request_started.elapsed());
        let (parts, body) = response.into_parts();
        let body = GuardedBody::new(
            body.map_err(|error| -> BoxError { Box::new(error) })
                .boxed(),
            vec![route_lease, terminal_lease],
            request_guard,
            Arc::clone(&self.metrics),
            request_started,
            self.deadlines.stream_idle,
            self.deadlines.stream_total,
        )
        .boxed();
        Response::from_parts(parts, body)
    }

    fn request_id(&self, headers: &HeaderMap) -> Result<String, ()> {
        if matches!(self.profile, PolicyProfile::ProxyBaseline) {
            return Ok(format!(
                "trusted-ingress-{}-{}",
                process::id(),
                self.request_ids.fetch_add(1, Ordering::Relaxed)
            ));
        }
        RequestIdPolicy
            .canonicalize(headers, || {
                format!(
                    "trusted-ingress-{}-{}",
                    process::id(),
                    self.request_ids.fetch_add(1, Ordering::Relaxed)
                )
            })
            .map_err(|_| ())
    }

    fn evaluate_cors(
        &self,
        request: &Request<Incoming>,
    ) -> Result<Option<wasi_http_policy_core::CorsDecision>, Rejection> {
        if matches!(self.profile, PolicyProfile::ProxyBaseline) {
            return Ok(None);
        }
        self.cors
            .evaluate(request.method(), request.headers())
            .map(Some)
            .map_err(|_| Rejection::InvalidRequest)
    }

    async fn authenticate(
        &self,
        authorization: Option<&str>,
        request_id: &str,
    ) -> Result<AuthContextV1, Rejection> {
        let Some(authorization) = authorization else {
            return self.anonymous_context();
        };
        let queue_started = Instant::now();
        let _lease = Arc::clone(&self.gates.broker)
            .acquire(&self.metrics, Stage::AuthnQueue, self.deadlines.queue)
            .await?;
        self.metrics
            .record(Stage::AuthnQueue, queue_started.elapsed());
        let broker_request = AuthnRequestV1 {
            version: 1,
            service_id: self.service_id.to_string(),
            audiences: self.audiences.to_vec(),
            request_id: request_id.to_owned(),
        };
        let body = serde_json::to_vec(&broker_request).map_err(|_| {
            self.metrics.error(ErrorClass::AuthnContract);
            Rejection::Unavailable
        })?;
        let operation = async {
            let mut request = Request::post(self.broker_uri.clone())
                .header(CONTENT_TYPE, "application/json")
                .header(AUTHORIZATION, authorization)
                .header(REQUEST_ID_HEADER, request_id)
                .body(box_full(Bytes::from(body)))
                .map_err(|_| Rejection::Unavailable)?;
            request.headers_mut().remove(HOST);
            let send_started = Instant::now();
            let response = self
                .authn_client
                .request(request)
                .await
                .map_err(|_| Rejection::Unavailable)?;
            self.metrics
                .record(Stage::AuthnSend, send_started.elapsed());
            let status = response.status();
            let body_started = Instant::now();
            let body = Limited::new(response.into_body(), BROKER_BODY_LIMIT)
                .collect()
                .await
                .map_err(|_| Rejection::Unavailable)?
                .to_bytes();
            self.metrics
                .record(Stage::AuthnBody, body_started.elapsed());
            Ok::<_, Rejection>((status, body))
        };
        let (status, body) =
            match timeout(self.deadlines.authentication, operation).await {
                Ok(Ok(value)) => value,
                Ok(Err(rejection)) => {
                    self.metrics.error(ErrorClass::AuthnTransport);
                    return Err(rejection);
                }
                Err(_) => {
                    self.metrics.error(ErrorClass::AuthnTimeout);
                    return Err(Rejection::Unavailable);
                }
            };
        match parse_authn_response(status, &body) {
            AuthDecision::Allow(claims) => claims
                .into_context(
                    self.service_id.to_string(),
                    self.audiences.iter().cloned(),
                )
                .map_err(|_| {
                    self.metrics.error(ErrorClass::AuthnContract);
                    Rejection::Unavailable
                }),
            AuthDecision::Unauthenticated => Err(Rejection::Unauthenticated),
            AuthDecision::Unavailable => {
                self.metrics.error(ErrorClass::AuthnContract);
                Err(Rejection::Unavailable)
            }
        }
    }

    async fn promote_context(
        &self,
        authorization: Option<&str>,
        request_id: &str,
    ) -> Result<AuthContextV1, Rejection> {
        match self.profile {
            PolicyProfile::ProxyBaseline | PolicyProfile::EdgeAnonymous => {
                self.anonymous_context()
            }
            PolicyProfile::EdgeAuthenticated => {
                self.authenticate(authorization, request_id).await
            }
        }
    }

    fn anonymous_context(&self) -> Result<AuthContextV1, Rejection> {
        AuthContextV1::anonymous(
            self.service_id.to_string(),
            self.audiences.iter().cloned(),
        )
        .map_err(|_| Rejection::Unavailable)
    }

    fn install_trusted_headers(
        &self,
        request: &mut Request<Incoming>,
        context: &AuthContextV1,
        request_id: &str,
    ) -> Result<(), ()> {
        request.headers_mut().insert(
            wasi_http_metadata::AUTH_CONTEXT_HEADER,
            encode_auth_context(context).map_err(|_| ())?,
        );
        if !matches!(self.profile, PolicyProfile::ProxyBaseline) {
            request.headers_mut().insert(
                REQUEST_ID_HEADER,
                HeaderValue::from_str(request_id).map_err(|_| ())?,
            );
        }
        Ok(())
    }

    fn select_terminal(&self) -> Option<Terminal> {
        let minimum = self
            .terminals
            .iter()
            .filter(|terminal| terminal.healthy.load(Ordering::Acquire))
            .map(|terminal| terminal.gate.active.load(Ordering::Relaxed))
            .min()?;
        let candidates = self
            .terminals
            .iter()
            .filter(|terminal| {
                terminal.healthy.load(Ordering::Acquire)
                    && terminal.gate.active.load(Ordering::Relaxed) == minimum
            })
            .collect::<Vec<_>>();
        let cursor = self.terminal_cursor.fetch_add(1, Ordering::Relaxed);
        let index = usize::try_from(cursor).unwrap_or(0) % candidates.len();
        candidates.get(index).map(|terminal| (*terminal).clone())
    }

    fn route_class(&self, path: &str) -> RouteClass {
        self.route_policy
            .get(path)
            .copied()
            .unwrap_or(RouteClass::Public)
    }

    fn reject(
        &self,
        rejection: Rejection,
        request_id: Option<&str>,
        cors: Option<&HeaderMap>,
    ) -> Response<ProxyBody> {
        let mut response = empty_response(rejection.status());
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
        if matches!(rejection, Rejection::Unauthenticated) {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if matches!(rejection, Rejection::Overloaded) {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        }
        if let Some(cors) = cors {
            merge_headers(response.headers_mut(), cors);
        }
        if let Some(request_id) = request_id {
            self.finish_response(&mut response, request_id);
        } else if !matches!(self.profile, PolicyProfile::ProxyBaseline) {
            apply_security_headers(response.headers_mut());
        }
        response
    }

    fn finish_response(
        &self,
        response: &mut Response<ProxyBody>,
        request_id: &str,
    ) {
        self.finish_response_headers(response.headers_mut(), request_id);
    }

    fn finish_response_headers(
        &self,
        headers: &mut HeaderMap,
        request_id: &str,
    ) {
        strip_reserved_auth_headers(headers);
        if matches!(self.profile, PolicyProfile::ProxyBaseline) {
            return;
        }
        apply_security_headers(headers);
        if let Ok(value) = HeaderValue::from_str(request_id) {
            headers.insert(REQUEST_ID_HEADER, value);
        }
    }

    fn spawn_health_checks(&self) {
        let ingress = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ingress.deadlines.health_interval).await;
                for terminal in &*ingress.terminals {
                    let terminal = terminal.clone();
                    let ingress = ingress.clone();
                    tokio::spawn(async move {
                        if ingress.health_check(&terminal).await {
                            terminal.mark_success();
                        } else {
                            terminal.mark_failure();
                        }
                    });
                }
            }
        });
    }

    async fn health_check(&self, terminal: &Terminal) -> bool {
        let Ok(context) = self.anonymous_context() else {
            return false;
        };
        let Ok(encoded) = encode_auth_context(&context) else {
            return false;
        };
        let Ok(uri) = format!("{}/", terminal.origin.trim_end_matches('/'))
            .parse::<Uri>()
        else {
            return false;
        };
        let Ok(request) = Request::get(uri)
            .header(wasi_http_metadata::AUTH_CONTEXT_HEADER, encoded)
            .body(box_full(Bytes::new()))
        else {
            return false;
        };
        timeout(self.deadlines.terminal_first_byte, async {
            let response = self.terminal_client.request(request).await?;
            let healthy = response.status().is_success();
            response.into_body().collect().await?;
            Ok::<_, BoxError>(healthy)
        })
        .await
        .is_ok_and(|response| response.unwrap_or(false))
    }
}

impl PolicyProfile {
    fn from_environment() -> Result<Self> {
        if let Ok(value) = std::env::var("TRUSTED_INGRESS_PROFILE") {
            return match value.as_str() {
                "proxy-baseline" => Ok(Self::ProxyBaseline),
                "edge-anonymous" => Ok(Self::EdgeAnonymous),
                "edge-authenticated" => Ok(Self::EdgeAuthenticated),
                _ => anyhow::bail!("invalid TRUSTED_INGRESS_PROFILE"),
            };
        }
        Ok(if optional_bool("TRUSTED_INGRESS_POLICY_ENABLED")? {
            Self::EdgeAuthenticated
        } else {
            Self::ProxyBaseline
        })
    }
}

impl Deadlines {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            queue: environment_duration_ms("TRUSTED_INGRESS_QUEUE_WAIT_MS", 5)?,
            authentication: environment_duration_ms(
                "TRUSTED_INGRESS_AUTHN_DEADLINE_MS",
                2_000,
            )?,
            terminal_first_byte: environment_duration_ms(
                "TRUSTED_INGRESS_TERMINAL_FIRST_BYTE_MS",
                2_000,
            )?,
            stream_idle: environment_duration_ms(
                "TRUSTED_INGRESS_STREAM_IDLE_MS",
                2_000,
            )?,
            stream_total: environment_duration_ms(
                "TRUSTED_INGRESS_STREAM_TOTAL_MS",
                30_000,
            )?,
            health_interval: environment_duration_ms(
                "TRUSTED_INGRESS_HEALTH_INTERVAL_MS",
                1_000,
            )?,
        })
    }
}

impl Gates {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            public: class_gate("PUBLIC", 128, 128)?,
            authn_only: class_gate("AUTHN_ONLY", 100, 100)?,
            cedar: class_gate("CEDAR", 100, 100)?,
            relationship: class_gate("RELATIONSHIP", 100, 100)?,
            hybrid: class_gate("HYBRID", 100, 100)?,
            broker: class_gate("BROKER", 128, 128)?,
        })
    }

    fn for_class(&self, route: RouteClass) -> Arc<AdmissionGate> {
        match route {
            RouteClass::Public => Arc::clone(&self.public),
            RouteClass::AuthnOnly => Arc::clone(&self.authn_only),
            RouteClass::Cedar => Arc::clone(&self.cedar),
            RouteClass::Relationship => Arc::clone(&self.relationship),
            RouteClass::Hybrid => Arc::clone(&self.hybrid),
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "public": self.public.snapshot(),
            "authn_only": self.authn_only.snapshot(),
            "cedar": self.cedar.snapshot(),
            "relationship": self.relationship.snapshot(),
            "hybrid": self.hybrid.snapshot(),
            "broker": self.broker.snapshot(),
        })
    }
}

impl AdmissionGate {
    fn shared(limit: usize, max_queue: u64) -> Arc<Self> {
        Arc::new(Self {
            permits: Arc::new(Semaphore::new(limit)),
            active: AtomicU64::new(0),
            queued: AtomicU64::new(0),
            max_active_observed: AtomicU64::new(0),
            max_queued_observed: AtomicU64::new(0),
            max_queue,
        })
    }

    async fn acquire(
        self: Arc<Self>,
        metrics: &Metrics,
        stage: Stage,
        queue_timeout: Duration,
    ) -> Result<AdmissionLease, Rejection> {
        let queued = self.queued.fetch_add(1, Ordering::AcqRel);
        self.max_queued_observed
            .fetch_max(queued.saturating_add(1), Ordering::Relaxed);
        if queued >= self.max_queue {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            metrics.overload();
            return Err(Rejection::Overloaded);
        }
        let started = Instant::now();
        let permit =
            timeout(queue_timeout, Arc::clone(&self.permits).acquire_owned())
                .await;
        self.queued.fetch_sub(1, Ordering::AcqRel);
        metrics.record(stage, started.elapsed());
        let Ok(Ok(permit)) = permit else {
            metrics.overload();
            return Err(Rejection::Overloaded);
        };
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active_observed
            .fetch_max(active, Ordering::Relaxed);
        Ok(AdmissionLease {
            gate: self,
            _permit: permit,
        })
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "active": self.active.load(Ordering::Relaxed),
            "queued": self.queued.load(Ordering::Relaxed),
            "max_active_observed": self.max_active_observed.load(Ordering::Relaxed),
            "max_queued_observed": self.max_queued_observed.load(Ordering::Relaxed),
            "available_permits": self.permits.available_permits(),
        })
    }
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.metrics.active_requests.fetch_sub(1, Ordering::AcqRel);
    }
}

impl GuardedBody {
    fn new(
        inner: ProxyBody,
        leases: Vec<AdmissionLease>,
        request: RequestGuard,
        metrics: Arc<Metrics>,
        request_started: Instant,
        idle_duration: Duration,
        total_duration: Duration,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            idle: Box::pin(tokio::time::sleep(idle_duration)),
            total: Box::pin(tokio::time::sleep(total_duration)),
            _leases: leases,
            _request: request,
            metrics,
            body_started: Instant::now(),
            request_started,
            idle_duration,
            completed: false,
        }
    }
}

impl http_body::Body for GuardedBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if self.total.as_mut().poll(context).is_ready()
            || self.idle.as_mut().poll(context).is_ready()
        {
            self.completed = true;
            self.metrics.error(ErrorClass::StreamTimeout);
            self.metrics
                .record(Stage::TerminalBody, self.body_started.elapsed());
            self.metrics
                .record(Stage::ResponseTotal, self.request_started.elapsed());
            return Poll::Ready(Some(Err(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "trusted ingress stream deadline exceeded",
            )))));
        }
        match self.inner.as_mut().poll_frame(context) {
            Poll::Ready(Some(Ok(frame))) => {
                let idle_duration = self.idle_duration;
                self.idle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + idle_duration);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.completed = true;
                self.metrics.error(ErrorClass::TerminalTransport);
                self.metrics
                    .record(Stage::TerminalBody, self.body_started.elapsed());
                self.metrics.record(
                    Stage::ResponseTotal,
                    self.request_started.elapsed(),
                );
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.completed = true;
                self.metrics
                    .record(Stage::TerminalBody, self.body_started.elapsed());
                self.metrics.record(
                    Stage::ResponseTotal,
                    self.request_started.elapsed(),
                );
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for GuardedBody {
    fn drop(&mut self) {
        if !self.completed {
            self.metrics.cancellations.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .client_disconnects
                .fetch_add(1, Ordering::Relaxed);
            self.metrics
                .record(Stage::TerminalBody, self.body_started.elapsed());
            self.metrics
                .record(Stage::ResponseTotal, self.request_started.elapsed());
        }
    }
}

impl Metrics {
    fn new() -> Self {
        Self {
            active_requests: AtomicU64::new(0),
            overload_rejections: AtomicU64::new(0),
            cancellations: AtomicU64::new(0),
            client_disconnects: AtomicU64::new(0),
            stages: array::from_fn(|_| LatencyMetric::new()),
            errors: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn record(&self, stage: Stage, duration: Duration) {
        self.stages[stage as usize].record(duration);
    }

    fn error(&self, class: ErrorClass) {
        self.errors[class as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn overload(&self) {
        self.overload_rejections.fetch_add(1, Ordering::Relaxed);
        self.error(ErrorClass::Overload);
    }

    fn snapshot(&self) -> serde_json::Value {
        let stage_names = [
            "request_total",
            "policy",
            "authn_queue",
            "authn_send",
            "authn_body",
            "terminal_queue",
            "terminal_send",
            "terminal_first_byte",
            "terminal_body",
            "response_total",
        ];
        let error_names = [
            "overload",
            "authn_timeout",
            "authn_transport",
            "authn_contract",
            "terminal_timeout",
            "terminal_transport",
            "stream_timeout",
            "protocol",
        ];
        let stages = stage_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                ((*name).to_owned(), self.stages[index].snapshot())
            })
            .collect::<serde_json::Map<_, _>>();
        let errors = error_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    (*name).to_owned(),
                    serde_json::json!(
                        self.errors[index].load(Ordering::Relaxed)
                    ),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "active_requests": self.active_requests.load(Ordering::Relaxed),
            "overload_rejections": self.overload_rejections.load(Ordering::Relaxed),
            "cancellations": self.cancellations.load(Ordering::Relaxed),
            "client_disconnects": self.client_disconnects.load(Ordering::Relaxed),
            "stages": stages,
            "errors": errors,
        })
    }
}

impl LatencyMetric {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_micros: AtomicU64::new(0),
            max_micros: AtomicU64::new(0),
            buckets: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn record(&self, duration: Duration) {
        let micros = u64::try_from(duration.as_micros())
            .unwrap_or(u64::MAX)
            .max(1);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_micros.fetch_add(micros, Ordering::Relaxed);
        self.max_micros.fetch_max(micros, Ordering::Relaxed);
        let bucket = usize::try_from(micros.ilog2()).unwrap_or(31).min(31);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> serde_json::Value {
        let count = self.count.load(Ordering::Relaxed);
        let total = self.total_micros.load(Ordering::Relaxed);
        serde_json::json!({
            "count": count,
            "mean_micros": total.checked_div(count).unwrap_or(0),
            "max_micros": self.max_micros.load(Ordering::Relaxed),
            "p50_upper_micros": self.quantile(50),
            "p95_upper_micros": self.quantile(95),
            "p99_upper_micros": self.quantile(99),
        })
    }

    fn quantile(&self, percentile: u64) -> u64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        let target = count.saturating_mul(percentile).div_ceil(100);
        let mut seen = 0_u64;
        for (index, bucket) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(bucket.load(Ordering::Relaxed));
            if seen >= target {
                return 1_u64
                    .checked_shl(u32::try_from(index + 1).unwrap_or(63))
                    .unwrap_or(u64::MAX);
            }
        }
        u64::MAX
    }
}

impl Terminal {
    fn mark_success(&self) {
        self.failures.store(0, Ordering::Release);
        let successes = self.successes.fetch_add(1, Ordering::AcqRel) + 1;
        if successes >= 2 {
            self.healthy.store(true, Ordering::Release);
        }
    }

    fn mark_failure(&self) {
        self.successes.store(0, Ordering::Release);
        let failures = self.failures.fetch_add(1, Ordering::AcqRel) + 1;
        if failures >= 3 {
            self.healthy.store(false, Ordering::Release);
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "healthy": self.healthy.load(Ordering::Acquire),
            "active": self.gate.active.load(Ordering::Relaxed),
            "queued": self.gate.queued.load(Ordering::Relaxed),
            "available_permits": self.gate.permits.available_permits(),
        })
    }
}

async fn serve_diagnostics(
    address: SocketAddr,
    metrics: Arc<Metrics>,
    gates: Gates,
    terminals: Arc<[Terminal]>,
) -> Result<()> {
    anyhow::ensure!(
        address.ip().is_loopback(),
        "diagnostics must bind loopback"
    );
    let listener = TcpListener::bind(address)
        .await
        .context("failed to bind diagnostics listener")?;
    loop {
        let (stream, _) = listener.accept().await?;
        stream.set_nodelay(true)?;
        let metrics = Arc::clone(&metrics);
        let gates = gates.clone();
        let terminals = Arc::clone(&terminals);
        tokio::spawn(async move {
            let service = service_fn(move |_request: Request<Incoming>| {
                let body = serde_json::to_vec(&serde_json::json!({
                    "schema": 1,
                    "metrics": metrics.snapshot(),
                    "gates": gates.snapshot(),
                    "terminals": terminals.iter().map(Terminal::snapshot).collect::<Vec<_>>(),
                }))
                .unwrap_or_else(|_| b"{}".to_vec());
                async move {
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(
                        body,
                    ))))
                }
            });
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

fn class_gate(
    name: &str,
    limit: usize,
    queued: u64,
) -> Result<Arc<AdmissionGate>> {
    Ok(AdmissionGate::shared(
        environment_usize(
            &format!("TRUSTED_INGRESS_{name}_MAX_IN_FLIGHT"),
            limit,
        )?,
        environment_u64(&format!("TRUSTED_INGRESS_{name}_MAX_QUEUED"), queued)?,
    ))
}

fn terminal_origins() -> Result<Vec<String>> {
    let values = std::env::var("TRUSTED_INGRESS_TERMINAL_ORIGINS")
        .or_else(|_| std::env::var("TRUSTED_INGRESS_TERMINAL_ORIGIN"))
        .context("missing trusted-ingress terminal origins")?;
    let origins = values
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    anyhow::ensure!(!origins.is_empty() && origins.len() <= 4);
    for origin in &origins {
        validate_origin(origin).context("invalid terminal origin")?;
    }
    Ok(origins)
}

fn diagnostics_address() -> Result<Option<SocketAddr>> {
    std::env::var("TRUSTED_INGRESS_DIAGNOSTICS_ADDR")
        .ok()
        .map(|value| value.parse().context("invalid diagnostics address"))
        .transpose()
}

fn load_route_policy() -> Result<BTreeMap<String, RouteClass>> {
    let path = environment("TRUSTED_INGRESS_ROUTE_POLICY")?;
    let source =
        fs::read_to_string(path).context("failed to read route policy")?;
    parse_route_policy(&source)
}

fn parse_route_policy(source: &str) -> Result<BTreeMap<String, RouteClass>> {
    let document: RoutePolicyDocument =
        toml::from_str(source).context("invalid route policy")?;
    let mut routes = BTreeMap::new();
    for entry in document.route {
        anyhow::ensure!(
            entry.path.starts_with('/') && !entry.path.contains('?')
        );
        let class = match entry.class.as_str() {
            "authn_only" => RouteClass::AuthnOnly,
            "cedar" => RouteClass::Cedar,
            "relationship" => RouteClass::Relationship,
            "hybrid" => RouteClass::Hybrid,
            _ => anyhow::bail!("invalid route class"),
        };
        anyhow::ensure!(routes.insert(entry.path, class).is_none());
    }
    Ok(routes)
}

fn pooled_client(max_idle: usize) -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);
    Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(max_idle.max(1))
        .pool_idle_timeout(Duration::from_secs(30))
        .build(connector)
}

fn optional_bool(name: &str) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) if value.eq_ignore_ascii_case("true") || value == "1" => {
            Ok(true)
        }
        Ok(value) if value.eq_ignore_ascii_case("false") || value == "0" => {
            Ok(false)
        }
        Ok(_) => anyhow::bail!("{name} must be true, false, 1, or 0"),
        Err(std::env::VarError::NotPresent) => Ok(true),
        Err(error) => Err(error).with_context(|| format!("invalid {name}")),
    }
}

fn environment(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing {name}"))
}

fn environment_usize(name: &str, default: usize) -> Result<usize> {
    let value = std::env::var(name)
        .ok()
        .map_or(Ok(default), |value| value.parse().context("invalid limit"))?;
    anyhow::ensure!(value > 0 && value <= 65_536);
    Ok(value)
}

fn environment_u64(name: &str, default: u64) -> Result<u64> {
    let value = std::env::var(name)
        .ok()
        .map_or(Ok(default), |value| value.parse().context("invalid limit"))?;
    anyhow::ensure!(value > 0 && value <= 65_536);
    Ok(value)
}

fn environment_duration_ms(name: &str, default: u64) -> Result<Duration> {
    let milliseconds = environment_u64(name, default)?;
    anyhow::ensure!(milliseconds > 0 && milliseconds <= 300_000);
    Ok(Duration::from_millis(milliseconds))
}

fn validate_origin(value: &str) -> Result<()> {
    let uri = value.parse::<Uri>().context("invalid URI")?;
    let host = uri.host().context("origin host is missing")?;
    let ip = host
        .parse::<IpAddr>()
        .context("origin host must be an IP address")?;
    anyhow::ensure!(uri.scheme_str() == Some("http") && ip.is_loopback());
    anyhow::ensure!(uri.path().is_empty() || uri.path() == "/");
    Ok(())
}

fn downstream_uri(origin: &str, incoming: &Uri) -> Result<Uri> {
    let path = incoming
        .path_and_query()
        .map_or("/", |value| value.as_str());
    format!("{}{path}", origin.trim_end_matches('/'))
        .parse()
        .context("invalid downstream URI")
}

fn merge_headers(target: &mut HeaderMap, source: &HeaderMap) {
    for (name, value) in source {
        target.insert(name.clone(), value.clone());
    }
}

fn empty_response(status: StatusCode) -> Response<ProxyBody> {
    let mut response = Response::new(box_full(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn box_full(bytes: Bytes) -> ProxyBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

fn box_incoming(body: Incoming) -> ProxyBody {
    body.map_err(|error| -> BoxError { Box::new(error) })
        .boxed()
}

fn ingress_connection_failure() {
    eprintln!("trusted.ingress stage=connection");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downstream_uri_should_preserve_path_and_query() {
        let incoming =
            "/pkg/split_1.wasm?v=1".parse::<Uri>().expect("valid URI");

        let result = downstream_uri("http://127.0.0.1:3001", &incoming)
            .expect("downstream URI");

        assert_eq!(result, "http://127.0.0.1:3001/pkg/split_1.wasm?v=1");
    }

    #[test]
    fn route_classes_should_separate_authorization_capacity() {
        let policy = parse_route_policy(
            r#"[[route]]
path = "/api/authorize_relationship"
class = "relationship"
"#,
        )
        .expect("route policy");
        assert!(matches!(
            policy.get("/api/authorize_relationship"),
            Some(RouteClass::Relationship)
        ));
    }

    #[test]
    fn rejection_statuses_should_fail_closed() {
        assert_eq!(Rejection::InvalidRequest.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            Rejection::Unauthenticated.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            Rejection::Unavailable.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            Rejection::Overloaded.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
