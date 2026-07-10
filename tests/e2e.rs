use futures::StreamExt as _;
use reqwest::StatusCode;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[derive(Clone, Copy)]
struct RuntimeCapabilities {
    static_files: bool,
    request_body_limit: bool,
    panic_returns_status: bool,
    chunked_streaming: bool,
}

const WASMTIME_CAPABILITIES: RuntimeCapabilities = RuntimeCapabilities {
    static_files: true,
    request_body_limit: true,
    panic_returns_status: true,
    chunked_streaming: true,
};

const SPIN_CAPABILITIES: RuntimeCapabilities = RuntimeCapabilities {
    static_files: true,
    request_body_limit: true,
    panic_returns_status: false,
    chunked_streaming: false,
};

struct WasmtimeServer {
    child: Child,
    port: u16,
    stderr_log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for WasmtimeServer {
    fn drop(&mut self) {
        println!("Stopping Wasmtime server on port {}", self.port);
        let _ = self.child.kill();
        let _ = self.child.wait();

        if std::thread::panicking()
            && let Ok(log) = self.stderr_log.lock()
        {
            eprintln!(
                "\n--- Wasmtime Server Stderr on port {} (test failed) ---",
                self.port
            );
            for line in log.iter() {
                eprintln!("{}", line);
            }
            eprintln!("--------------------------------------------------");
        }
    }
}

fn start_server(
    wasm_path: &str,
    is_p3: bool,
) -> anyhow::Result<WasmtimeServer> {
    let mut args =
        vec!["serve".to_string(), "-S".to_string(), "cli=y".to_string()];
    if is_p3 {
        args.push("-S".to_string());
        args.push("p3=y".to_string());
        args.push("-W".to_string());
        args.push("component-model-async=y".to_string());
    }
    if wasm_path.contains("middleware") {
        let broker_url = std::env::var("WASI_MIDDLEWARE_AUTHN_BROKER_URL")
            .unwrap_or_else(|_| {
                "http://127.0.0.1:3112/authenticate".to_string()
            });
        args.extend([
            "-S".to_string(),
            "http=y".to_string(),
            "-S".to_string(),
            "inherit-network=y".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_CORS_ORIGINS=http://127.0.0.1:3110,http://127.0.0.1:3111"
                .to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_CORS_METHODS=GET,HEAD,POST".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_CORS_HEADERS=content-type,authorization,x-request-id"
                .to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_CORS_ALLOW_CREDENTIALS=false".to_string(),
            "--env".to_string(),
            format!("WASI_MIDDLEWARE_AUTHN_BROKER_URL={broker_url}"),
            "--env".to_string(),
            "WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=2000".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_AUTHN_MODE=optional".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-test-app".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_AUTHN_AUDIENCES=leptos-wasi-test-app".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT=64".to_string(),
            "--env".to_string(),
            "WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true".to_string(),
        ]);
    }

    // Mount the static files directory to the virtual filesystem in the guest.
    args.push("--dir".to_string());
    args.push("tests/test-app/static::/static".to_string());

    args.push(wasm_path.to_string());
    args.push("--addr".to_string());
    args.push("127.0.0.1:0".to_string());

    println!("Spawning wasmtime {:?}", args);
    let wasmtime =
        std::env::var_os("WASMTIME_BIN").unwrap_or_else(|| "wasmtime".into());
    let mut child = Command::new(wasmtime)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to take stderr"))?;
    let mut reader = BufReader::new(stderr);

    let mut port = None;
    let mut line = String::new();
    let stderr_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_log_clone = stderr_log.clone();

    for _ in 0..100 {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim().to_string();
        if let Ok(mut log) = stderr_log_clone.lock() {
            log.push(trimmed.clone());
        }
        println!("wasmtime output: {}", trimmed);
        if line.contains("Serving HTTP on") && line.rfind(':').is_some() {
            let pos = line.rfind(':').unwrap();
            let port_str = line[pos + 1..].trim().trim_end_matches('/');
            if let Ok(p) = port_str.parse::<u16>() {
                port = Some(p);
                break;
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(anyhow::anyhow!(
                "wasmtime serve exited early with status: {}",
                status
            ));
        }
    }

    let port = port.ok_or_else(|| {
        anyhow::anyhow!("Failed to parse port from wasmtime serve output")
    })?;

    // Spawn thread to drain the rest of stderr so wasmtime does not block on write
    std::thread::spawn(move || {
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line) {
            if n == 0 {
                break;
            }
            let trimmed = line.trim().to_string();
            if let Ok(mut log) = stderr_log_clone.lock() {
                log.push(trimmed);
            }
            line.clear();
        }
    });

    Ok(WasmtimeServer {
        child,
        port,
        stderr_log,
    })
}

async fn raw_http_status(port: u16, path: &str) -> anyhow::Result<StatusCode> {
    if path.contains(['\r', '\n']) {
        anyhow::bail!("raw request path must not contain CR or LF");
    }

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    raw_request_status(port, request).await
}

async fn raw_chunked_body_status(
    port: u16,
    path: &str,
    body: &[u8],
) -> anyhow::Result<StatusCode> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    let headers = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes()).await?;
    for chunk in body.chunks(4096) {
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(chunk).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    read_raw_status(&mut stream).await
}

async fn raw_slow_body_status(
    port: u16,
    path: &str,
    body: &[u8],
) -> anyhow::Result<StatusCode> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    let headers = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    for chunk in body.chunks(2048) {
        stream.write_all(chunk).await?;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    read_raw_status(&mut stream).await
}

async fn disconnect_during_upload(
    port: u16,
    path: &str,
    declared_length: usize,
) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    let headers = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(br#"{"data":"partial"#).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn raw_request_status(
    port: u16,
    request: String,
) -> anyhow::Result<StatusCode> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(request.as_bytes()).await?;

    read_raw_status(&mut stream).await
}

async fn read_raw_status(stream: &mut TcpStream) -> anyhow::Result<StatusCode> {
    let mut response = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream.read_to_end(&mut response),
    )
    .await??;

    let status_line = response
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or_else(|| anyhow::anyhow!("raw HTTP response was empty"))?;
    let status_line = std::str::from_utf8(status_line)?.trim_end();
    let status = status_line
        .split_ascii_whitespace()
        .nth(1)
        .ok_or_else(|| {
            anyhow::anyhow!("raw HTTP response had no status: {status_line}")
        })?
        .parse::<u16>()?;
    Ok(StatusCode::from_u16(status)?)
}

async fn run_assertions(
    port: u16,
    capabilities: RuntimeCapabilities,
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none()) // Don't auto-follow redirects so we can test 302 locations
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", port);
    println!("Running assertions against {}", base_url);

    // 1. GET /api/get_test
    {
        let res = client
            .get(format!("{}/api/get_test", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("GET response"),
            "Expected 'GET response', got: {}",
            text
        );
    }

    // 2. POST /api/post_test
    {
        let res = client
            .post(format!("{}/api/post_test", base_url))
            .header("Content-Type", "application/json")
            .body(r#"{"msg": "hello"}"#)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("POST response: hello"),
            "Expected 'POST response: hello', got: {}",
            text
        );
    }

    // 3. POST /api/custom_test
    {
        let res = client
            .post(format!("{}/api/custom_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("Custom response"),
            "Expected 'Custom response', got: {}",
            text
        );
    }

    // Server-function middleware runs in the same order as upstream Leptos.
    {
        let res = client
            .post(format!("{}/api/middleware_header_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let order = res
            .headers()
            .get_all("x-middleware-order")
            .iter()
            .map(|value| value.to_str())
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(order, ["first", "second"]);
        assert!(res.text().await?.contains("middleware response"));
    }

    // Authentication middleware rejects missing credentials and permits an
    // explicitly authorized request.
    {
        let rejected = client
            .post(format!("{}/api/auth_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert!(rejected.text().await?.contains("authentication required"));

        let accepted = client
            .post(format!("{}/api/auth_test", base_url))
            .header("Content-Type", "application/json")
            .header("x-test-auth", "allowed")
            .body("{}")
            .send()
            .await?;
        assert_eq!(accepted.status(), StatusCode::OK);
        assert!(accepted.text().await?.contains("authenticated response"));
    }

    // Middleware failures use the server function's configured serializer.
    {
        let res = client
            .post(format!("{}/api/middleware_error_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(res.text().await?.contains("intentional middleware failure"));
    }

    // 5. POST /api/panic_test
    {
        let res = client
            .post(format!("{}/api/panic_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await;
        if capabilities.panic_returns_status {
            // Wasmtime returns 500 Internal Server Error
            let res = res?;
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        } else {
            // Spin may drop the connection or return a non-200 status
            match res {
                Ok(res) => assert_ne!(
                    res.status(),
                    StatusCode::OK,
                    "Panic endpoint should not return 200 OK"
                ),
                Err(_) => { /* connection dropped — acceptable for Spin */ }
            }
        }
    }

    // 6. POST /api/form_submit_test (with Referer)
    {
        let res = client
            .post(format!("{}/api/form_submit_test", base_url))
            .header("Accept", "text/html")
            .header("Referer", "http://127.0.0.1/previous-page")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res
            .headers()
            .get("Location")
            .expect("Location header missing");
        let loc_clean = loc.to_str()?.trim_end_matches('?');
        assert!(
            loc_clean == "/previous-page"
                || loc_clean.ends_with("/previous-page"),
            "Expected relative /previous-page or absolute ending with \
             /previous-page, got: {}",
            loc_clean
        );
    }

    // 7. POST /api/form_submit_test (with Referrer spelling)
    {
        let res = client
            .post(format!("{}/api/form_submit_test", base_url))
            .header("Accept", "text/html")
            .header("Referrer", "http://127.0.0.1/other-page")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res
            .headers()
            .get("Location")
            .expect("Location header missing");
        let loc_clean = loc.to_str()?.trim_end_matches('?');
        assert!(
            loc_clean == "/other-page" || loc_clean.ends_with("/other-page"),
            "Expected relative /other-page or absolute ending with \
             /other-page, got: {}",
            loc_clean
        );
    }

    // Adversarial Test 1: Open Redirect Vulnerability Prevention
    {
        let res = client
            .post(format!("{}/api/form_submit_test", base_url))
            .header("Accept", "text/html")
            .header("Referer", "https://malicious.example.com/steal-session")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res
            .headers()
            .get("Location")
            .expect("Location header missing");
        let loc_clean = loc.to_str()?.trim_end_matches('?');
        assert!(
            loc_clean == "/steal-session"
                || loc_clean.ends_with("/steal-session"),
            "Expected relative /steal-session or absolute ending with \
             /steal-session, got: {}",
            loc_clean
        );
    }

    // Adversarial Test 1b: Open Redirect Backslash Bypass Prevention
    {
        let backslash_referrers = vec![
            "http://127.0.0.1/\\\\evil.com/steal-session",
            "http://127.0.0.1/%5C%5Cevil.com/steal-session",
        ];
        for ref_url in backslash_referrers {
            let res = client
                .post(format!("{}/api/form_submit_test", base_url))
                .header("Accept", "text/html")
                .header("Referer", ref_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send()
                .await?;
            assert_eq!(res.status(), StatusCode::FOUND);
            let loc = res
                .headers()
                .get("Location")
                .expect("Location header missing");
            let loc_clean = loc.to_str()?.trim_end_matches('?');
            assert!(
                loc_clean == "/"
                    || loc_clean.ends_with('/')
                    || (!loc_clean.contains("evil.com")
                        && !loc_clean.contains('\\')),
                "Expected location to fall back to '/' or at least not \
                 contain evil.com or backslashes, got: {}",
                loc_clean
            );
        }
    }

    // Adversarial Test 2: Process Panic on Malformed Header Redirects Prevention
    {
        let res = client
            .post(format!("{}/api/malformed_redirect_test", base_url))
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    // Adversarial Test 3: Overlapping Prefix Route Hijacking Prevention
    {
        let res = client
            .get(format!("{}/static-page", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("Static Page SSR View"),
            "Expected 'Static Page SSR View', got: {}",
            text
        );
    }

    // Adversarial Test 4: URL Decoded Path Serving (static files only)
    if capabilities.static_files {
        let res = client
            .get(format!("{}/static/my%20file.css", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert_eq!(text.trim(), "body { background: blue; }");
    }

    // Islands router requests receive navigation context while normal SSR does not.
    {
        let res = client.get(format!("{}/ssr/async", base_url)).send().await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("data-islands-router-navigation=\"false\""),
            "Expected a normal SSR response, got: {}",
            text
        );
    }

    {
        let res = client
            .get(format!("{}/ssr/async", base_url))
            .header("Islands-Router", "1")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("data-islands-router-navigation=\"true\""),
            "Expected islands-router navigation context, got: {}",
            text
        );
    }

    // SSR receives the complete path and query, and application context is
    // installed after the standard Leptos request contexts.
    {
        let res = client
            .get(format!("{}/ssr/query?name=leptos%20wasi", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains("/ssr/query?name=leptos%20wasi"),
            "expected the SSR request URL to include its query: {text}"
        );
        assert!(
            text.contains("id=\"standard-contexts-visible\">true"),
            "expected standard contexts before application context: {text}"
        );
    }

    // 8. Static Files Content-Types, 404, zero-byte file, path traversal
    if capabilities.static_files {
        let static_js_length =
            include_bytes!("test-app/static/app.js").len().to_string();
        // js
        let res = client
            .get(format!("{}/static/app.js", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()?
                .contains("javascript")
        );
        assert_eq!(
            res.headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            res.headers()
                .get("Content-Length")
                .and_then(|value| value.to_str().ok()),
            Some(static_js_length.as_str())
        );
        assert!(res.text().await?.contains("console.log"));

        // HEAD returns the GET metadata without a response body.
        let res = client
            .head(format!("{}/static/app.js", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("javascript"))
        );
        assert_eq!(
            res.headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            res.headers()
                .get("Content-Length")
                .and_then(|value| value.to_str().ok()),
            Some(static_js_length.as_str())
        );
        assert!(res.bytes().await?.is_empty());

        // Methods other than GET and HEAD are rejected deterministically.
        let res = client
            .post(format!("{}/static/app.js", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            res.headers()
                .get("Allow")
                .and_then(|value| value.to_str().ok()),
            Some("GET, HEAD")
        );

        // A component containing two dots is a valid filename, not traversal.
        let res = client
            .get(format!("{}/static/app..min.js", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.text().await?.contains("valid double-dot filename"));

        // css
        let res = client
            .get(format!("{}/static/app.css", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()?
                .contains("css")
        );

        // html
        let res = client
            .get(format!("{}/static/app.html", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()?
                .contains("html")
        );

        // wasm
        let res = client
            .get(format!("{}/static/app.wasm", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()?
                .contains("wasm")
        );

        // zero-byte
        let res = client
            .get(format!("{}/static/zero.txt", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.bytes().await?.len(), 0);

        // nested icons/logo.svg
        let res = client
            .get(format!("{}/static/assets/icons/logo.svg", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()?
                .contains("svg")
        );

        // 404 missing file
        let res = client
            .get(format!("{}/static/does-not-exist.png", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // Send paths over a raw TCP connection so the HTTP client cannot
        // normalize dot segments or encoded separators before the host sees
        // them.
        let unsafe_paths = [
            "/static/%2Fetc/passwd",
            "/static/%2f..%2fCargo.toml",
            "/static/%2e%2e/Cargo.toml",
            "/static/%5cCargo.toml",
            "/static/%00",
            "/static/%",
            "/static/%252e%252e%252fCargo.toml",
            "/static/../Cargo.toml",
        ];
        for path in unsafe_paths {
            let status = raw_http_status(port, path).await?;
            assert!(
                matches!(
                    status,
                    StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
                ),
                "unsafe static path {path} returned {status}"
            );
        }
    }

    // A streaming response must expose its first frame before its delayed
    // second frame is ready.
    {
        let started = Instant::now();
        let res = client
            .get(format!("{}/static/delayed.stream", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let mut body = res.bytes_stream();
        let first =
            tokio::time::timeout(Duration::from_millis(350), body.next())
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "delayed stream ended before its first frame"
                    )
                })??;
        assert_eq!(first.as_ref(), b"first-frame\n");
        assert!(
            started.elapsed() < Duration::from_millis(350),
            "first frame arrived only after the delayed frame was eligible"
        );

        let mut remaining = Vec::new();
        while let Some(frame) = body.next().await {
            remaining.extend_from_slice(&frame?);
        }
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert_eq!(remaining, b"second-frame\n");
    }

    // A producer failure after commitment must end that body and leave the
    // runtime able to serve the next request.
    {
        let response = client
            .get(format!("{}/static/failing.stream", base_url))
            .send()
            .await;
        if let Ok(res) = response {
            assert_eq!(res.status(), StatusCode::OK);
            let mut body = res.bytes_stream();
            let first = tokio::time::timeout(
                Duration::from_secs(5),
                body.next(),
            )
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "committed failing stream ended before its first frame"
                )
            })?
            .map_err(|error| {
                anyhow::anyhow!(
                    "committed failing stream errored before its first frame: {error}"
                )
            })?;
            assert_eq!(first.as_ref(), b"first-frame\n");

            match tokio::time::timeout(Duration::from_secs(5), body.next())
                .await?
            {
                None | Some(Err(_)) => {}
                Some(Ok(frame)) => anyhow::bail!(
                    "failing stream produced an unexpected frame: {frame:?}"
                ),
            }
        }

        let healthy = tokio::time::timeout(
            Duration::from_secs(5),
            client.get(format!("{}/api/get_test", base_url)).send(),
        )
        .await??;
        assert_eq!(healthy.status(), StatusCode::OK);
    }

    // 9. SSR Modes
    {
        // Async mode
        let res = client.get(format!("{}/ssr/async", base_url)).send().await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(text.contains("Async View"));
        assert!(text.contains("Async resource resolved"));

        let res = client
            .head(format!("{}/ssr/async", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.bytes().await?.is_empty());

        // InOrder mode
        let res = client
            .get(format!("{}/ssr/in-order", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(text.contains("InOrder View"));
        assert!(text.contains("InOrder resource resolved"));

        // OutOfOrder mode (with chunked encoding assertion)
        let res = client
            .get(format!("{}/ssr/out-of-order", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        if capabilities.chunked_streaming {
            // Only assert chunked encoding for Wasmtime; Spin may buffer responses
            let is_chunked = res
                .headers()
                .get("Transfer-Encoding")
                .map(|v| v.to_str().unwrap().contains("chunked"))
                .unwrap_or(false);
            assert!(
                is_chunked,
                "OutOfOrder SSR mode must use chunked transfer encoding"
            );
        }
        let text = res.text().await?;
        assert!(text.contains("OutOfOrder View"));
        assert!(text.contains("OutOfOrder resource resolved"));

        // Meta tags
        let res = client.get(format!("{}/ssr/meta", base_url)).send().await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(text.contains("<title>Meta Test Title</title>"));
        assert!(text.contains(
            r#"<meta name="description" content="Meta Test Description""#
        ));

        // SSR Panic
        let res = client.get(format!("{}/ssr/panic", base_url)).send().await;
        if capabilities.panic_returns_status {
            // Wasmtime returns 500 Internal Server Error
            let res = res?;
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        } else {
            // Spin may drop the connection or return a non-200 status
            match res {
                Ok(res) => assert_ne!(
                    res.status(),
                    StatusCode::OK,
                    "SSR panic endpoint should not return 200 OK"
                ),
                Err(_) => { /* connection dropped — acceptable for Spin */ }
            }
        }
    }

    // 10. Request Body size limit check (Wasmtime only — Spin has its own limits)
    if capabilities.request_body_limit {
        const LIMIT: usize = 64 * 1024;
        const JSON_OVERHEAD: usize = r#"{"data":""}"#.len();

        // The test guest uses a 64 KiB policy so an exact-boundary request and
        // its limit + 1 counterpart both fit beneath host request limits.
        println!("Testing exact 64 KiB payload boundary...");
        let data_len = LIMIT - JSON_OVERHEAD;
        let body_json = format!(r#"{{"data":"{}"}}"#, "a".repeat(data_len));
        assert_eq!(body_json.len(), LIMIT);
        let res = client
            .post(format!("{}/api/large_body_test", base_url))
            .header("Content-Type", "application/json")
            .body(body_json)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let text = res.text().await?;
        assert!(
            text.contains(&data_len.to_string()),
            "expected exact-boundary string length, got: {text}"
        );

        println!("Testing 64 KiB + 1 payload rejection...");
        let body_json = format!(
            r#"{{"data":"{}"}}"#,
            "a".repeat(LIMIT + 1 - JSON_OVERHEAD)
        );
        let res = client
            .post(format!("{}/api/large_body_test", base_url))
            .header("Content-Type", "application/json")
            .body(body_json)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

        println!("Testing chunked body overflow...");
        let chunked_body = format!(
            r#"{{"data":"{}"}}"#,
            "a".repeat(LIMIT + 1 - JSON_OVERHEAD)
        );
        let status = raw_chunked_body_status(
            port,
            "/api/large_body_test",
            chunked_body.as_bytes(),
        )
        .await?;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

        println!("Testing a slow upload within the configured limit...");
        let slow_body = format!(r#"{{"data":"{}"}}"#, "a".repeat(16 * 1024));
        let status = raw_slow_body_status(
            port,
            "/api/large_body_test",
            slow_body.as_bytes(),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);

        println!("Testing client disconnect cleanup...");
        disconnect_during_upload(port, "/api/large_body_test", LIMIT).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let healthy = tokio::time::timeout(
            Duration::from_secs(5),
            client.get(format!("{}/api/get_test", base_url)).send(),
        )
        .await??;
        assert_eq!(healthy.status(), StatusCode::OK);
    }

    // Preview 2 reports its real internal queue; Preview 3 returns the same
    // zero-shaped probe so the release script has one stable endpoint.
    {
        let res = client
            .get(format!("{}/api/pollable_depth", base_url))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.text().await?.trim(), "0");
    }

    println!("All assertions passed successfully!");
    Ok(())
}

async fn run_e2e_tests(wasm_path: &str, is_p3: bool) {
    let server = start_server(wasm_path, is_p3)
        .expect("Failed to start Wasmtime server");
    run_assertions(server.port, WASMTIME_CAPABILITIES)
        .await
        .expect("Assertions failed");
}

async fn run_middleware_assertions(port: u16) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{port}");

    let anonymous = client
        .get(format!("{base_url}/api/middleware_request_header"))
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::OK);
    assert_middleware_response_headers(anonymous.headers());
    assert_eq!(anonymous.json::<String>().await?, "sanitized");

    let denied = client
        .get(format!("{base_url}/api/middleware_request_header"))
        .bearer_auth("deny")
        .send()
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_middleware_response_headers(denied.headers());

    let unavailable = client
        .get(format!("{base_url}/api/middleware_request_header"))
        .bearer_auth("error")
        .send()
        .await?;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_middleware_response_headers(unavailable.headers());

    let invalid = client
        .get(format!("{base_url}/api/middleware_request_header"))
        .bearer_auth("invalid")
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    assert_middleware_response_headers(invalid.headers());

    let context = client
        .get(format!("{base_url}/api/middleware_request_header"))
        .bearer_auth("allow")
        .header("x-wasi-auth-subject", "attacker")
        .header("x-wasi-auth-context", "attacker")
        .send()
        .await?;
    assert_eq!(context.status(), StatusCode::OK);
    assert_middleware_response_headers(context.headers());
    assert_eq!(context.json::<String>().await?, "sanitized");

    let ssr = client.get(format!("{base_url}/ssr/async")).send().await?;
    assert_eq!(ssr.status(), StatusCode::OK);
    assert_middleware_response_headers(ssr.headers());
    assert!(ssr.text().await?.contains("Async resource resolved"));

    let static_asset = client
        .get(format!("{base_url}/static/app.js"))
        .send()
        .await?;
    assert_eq!(static_asset.status(), StatusCode::OK);
    assert_middleware_response_headers(static_asset.headers());

    let preflight = client
        .request(
            reqwest::Method::OPTIONS,
            format!("{base_url}/api/post_test"),
        )
        .header("origin", "http://127.0.0.1:3110")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "content-type,authorization",
        )
        .send()
        .await?;
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_middleware_response_headers(preflight.headers());

    let started = Instant::now();
    let delayed = client
        .get(format!("{base_url}/static/delayed.stream"))
        .send()
        .await?;
    assert_eq!(delayed.status(), StatusCode::OK);
    assert_middleware_response_headers(delayed.headers());
    let mut body = delayed.bytes_stream();
    let first = tokio::time::timeout(Duration::from_millis(350), body.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("middleware stream ended early"))??;
    assert_eq!(first.as_ref(), b"first-frame\n");
    assert!(started.elapsed() < Duration::from_millis(350));

    let mut remaining = Vec::new();
    while let Some(frame) = body.next().await {
        remaining.extend_from_slice(&frame?);
    }
    assert_eq!(remaining, b"second-frame\n");

    Ok(())
}

fn assert_middleware_response_headers(headers: &reqwest::header::HeaderMap) {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("middleware response should include a request ID");
    assert!(!request_id.is_empty() && request_id.len() <= 128);
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        headers
            .get("referrer-policy")
            .and_then(|value| value.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
}

async fn run_e2e_tests_with_middleware(wasm_path: &str) {
    let server = start_server(wasm_path, true)
        .expect("Failed to start composed Wasmtime middleware server");
    run_assertions(server.port, WASMTIME_CAPABILITIES)
        .await
        .expect("Composed middleware transport assertions failed");
    run_middleware_assertions(server.port)
        .await
        .expect("Middleware assertions failed");
}

async fn run_middleware_transport_profile(wasm_path: &str) {
    let server = start_server(wasm_path, true)
        .expect("Failed to start middleware transport profile");
    run_assertions(server.port, WASMTIME_CAPABILITIES)
        .await
        .expect("Middleware transport profile assertions failed");
}

#[tokio::test]
#[ignore] // Run via ./run_tests.sh (requires wasmtime + pre-built WASM guests)
async fn test_e2e_wasip2() {
    run_e2e_tests("tests/test-app-p2.wasm", false).await;
}

#[tokio::test]
#[ignore] // Run via ./run_tests.sh (requires wasmtime + pre-built WASM guests)
async fn test_e2e_wasip3() {
    run_e2e_tests("tests/test-app-p3.wasm", true).await;
}

#[tokio::test]
#[ignore] // Run via ./scripts/run-middleware-tests.sh (requires Wasmtime + composed guests)
async fn experimental_middleware_wasmtime_wasip3() {
    run_e2e_tests_with_middleware("tests/test-app-p3-middleware.wasm").await;
}

#[tokio::test]
#[ignore] // Diagnostic profile selected through LEPTOS_WASI_MIDDLEWARE_WASM
async fn experimental_middleware_transport_profile_wasip3() {
    let wasm_path = std::env::var("LEPTOS_WASI_MIDDLEWARE_WASM")
        .expect("LEPTOS_WASI_MIDDLEWARE_WASM must select a composed profile");
    run_middleware_transport_profile(&wasm_path).await;
}

// ---------------------------------------------------------------------------
// Spin server support
// ---------------------------------------------------------------------------

struct SpinServer {
    child: Child,
    port: u16,
    stdout_log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for SpinServer {
    fn drop(&mut self) {
        println!("Stopping Spin server on port {}", self.port);
        let _ = self.child.kill();
        let _ = self.child.wait();

        if std::thread::panicking()
            && let Ok(log) = self.stdout_log.lock()
        {
            eprintln!(
                "\n--- Spin Server Log on port {} (test failed) ---",
                self.port
            );
            for line in log.iter() {
                eprintln!("{}", line);
            }
            eprintln!("--------------------------------------------------");
        }
    }
}

fn start_spin_server(manifest_path: &str) -> anyhow::Result<SpinServer> {
    let args = vec!["up", "-f", manifest_path, "--listen", "127.0.0.1:0"];
    println!("Spawning spin {:?}", args);
    let spin = std::env::var_os("SPIN_BIN").unwrap_or_else(|| "spin".into());
    let mut child = Command::new(spin)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("failed to take stdout"))?;
    let mut reader = BufReader::new(stdout);

    let mut port = None;
    let mut line = String::new();
    let stdout_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stdout_log_clone = stdout_log.clone();

    // Parse "Serving http://127.0.0.1:PORT" from stdout
    for _ in 0..100 {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim().to_string();
        if let Ok(mut log) = stdout_log_clone.lock() {
            log.push(trimmed.clone());
        }
        println!("spin output: {}", trimmed);
        // Look for "Serving http://127.0.0.1:PORT"
        let p = trimmed
            .contains("Serving http")
            .then(|| {
                trimmed
                    .split("http://")
                    .nth(1)
                    .and_then(|addr| {
                        addr.trim().trim_end_matches('/').rsplit(':').next()
                    })
                    .and_then(|port_str| port_str.parse::<u16>().ok())
            })
            .flatten();
        if let Some(p) = p {
            port = Some(p);
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(anyhow::anyhow!(
                "spin up exited early with status: {}",
                status
            ));
        }
    }

    let port = port.ok_or_else(|| {
        anyhow::anyhow!("Failed to parse port from spin up output")
    })?;

    // Drain stdout in background
    std::thread::spawn(move || {
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line) {
            if n == 0 {
                break;
            }
            let trimmed = line.trim().to_string();
            if let Ok(mut log) = stdout_log_clone.lock() {
                log.push(trimmed);
            }
            line.clear();
        }
    });

    Ok(SpinServer {
        child,
        port,
        stdout_log,
    })
}

async fn run_spin_e2e_tests(manifest_path: &str) {
    let server =
        start_spin_server(manifest_path).expect("Failed to start Spin server");
    run_assertions(server.port, SPIN_CAPABILITIES)
        .await
        .expect("Assertions failed");
}

async fn run_spin_e2e_tests_with_middleware(manifest_path: &str) {
    let server = start_spin_server(manifest_path)
        .expect("Failed to start Spin middleware server");
    run_middleware_assertions(server.port)
        .await
        .expect("Middleware assertions failed");
}

#[tokio::test]
#[ignore] // Run via ./run_tests.sh (requires spin + pre-built WASM guests)
async fn test_e2e_spin_wasip2() {
    run_spin_e2e_tests("tests/spin.toml").await;
}

#[tokio::test]
#[ignore] // Promotion fixture: stable Spin 4 currently fails the final-WASI canary
async fn test_e2e_spin_wasip3() {
    run_spin_e2e_tests("tests/spin-p3.toml").await;
}

#[tokio::test]
#[ignore] // Promotion fixture: stable Spin 4 currently fails the final-WASI canary
async fn experimental_middleware_spin_wasip3() {
    run_spin_e2e_tests_with_middleware(
        "tests/spin-p3-middleware-composed.toml",
    )
    .await;
}
