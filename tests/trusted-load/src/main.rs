//! Persistent-connection load driver for the trusted-ingress topology.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use bytes::Bytes;
use hdrhistogram::Histogram;
use http::{HeaderName, HeaderValue, Method, Request, StatusCode, Uri};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::Semaphore, task::JoinSet, time::sleep_until};

type HttpClient = Client<HttpConnector, Full<Bytes>>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum LoadMode {
    ClosedLoop,
    OpenLoop,
    Fixed,
}

impl FromStr for LoadMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "closed-loop" => Ok(Self::ClosedLoop),
            "open-loop" => Ok(Self::OpenLoop),
            "fixed" => Ok(Self::Fixed),
            _ => bail!("mode must be closed-loop, open-loop, or fixed"),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Scenario {
    route: Vec<Route>,
}

#[derive(Clone, Debug, Deserialize)]
struct Route {
    name: String,
    weight: u64,
    method: String,
    path: String,
    #[serde(default)]
    credential: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: String,
    expected_status: Vec<u16>,
}

#[derive(Clone, Debug)]
struct Config {
    base_url: String,
    scenario_path: PathBuf,
    process_file: Option<PathBuf>,
    output: PathBuf,
    mode: LoadMode,
    duration: Duration,
    drain: Duration,
    concurrency: usize,
    requests: Option<u64>,
    rate: Option<u64>,
    timeout: Duration,
    warmup_requests: u64,
    seed: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct ProcessFile {
    processes: Vec<ProcessDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
struct ProcessDescriptor {
    name: String,
    pid: u32,
}

#[derive(Clone, Debug, Serialize)]
struct Report {
    schema: u32,
    configuration: ReportConfiguration,
    totals: Totals,
    statuses: BTreeMap<u16, u64>,
    errors: BTreeMap<String, u64>,
    latency_ms: LatencyGroups,
    response_headers_ms: HistogramSummary,
    first_body_byte_ms: HistogramSummary,
    routes: BTreeMap<String, RouteReport>,
    processes: BTreeMap<String, Vec<ProcessSample>>,
}

#[derive(Clone, Debug, Serialize)]
struct ReportConfiguration {
    base_url: String,
    scenario: String,
    mode: LoadMode,
    duration_seconds: f64,
    elapsed_seconds: f64,
    concurrency: usize,
    requests: Option<u64>,
    rate: Option<u64>,
    scheduled_latency_correction: bool,
    timeout_seconds: f64,
    warmup_requests: u64,
    seed: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
struct Totals {
    attempted: u64,
    completed: u64,
    status_failures: u64,
    transport_failures: u64,
    canceled: u64,
    hung: u64,
    bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
struct LatencyGroups {
    all_attempts: HistogramSummary,
    successful: HistogramSummary,
    failed: HistogramSummary,
}

#[derive(Clone, Debug, Default, Serialize)]
struct HistogramSummary {
    samples: u64,
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
struct RouteReport {
    attempted: u64,
    completed: u64,
    failures: u64,
    statuses: BTreeMap<u16, u64>,
    latency_ms: HistogramSummary,
}

#[derive(Clone, Debug, Serialize)]
struct ProcessSample {
    elapsed_seconds: f64,
    alive: bool,
    cpu_percent: Option<f64>,
    rss_kib: Option<u64>,
    file_descriptors: Option<u64>,
    sockets: Option<u64>,
}

struct Metrics {
    totals: Totals,
    statuses: BTreeMap<u16, u64>,
    errors: BTreeMap<String, u64>,
    all: Histogram<u64>,
    successful: Histogram<u64>,
    failed: Histogram<u64>,
    first_byte: Histogram<u64>,
    response_headers: Histogram<u64>,
    routes: BTreeMap<String, RouteMetrics>,
}

struct RouteMetrics {
    attempted: u64,
    completed: u64,
    failures: u64,
    statuses: BTreeMap<u16, u64>,
    latency: Histogram<u64>,
}

enum AttemptOutcome {
    Response {
        status: StatusCode,
        expected: bool,
        response_headers: Duration,
        first_body_byte: Duration,
        elapsed: Duration,
        bytes: u64,
    },
    Transport {
        class: &'static str,
        elapsed: Duration,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse()?;
    let scenario = Scenario::load(&config.scenario_path)?;
    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);
    let client = Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(config.concurrency)
        .pool_idle_timeout(Duration::from_secs(30))
        .build(connector);
    if config.warmup_requests != 0 {
        let warmup_metrics = Arc::new(Mutex::new(Metrics::new(&scenario)?));
        let mut warmup = config.clone();
        warmup.mode = LoadMode::Fixed;
        warmup.requests = Some(config.warmup_requests);
        run_load(&warmup, &scenario, client.clone(), warmup_metrics).await?;
    }
    let metrics = Arc::new(Mutex::new(Metrics::new(&scenario)?));
    let processes = Arc::new(Mutex::new(BTreeMap::new()));
    let sampling = Arc::new(AtomicBool::new(true));
    let started = Instant::now();
    let sampler = spawn_process_sampler(
        config.process_file.as_deref(),
        Arc::clone(&processes),
        Arc::clone(&sampling),
        started,
    )?;
    run_load(&config, &scenario, client, Arc::clone(&metrics)).await?;
    let measured_elapsed = started.elapsed();
    sampling.store(false, Ordering::Release);
    if let Some(sampler) = sampler {
        sampler.await.context("process sampler failed")??;
    }

    let report = build_report(&config, metrics, processes, measured_elapsed)?;
    if let Some(parent) = config.output.parent() {
        fs::create_dir_all(parent)
            .context("failed to create report directory")?;
    }
    fs::write(&config.output, serde_json::to_vec_pretty(&report)?)
        .context("failed to write load report")?;
    println!("{}", config.output.display());
    if report.totals.status_failures != 0
        || report.totals.transport_failures != 0
        || report.totals.hung != 0
    {
        std::process::exit(1);
    }
    Ok(())
}

impl Config {
    fn parse() -> Result<Self> {
        let mut values = env::args().skip(1);
        let mut arguments = BTreeMap::new();
        while let Some(name) = values.next() {
            if !name.starts_with("--") {
                bail!("unexpected argument: {name}");
            }
            let value = values
                .next()
                .with_context(|| format!("missing value for {name}"))?;
            if arguments.insert(name, value).is_some() {
                bail!("duplicate command-line option");
            }
        }
        let required = |name: &str| {
            arguments
                .get(name)
                .cloned()
                .with_context(|| format!("missing {name}"))
        };
        let parsed = |name: &str, default: &str| -> Result<u64> {
            arguments
                .get(name)
                .map_or(default, String::as_str)
                .parse()
                .with_context(|| format!("invalid {name}"))
        };
        let base_url = required("--base-url")?;
        validate_base_url(&base_url)?;
        let concurrency = usize::try_from(parsed("--concurrency", "100")?)
            .context("concurrency is too large")?;
        if concurrency == 0 {
            bail!("concurrency must be positive");
        }
        let mode = arguments
            .get("--mode")
            .map_or(Ok(LoadMode::ClosedLoop), |value| value.parse())?;
        let requests = arguments
            .get("--requests")
            .map(|value| value.parse().context("invalid --requests"))
            .transpose()?;
        let rate = arguments
            .get("--rate")
            .map(|value| value.parse().context("invalid --rate"))
            .transpose()?;
        if matches!(mode, LoadMode::OpenLoop) && rate.is_none() {
            bail!("open-loop mode requires --rate");
        }
        if matches!(mode, LoadMode::Fixed) && requests.is_none() {
            bail!("fixed mode requires --requests");
        }
        Ok(Self {
            base_url,
            scenario_path: required("--scenario")?.into(),
            process_file: arguments.get("--process-file").map(PathBuf::from),
            output: required("--output")?.into(),
            mode,
            duration: Duration::from_secs(parsed("--duration", "60")?),
            drain: Duration::from_secs(parsed("--drain", "30")?),
            concurrency,
            requests,
            rate,
            timeout: Duration::from_secs(parsed("--timeout", "10")?),
            warmup_requests: parsed("--warmup-requests", "0")?,
            seed: parsed("--seed", "0")?,
        })
    }
}

impl Scenario {
    fn load(path: &Path) -> Result<Self> {
        let source =
            fs::read_to_string(path).context("failed to read scenario")?;
        let scenario: Self =
            toml::from_str(&source).context("invalid scenario")?;
        if scenario.route.is_empty() {
            bail!("scenario must declare at least one route");
        }
        for route in &scenario.route {
            route.validate()?;
        }
        Ok(scenario)
    }

    fn select(&self, sequence: u64) -> &Route {
        let total = self.route.iter().map(|route| route.weight).sum::<u64>();
        let mut selected = sequence % total;
        for route in &self.route {
            if selected < route.weight {
                return route;
            }
            selected -= route.weight;
        }
        &self.route[0]
    }
}

impl Route {
    fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.weight == 0
            || !self.path.starts_with('/')
        {
            bail!("route name, weight, or path is invalid");
        }
        Method::from_bytes(self.method.as_bytes())
            .context("invalid route method")?;
        if self.expected_status.is_empty()
            || self
                .expected_status
                .iter()
                .any(|status| StatusCode::from_u16(*status).is_err())
        {
            bail!("route expected statuses are invalid");
        }
        for (name, value) in &self.headers {
            HeaderName::from_bytes(name.as_bytes())
                .context("invalid route header name")?;
            HeaderValue::from_str(value)
                .context("invalid route header value")?;
        }
        Ok(())
    }
}

impl Metrics {
    fn new(scenario: &Scenario) -> Result<Self> {
        let mut routes = BTreeMap::new();
        for route in &scenario.route {
            if routes
                .insert(
                    route.name.clone(),
                    RouteMetrics {
                        attempted: 0,
                        completed: 0,
                        failures: 0,
                        statuses: BTreeMap::new(),
                        latency: histogram()?,
                    },
                )
                .is_some()
            {
                bail!("route names must be unique");
            }
        }
        Ok(Self {
            totals: Totals::default(),
            statuses: BTreeMap::new(),
            errors: BTreeMap::new(),
            all: histogram()?,
            successful: histogram()?,
            failed: histogram()?,
            first_byte: histogram()?,
            response_headers: histogram()?,
            routes,
        })
    }

    fn record_attempt(&mut self, route: &str) {
        self.totals.attempted += 1;
        if let Some(metrics) = self.routes.get_mut(route) {
            metrics.attempted += 1;
        }
    }

    fn record(&mut self, route: &str, outcome: AttemptOutcome) {
        match outcome {
            AttemptOutcome::Response {
                status,
                expected,
                response_headers,
                first_body_byte,
                elapsed,
                bytes,
            } => {
                self.totals.completed += 1;
                self.totals.bytes += bytes;
                *self.statuses.entry(status.as_u16()).or_default() += 1;
                record_duration(&mut self.all, elapsed);
                record_duration(&mut self.response_headers, response_headers);
                record_duration(&mut self.first_byte, first_body_byte);
                if let Some(route_metrics) = self.routes.get_mut(route) {
                    route_metrics.completed += 1;
                    *route_metrics
                        .statuses
                        .entry(status.as_u16())
                        .or_default() += 1;
                    record_duration(&mut route_metrics.latency, elapsed);
                    if !expected {
                        route_metrics.failures += 1;
                    }
                }
                if expected {
                    record_duration(&mut self.successful, elapsed);
                } else {
                    self.totals.status_failures += 1;
                    record_duration(&mut self.failed, elapsed);
                }
            }
            AttemptOutcome::Transport { class, elapsed } => {
                self.totals.transport_failures += 1;
                *self.errors.entry(class.to_owned()).or_default() += 1;
                record_duration(&mut self.all, elapsed);
                record_duration(&mut self.failed, elapsed);
                if let Some(route_metrics) = self.routes.get_mut(route) {
                    route_metrics.failures += 1;
                    record_duration(&mut route_metrics.latency, elapsed);
                }
            }
        }
    }
}

async fn run_load(
    config: &Config,
    scenario: &Scenario,
    client: HttpClient,
    metrics: Arc<Mutex<Metrics>>,
) -> Result<()> {
    let scenario = Arc::new(scenario.clone());
    let sequence = Arc::new(AtomicU64::new(config.seed));
    let deadline = Instant::now() + config.duration;
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut tasks = JoinSet::new();
    match config.mode {
        LoadMode::ClosedLoop => {
            for _ in 0..config.concurrency {
                tasks.spawn(closed_loop_worker(
                    config.clone(),
                    Arc::clone(&scenario),
                    client.clone(),
                    Arc::clone(&metrics),
                    Arc::clone(&sequence),
                    deadline,
                ));
            }
        }
        LoadMode::Fixed => {
            let requests =
                config.requests.context("fixed request count missing")?;
            for _ in 0..requests {
                let permit = Arc::clone(&semaphore)
                    .acquire_owned()
                    .await
                    .context("load semaphore closed")?;
                spawn_attempt(
                    &mut tasks,
                    config.clone(),
                    Arc::clone(&scenario),
                    client.clone(),
                    Arc::clone(&metrics),
                    Arc::clone(&sequence),
                    permit,
                    None,
                );
            }
        }
        LoadMode::OpenLoop => {
            let rate = config.rate.context("open-loop rate missing")?;
            if rate == 0 {
                bail!("open-loop rate must be positive");
            }
            let interval = Duration::from_secs_f64(1.0 / rate as f64);
            let started = Instant::now();
            let mut scheduled = 0_u64;
            while Instant::now() < deadline
                && config.requests.is_none_or(|limit| scheduled < limit)
            {
                let scheduled_at = started + interval.mul_f64(scheduled as f64);
                sleep_until(scheduled_at.into()).await;
                let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned()
                else {
                    let mut metrics = metrics
                        .lock()
                        .map_err(|_| anyhow::anyhow!("metrics lock"))?;
                    let index = sequence.fetch_add(1, Ordering::Relaxed);
                    let route = scenario.select(index);
                    metrics.record_attempt(&route.name);
                    metrics.record(
                        &route.name,
                        AttemptOutcome::Transport {
                            class: "client_saturated",
                            elapsed: scheduled_at.elapsed(),
                        },
                    );
                    scheduled += 1;
                    continue;
                };
                spawn_attempt(
                    &mut tasks,
                    config.clone(),
                    Arc::clone(&scenario),
                    client.clone(),
                    Arc::clone(&metrics),
                    Arc::clone(&sequence),
                    permit,
                    Some(scheduled_at),
                );
                scheduled += 1;
            }
        }
    }

    let drain_deadline = task_drain_deadline(
        config.mode,
        deadline,
        Instant::now(),
        config.drain,
    );
    loop {
        if tasks.is_empty() {
            break;
        }
        if tokio::time::timeout_at(drain_deadline.into(), tasks.join_next())
            .await
            .is_err()
        {
            let hung = u64::try_from(tasks.len()).unwrap_or(u64::MAX);
            tasks.abort_all();
            let mut metrics = metrics
                .lock()
                .map_err(|_| anyhow::anyhow!("metrics lock"))?;
            metrics.totals.hung += hung;
            metrics.totals.canceled += hung;
            break;
        }
    }
    Ok(())
}

fn task_drain_deadline(
    mode: LoadMode,
    load_deadline: Instant,
    scheduling_finished: Instant,
    drain: Duration,
) -> Instant {
    match mode {
        LoadMode::ClosedLoop => load_deadline + drain,
        LoadMode::OpenLoop | LoadMode::Fixed => scheduling_finished + drain,
    }
}

async fn closed_loop_worker(
    config: Config,
    scenario: Arc<Scenario>,
    client: HttpClient,
    metrics: Arc<Mutex<Metrics>>,
    sequence: Arc<AtomicU64>,
    deadline: Instant,
) {
    while Instant::now() < deadline {
        run_attempt(&config, &scenario, &client, &metrics, &sequence, None)
            .await;
    }
}

fn spawn_attempt(
    tasks: &mut JoinSet<()>,
    config: Config,
    scenario: Arc<Scenario>,
    client: HttpClient,
    metrics: Arc<Mutex<Metrics>>,
    sequence: Arc<AtomicU64>,
    permit: tokio::sync::OwnedSemaphorePermit,
    scheduled_at: Option<Instant>,
) {
    tasks.spawn(async move {
        let _permit = permit;
        run_attempt(
            &config,
            &scenario,
            &client,
            &metrics,
            &sequence,
            scheduled_at,
        )
        .await;
    });
}

async fn run_attempt(
    config: &Config,
    scenario: &Scenario,
    client: &HttpClient,
    metrics: &Mutex<Metrics>,
    sequence: &AtomicU64,
    scheduled_at: Option<Instant>,
) {
    let index = sequence.fetch_add(1, Ordering::Relaxed);
    let route = scenario.select(index);
    if let Ok(mut metrics) = metrics.lock() {
        metrics.record_attempt(&route.name);
    }
    let outcome = execute(config, route, client, scheduled_at).await;
    if let Ok(mut metrics) = metrics.lock() {
        metrics.record(&route.name, outcome);
    }
}

async fn execute(
    config: &Config,
    route: &Route,
    client: &HttpClient,
    scheduled_at: Option<Instant>,
) -> AttemptOutcome {
    let started = Instant::now();
    let latency_origin = scheduled_at.unwrap_or(started);
    let request = match build_request(config, route) {
        Ok(request) => request,
        Err(_) => {
            return AttemptOutcome::Transport {
                class: "protocol",
                elapsed: latency_origin.elapsed(),
            };
        }
    };
    let deadline = tokio::time::Instant::now() + config.timeout;
    let response = match tokio::time::timeout_at(
        deadline,
        client.request(request),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return AttemptOutcome::Transport {
                class: classify_transport(&error.to_string()),
                elapsed: latency_origin.elapsed(),
            };
        }
        Err(_) => {
            return AttemptOutcome::Transport {
                class: "timeout",
                elapsed: latency_origin.elapsed(),
            };
        }
    };
    let response_headers = latency_origin.elapsed();
    collect_response(
        route,
        response,
        latency_origin,
        response_headers,
        deadline,
    )
    .await
}

async fn collect_response(
    route: &Route,
    response: http::Response<Incoming>,
    started: Instant,
    response_headers: Duration,
    deadline: tokio::time::Instant,
) -> AttemptOutcome {
    let status = response.status();
    let expected = route.expected_status.contains(&status.as_u16());
    let mut body = response.into_body();
    let mut first_byte = None;
    let mut bytes = 0_u64;
    let collected = tokio::time::timeout_at(deadline, async {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| ())?;
            if let Some(data) = frame.data_ref() {
                first_byte.get_or_insert_with(|| started.elapsed());
                bytes = bytes.saturating_add(
                    u64::try_from(data.len()).unwrap_or(u64::MAX),
                );
            }
        }
        Ok::<(), ()>(())
    })
    .await;
    match collected {
        Ok(Ok(())) => AttemptOutcome::Response {
            status,
            expected,
            response_headers,
            first_body_byte: first_byte.unwrap_or(response_headers),
            elapsed: started.elapsed(),
            bytes,
        },
        Ok(Err(())) => AttemptOutcome::Transport {
            class: "incomplete_body",
            elapsed: started.elapsed(),
        },
        Err(_) => AttemptOutcome::Transport {
            class: "timeout",
            elapsed: started.elapsed(),
        },
    }
}

fn build_request(
    config: &Config,
    route: &Route,
) -> Result<Request<Full<Bytes>>> {
    let uri =
        format!("{}{}", config.base_url.trim_end_matches('/'), route.path)
            .parse::<Uri>()
            .context("invalid request URI")?;
    let method = Method::from_bytes(route.method.as_bytes())
        .context("invalid method")?;
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Full::new(Bytes::copy_from_slice(route.body.as_bytes())))?;
    for (name, value) in &route.headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }
    if let Some(credential) = &route.credential {
        let key = format!(
            "TRUSTED_LOAD_CREDENTIAL_{}",
            credential.to_ascii_uppercase()
        );
        let value = env::var(&key).with_context(|| {
            format!("missing credential alias {credential}")
        })?;
        request.headers_mut().insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&value)
                .context("invalid credential header")?,
        );
    }
    Ok(request)
}

fn build_report(
    config: &Config,
    metrics: Arc<Mutex<Metrics>>,
    processes: Arc<Mutex<BTreeMap<String, Vec<ProcessSample>>>>,
    elapsed: Duration,
) -> Result<Report> {
    let metrics = Arc::try_unwrap(metrics)
        .map_err(|_| anyhow::anyhow!("load metrics still referenced"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("metrics lock poisoned"))?;
    let routes = metrics
        .routes
        .into_iter()
        .map(|(name, metrics)| {
            (
                name,
                RouteReport {
                    attempted: metrics.attempted,
                    completed: metrics.completed,
                    failures: metrics.failures,
                    statuses: metrics.statuses,
                    latency_ms: summarize(&metrics.latency),
                },
            )
        })
        .collect();
    let process_samples = Arc::try_unwrap(processes)
        .map_err(|_| anyhow::anyhow!("process samples still referenced"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("process lock poisoned"))?;
    Ok(Report {
        schema: 2,
        configuration: ReportConfiguration {
            base_url: config.base_url.clone(),
            scenario: config.scenario_path.display().to_string(),
            mode: config.mode,
            duration_seconds: config.duration.as_secs_f64(),
            elapsed_seconds: elapsed.as_secs_f64(),
            concurrency: config.concurrency,
            requests: config.requests,
            rate: config.rate,
            scheduled_latency_correction: matches!(
                config.mode,
                LoadMode::OpenLoop
            ),
            timeout_seconds: config.timeout.as_secs_f64(),
            warmup_requests: config.warmup_requests,
            seed: config.seed,
        },
        totals: metrics.totals,
        statuses: metrics.statuses,
        errors: metrics.errors,
        latency_ms: LatencyGroups {
            all_attempts: summarize(&metrics.all),
            successful: summarize(&metrics.successful),
            failed: summarize(&metrics.failed),
        },
        response_headers_ms: summarize(&metrics.response_headers),
        first_body_byte_ms: summarize(&metrics.first_byte),
        routes,
        processes: process_samples,
    })
}

fn spawn_process_sampler(
    path: Option<&Path>,
    output: Arc<Mutex<BTreeMap<String, Vec<ProcessSample>>>>,
    sampling: Arc<AtomicBool>,
    started: Instant,
) -> Result<Option<tokio::task::JoinHandle<Result<()>>>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let document: ProcessFile = serde_json::from_slice(
        &fs::read(path).context("failed to read process descriptor")?,
    )
    .context("invalid process descriptor")?;
    Ok(Some(tokio::spawn(async move {
        while sampling.load(Ordering::Acquire) {
            for process in &document.processes {
                let sample = process_sample(process.pid, started.elapsed());
                output
                    .lock()
                    .map_err(|_| {
                        anyhow::anyhow!("process sample lock poisoned")
                    })?
                    .entry(process.name.clone())
                    .or_default()
                    .push(sample);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    })))
}

fn process_sample(pid: u32, elapsed: Duration) -> ProcessSample {
    let output = Command::new("ps")
        .args(["-o", "%cpu=,rss=", "-p", &pid.to_string()])
        .output();
    let values = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    let mut values = values.as_deref().unwrap_or_default().split_whitespace();
    let cpu_percent = values.next().and_then(|value| value.parse().ok());
    let rss_kib = values.next().and_then(|value| value.parse().ok());
    let proc_fd = PathBuf::from(format!("/proc/{pid}/fd"));
    let (file_descriptors, sockets) = if proc_fd.is_dir() {
        let entries = fs::read_dir(proc_fd)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        let sockets = entries
            .iter()
            .filter(|entry| {
                fs::read_link(entry.path()).ok().is_some_and(|target| {
                    target.to_string_lossy().starts_with("socket:[")
                })
            })
            .count();
        (
            u64::try_from(entries.len()).ok(),
            u64::try_from(sockets).ok(),
        )
    } else {
        (None, None)
    };
    ProcessSample {
        elapsed_seconds: elapsed.as_secs_f64(),
        alive: rss_kib.is_some(),
        cpu_percent,
        rss_kib,
        file_descriptors,
        sockets,
    }
}

fn histogram() -> Result<Histogram<u64>> {
    Histogram::new_with_bounds(1, 60_000_000, 3)
        .context("failed to create latency histogram")
}

fn record_duration(histogram: &mut Histogram<u64>, duration: Duration) {
    let micros = u64::try_from(duration.as_micros())
        .unwrap_or(u64::MAX)
        .max(1);
    let _ = histogram.record(micros.min(60_000_000));
}

fn summarize(histogram: &Histogram<u64>) -> HistogramSummary {
    if histogram.is_empty() {
        return HistogramSummary::default();
    }
    HistogramSummary {
        samples: histogram.len(),
        mean: histogram.mean() / 1000.0,
        p50: histogram.value_at_quantile(0.50) as f64 / 1000.0,
        p95: histogram.value_at_quantile(0.95) as f64 / 1000.0,
        p99: histogram.value_at_quantile(0.99) as f64 / 1000.0,
        max: histogram.max() as f64 / 1000.0,
    }
}

fn classify_transport(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("reset") {
        "reset"
    } else if lower.contains("connect") {
        "connect"
    } else if lower.contains("incomplete") {
        "incomplete_headers"
    } else if lower.contains("cancel") {
        "canceled"
    } else {
        "protocol"
    }
}

fn validate_base_url(value: &str) -> Result<()> {
    let uri = value.parse::<Uri>().context("invalid base URL")?;
    if uri.scheme_str() != Some("http")
        || uri.authority().is_none()
        || uri.path() != "/"
    {
        bail!("base URL must be an HTTP origin without a path");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_selection_should_repeat_deterministically() {
        let scenario = Scenario {
            route: vec![fixture_route("first", 2), fixture_route("second", 1)],
        };

        let selected = (0..6)
            .map(|index| scenario.select(index).name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            selected,
            ["first", "first", "second", "first", "first", "second"]
        );
    }

    #[test]
    fn transport_classification_should_not_expose_error_text() {
        assert_eq!(classify_transport("connection reset by peer"), "reset");
    }

    #[test]
    fn closed_loop_drain_should_start_after_the_load_deadline() {
        let started = Instant::now();
        let load_deadline = started + Duration::from_secs(600);
        let scheduling_finished = started + Duration::from_millis(10);
        let drain = Duration::from_secs(30);

        assert_eq!(
            task_drain_deadline(
                LoadMode::ClosedLoop,
                load_deadline,
                scheduling_finished,
                drain,
            ),
            started + Duration::from_secs(630)
        );
        assert_eq!(
            task_drain_deadline(
                LoadMode::Fixed,
                load_deadline,
                scheduling_finished,
                drain,
            ),
            scheduling_finished + drain
        );
    }

    fn fixture_route(name: &str, weight: u64) -> Route {
        Route {
            name: name.to_owned(),
            weight,
            method: "GET".to_owned(),
            path: "/".to_owned(),
            credential: None,
            headers: BTreeMap::new(),
            body: String::new(),
            expected_status: vec![200],
        }
    }
}
