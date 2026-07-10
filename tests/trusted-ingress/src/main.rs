//! Native reference ingress for trusted-boundary integration tests.

use std::{
    convert::Infallible,
    error::Error,
    net::SocketAddr,
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Request, Response, StatusCode, Uri,
    header::{AUTHORIZATION, CONTENT_TYPE, HOST, WWW_AUTHENTICATE},
};
use http_body_util::{BodyExt as _, Full, Limited, combinators::BoxBody};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use tokio::{net::TcpListener, sync::Semaphore, time::timeout};
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
const BROKER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct Ingress {
    client: HttpClient,
    terminal_origin: Arc<str>,
    broker_uri: Uri,
    service_id: Arc<str>,
    audiences: Arc<[String]>,
    cors: CorsConfig,
    request_ids: Arc<AtomicU64>,
    broker_admission: Arc<Semaphore>,
}

#[derive(Clone, Copy, Debug)]
enum Rejection {
    InvalidRequest,
    Unauthenticated,
    Unavailable,
}

impl Rejection {
    const fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let ingress = Ingress::from_environment()?;
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
                eprintln!("trusted.ingress stage=connection");
            }
        });
    }
}

impl Ingress {
    fn from_environment() -> Result<Self> {
        let terminal_origin = environment("TRUSTED_INGRESS_TERMINAL_ORIGIN")?;
        validate_origin(&terminal_origin).context("invalid terminal origin")?;
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
        let client =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        Ok(Self {
            client,
            terminal_origin: terminal_origin.into(),
            broker_uri,
            service_id: deployment.service_id().into(),
            audiences: deployment.audiences().to_vec().into(),
            cors,
            request_ids: Arc::new(AtomicU64::new(1)),
            broker_admission: Arc::new(Semaphore::new(128)),
        })
    }

    async fn handle(
        &self,
        mut request: Request<Incoming>,
    ) -> Response<ProxyBody> {
        let Ok(request_id) =
            RequestIdPolicy.canonicalize(request.headers(), || {
                format!(
                    "trusted-ingress-{}-{}",
                    process::id(),
                    self.request_ids.fetch_add(1, Ordering::Relaxed)
                )
            })
        else {
            return Self::reject(Rejection::Unavailable, None, None);
        };
        let Ok(cors) = self.cors.evaluate(request.method(), request.headers())
        else {
            return Self::reject(
                Rejection::InvalidRequest,
                Some(&request_id),
                None,
            );
        };
        if let Some(status) = cors.status() {
            let mut response = empty_response(status);
            merge_headers(response.headers_mut(), cors.response_headers());
            Self::finish_response(&mut response, &request_id);
            return response;
        }

        let authorization = authorization_value(request.headers())
            .map(|value| value.map(str::to_owned));
        request.headers_mut().remove(AUTHORIZATION);
        strip_reserved_auth_headers(request.headers_mut());
        let Ok(authorization) = authorization else {
            return Self::reject(
                Rejection::InvalidRequest,
                Some(&request_id),
                Some(cors.response_headers()),
            );
        };
        let context = match self
            .authenticate(authorization.as_deref(), &request_id)
            .await
        {
            Ok(context) => context,
            Err(rejection) => {
                return Self::reject(
                    rejection,
                    Some(&request_id),
                    Some(cors.response_headers()),
                );
            }
        };
        let Ok(encoded) = encode_auth_context(&context) else {
            return Self::reject(
                Rejection::Unavailable,
                Some(&request_id),
                Some(cors.response_headers()),
            );
        };
        request
            .headers_mut()
            .insert(wasi_http_metadata::AUTH_CONTEXT_HEADER, encoded);
        let Ok(request_id_header) = HeaderValue::from_str(&request_id) else {
            return Self::reject(
                Rejection::Unavailable,
                Some(&request_id),
                Some(cors.response_headers()),
            );
        };
        request
            .headers_mut()
            .insert(REQUEST_ID_HEADER, request_id_header);
        request.headers_mut().remove(HOST);
        let Ok(uri) = downstream_uri(&self.terminal_origin, request.uri())
        else {
            return Self::reject(
                Rejection::InvalidRequest,
                Some(&request_id),
                Some(cors.response_headers()),
            );
        };
        *request.uri_mut() = uri;
        let request = request.map(box_incoming);
        let mut response = match self.client.request(request).await {
            Ok(response) => response.map(box_incoming),
            Err(_) => {
                return Self::reject(
                    Rejection::Unavailable,
                    Some(&request_id),
                    Some(cors.response_headers()),
                );
            }
        };
        strip_reserved_auth_headers(response.headers_mut());
        merge_headers(response.headers_mut(), cors.response_headers());
        Self::finish_response(&mut response, &request_id);
        response
    }

    async fn authenticate(
        &self,
        authorization: Option<&str>,
        request_id: &str,
    ) -> Result<AuthContextV1, Rejection> {
        let Some(authorization) = authorization else {
            return AuthContextV1::anonymous(
                self.service_id.to_string(),
                self.audiences.iter().cloned(),
            )
            .map_err(|_| Rejection::Unavailable);
        };
        let _permit = self
            .broker_admission
            .try_acquire()
            .map_err(|_| Rejection::Unavailable)?;
        let broker_request = AuthnRequestV1 {
            version: 1,
            service_id: self.service_id.to_string(),
            audiences: self.audiences.to_vec(),
            request_id: request_id.to_owned(),
        };
        let body = serde_json::to_vec(&broker_request)
            .map_err(|_| Rejection::Unavailable)?;
        let mut request = Request::post(self.broker_uri.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, authorization)
            .header(REQUEST_ID_HEADER, request_id)
            .body(box_full(Bytes::from(body)))
            .map_err(|_| Rejection::Unavailable)?;
        request.headers_mut().remove(HOST);
        let response = timeout(BROKER_TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| Rejection::Unavailable)?
            .map_err(|_| Rejection::Unavailable)?;
        let status = response.status();
        let body = timeout(
            BROKER_TIMEOUT,
            Limited::new(response.into_body(), BROKER_BODY_LIMIT).collect(),
        )
        .await
        .map_err(|_| Rejection::Unavailable)?
        .map_err(|_| Rejection::Unavailable)?
        .to_bytes();
        match parse_authn_response(status, &body) {
            AuthDecision::Allow(claims) => claims
                .into_context(
                    self.service_id.to_string(),
                    self.audiences.iter().cloned(),
                )
                .map_err(|_| Rejection::Unavailable),
            AuthDecision::Unauthenticated => Err(Rejection::Unauthenticated),
            AuthDecision::Unavailable => Err(Rejection::Unavailable),
        }
    }

    fn reject(
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
        if let Some(cors) = cors {
            merge_headers(response.headers_mut(), cors);
        }
        if let Some(request_id) = request_id {
            Self::finish_response(&mut response, request_id);
        } else {
            apply_security_headers(response.headers_mut());
        }
        response
    }

    fn finish_response(response: &mut Response<ProxyBody>, request_id: &str) {
        strip_reserved_auth_headers(response.headers_mut());
        apply_security_headers(response.headers_mut());
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response.headers_mut().insert(REQUEST_ID_HEADER, value);
        }
    }
}

fn environment(name: &'static str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing {name}"))
}

fn validate_origin(value: &str) -> Result<()> {
    let uri = value.parse::<Uri>().context("invalid URI")?;
    anyhow::ensure!(uri.scheme().is_some() && uri.authority().is_some());
    anyhow::ensure!(uri.path().is_empty() || uri.path() == "/");
    Ok(())
}

fn downstream_uri(origin: &str, incoming: &Uri) -> Result<Uri> {
    let path = incoming
        .path_and_query()
        .map_or("/", |value| value.as_str());
    format!("{origin}{path}")
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
    }
}
