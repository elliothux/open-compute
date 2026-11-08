//! P24: one daemon serves operator-authorized exact hostnames behind an external proxy.

use super::*;

const ECHO_WORKER: &str = "export default { async fetch(request) { const h = request.headers; return Response.json({ url: request.url, ip: h.get('cf-connecting-ip'), auth: h.get('authorization'), cookie: h.get('cookie'), forwardedHost: h.get('x-forwarded-host'), forwardedProto: h.get('x-forwarded-proto'), forwarded: h.get('forwarded'), realIp: h.get('x-real-ip') }); } };";

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn authorize_hostnames(config: &Path, hostnames: &[&str]) {
    let mut instance: PlatformConfig =
        toml::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    instance.external_ingress = Some(open_compute_core::ExternalIngressConfig {
        hostnames: hostnames.iter().map(|value| (*value).to_owned()).collect(),
    });
    fs::write(config, toml::to_string(&instance).unwrap()).unwrap();
}

async fn upload_echo(address: SocketAddr, instance_id: &str, token: &str) {
    let boundary = "ocd-p24-echo";
    let metadata = r#"{"main_module":"index.js","compatibility_date":"2026-09-08"}"#;
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n{ECHO_WORKER}\r\n--{boundary}--\r\n"
    );
    let (status, response) = request_http_body_with_headers(
        address,
        "PUT",
        "127.0.0.1",
        token,
        &format!("/client/v4/accounts/{instance_id}/workers/scripts/echo"),
        &body,
        &format!("Content-Type: multipart/form-data; boundary={boundary}\r\n"),
    )
    .await;
    assert_eq!(status, 200, "{response}");
}

async fn set_origin(
    address: SocketAddr,
    instance_id: &str,
    token: &str,
    origin: &str,
) -> (u16, serde_json::Value) {
    let (status, body) = request_http_body(
        address,
        "PUT",
        "127.0.0.1",
        token,
        &format!("/client/v4/accounts/{instance_id}/open-compute/workers/echo/public-origin"),
        &format!(r#"{{"transport":"external_proxy","origin":"{origin}"}}"#),
    )
    .await;
    (status, serde_json::from_str(&body).unwrap_or_default())
}

/// Send one origin-form request to the external listener with extra raw headers.
async fn external(
    address: SocketAddr,
    host: &str,
    path: &str,
    extra_headers: &str,
) -> (u16, serde_json::Value) {
    let (status, body) = request_http_body_with_headers(
        address,
        "GET",
        host,
        "business-token",
        path,
        "",
        extra_headers,
    )
    .await;
    // The raw HTTP/1.1 client keeps chunked framing; the JSON object is one chunk.
    let json = match (body.find('{'), body.rfind('}')) {
        (Some(start), Some(end)) if start < end => &body[start..=end],
        _ => "null",
    };
    (status, serde_json::from_str(json).unwrap_or_default())
}

/// The operator proxy: Caddy with its own local CA terminates TLS and forwards Host unchanged.
struct OperatorProxy {
    child: Child,
    root_certificate: PathBuf,
}

impl OperatorProxy {
    fn start(root: &Path, tls_port: u16, upstream: SocketAddr) -> Self {
        let caddy = std::env::var_os("OPEN_COMPUTE_BUILD_CADDY")
            .map(PathBuf::from)
            .expect("OPEN_COMPUTE_BUILD_CADDY names the pinned Caddy build input");
        let directory = root.join("operator-proxy");
        fs::create_dir_all(&directory).unwrap();
        let caddyfile = directory.join("Caddyfile");
        fs::write(
            &caddyfile,
            format!(
                "{{\n  admin off\n  storage file_system {storage}\n  skip_install_trust\n  local_certs\n  auto_https disable_redirects\n  http_port {http_port}\n  https_port {tls_port}\n}}\napp.example.com:{tls_port} {{\n  tls internal\n  reverse_proxy http://{upstream}\n}}\n",
                storage = directory.join("storage").display(),
                http_port = free_port(),
            ),
        )
        .unwrap();
        let log = fs::File::create(directory.join("caddy.log")).unwrap();
        let child = Command::new(caddy)
            .args(["run", "--adapter", "caddyfile", "--config"])
            .arg(&caddyfile)
            .env_clear()
            .env("HOME", &directory)
            .env("XDG_DATA_HOME", directory.join("data"))
            .env("XDG_CONFIG_HOME", directory.join("config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        Self {
            child,
            root_certificate: directory.join("storage/pki/authorities/local/root.crt"),
        }
    }

    /// Fetch through the proxy, verifying its certificate with the test CA only.
    async fn fetch(&self, tls_port: u16, path: &str) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut last = String::new();
        loop {
            if self.root_certificate.is_file() {
                let output = Command::new("/usr/bin/curl")
                    .env_clear()
                    .args([
                        "--noproxy",
                        "*",
                        "--silent",
                        "--show-error",
                        "--fail-with-body",
                    ])
                    .arg("--cacert")
                    .arg(&self.root_certificate)
                    .arg("--resolve")
                    .arg(format!("app.example.com:{tls_port}:127.0.0.1"))
                    .arg(format!("https://app.example.com:{tls_port}{path}"))
                    .output()
                    .unwrap();
                if output.status.success() {
                    return serde_json::from_slice(&output.stdout).unwrap();
                }
                last = format!(
                    "{} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "operator proxy did not serve HTTPS: {last}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for OperatorProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_proxy_origins_route_exact_hostnames_without_gateway() {
    let root = Evidence::new();
    let _package_root = package_scope::UserRoot::reserve(root.path());
    let binary = isolated_binary(root.path());
    fs::create_dir(root.path().join("home")).unwrap();
    let address = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let ingress = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let tls_port = free_port();
    let registry = package_scope::registry(root.path());
    let ocd_root = registry.root_for(ServiceScope::User).to_path_buf();
    fs::create_dir_all(ocd_root.join("keys")).unwrap();
    let admin_token = ocd_root.join("keys/admin.token");
    fs::write(&admin_token, b"shared-admin-token").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    let manifest = ocd_root.join("ocd.toml");
    fs::write(
        &manifest,
        format!(
            "[server]\npublic_bind = \"{address}\"\nadmin_auth = {{ file = \"./keys/admin.token\" }}\n\n[external_ingress]\nbind = \"{ingress}\"\ntrusted_proxies = [\"127.0.0.1/32\"]\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let (config_a, data_a) = initialized_local_instance(root.path(), "alpha");
    let (config_b, data_b) = initialized_local_instance(root.path(), "beta");
    authorize_hostnames(&config_a, &["example.com", "app.example.com"]);
    authorize_hostnames(&config_b, &["app.example.com"]);
    let a = registry
        .register(&config_a, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert!(
        registry
            .register(&config_b, ServiceScope::User, SystemTime::now())
            .is_err(),
        "one hostname cannot be authorized for two instances"
    );
    authorize_hostnames(&config_b, &["beta.example.net"]);
    let b = registry
        .register(&config_b, ServiceScope::User, SystemTime::now())
        .unwrap();

    let log = root.path().join("external-ingress-stderr.log");
    let mut process = Process::spawn(&binary, &data_a, &log).with_instance(&data_b);
    process.ready(address).await;
    assert!(
        !ocd_root.join("gateway").exists(),
        "external mode needs no Gateway"
    );
    upload_echo(address, &a.instance_id, "alpha-deployer").await;
    upload_echo(address, &b.instance_id, "beta-deployer").await;

    let app_origin = format!("https://app.example.com:{tls_port}");
    let (status, body) = set_origin(address, &a.instance_id, "alpha-deployer", &app_origin).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["result"]["origin"], app_origin);
    assert_eq!(body["result"]["ingress_available"], true);
    let (status, _) = set_origin(address, &a.instance_id, "alpha-read-only", &app_origin).await;
    assert_eq!(status, 403);
    let (status, _) = set_origin(
        address,
        &b.instance_id,
        "beta-deployer",
        "https://example.com",
    )
    .await;
    assert_eq!(status, 403, "beta has no authorization for example.com");
    let (status, body) = set_origin(
        address,
        &b.instance_id,
        "beta-deployer",
        "http://beta.example.net",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, endpoints) = request_http(
        address,
        "GET",
        "127.0.0.1",
        "alpha-read-only",
        &format!(
            "/client/v4/accounts/{}/open-compute/workers/echo/endpoints",
            a.instance_id
        ),
    )
    .await;
    assert_eq!(status, 200, "{endpoints}");
    assert!(
        endpoints.contains(&format!("\"url\":\"{app_origin}/\"")),
        "{endpoints}"
    );

    let app_host = format!("app.example.com:{tls_port}");
    let (status, echo) = external(
        ingress,
        &app_host,
        "/a%2Fb?x=%2F",
        "Cookie: session=1\r\nX-Forwarded-Host: evil.example\r\nX-Forwarded-Proto: http\r\nForwarded: for=203.0.113.9;host=evil.example\r\nX-Real-IP: 203.0.113.9\r\nCF-Connecting-IP: 203.0.113.9\r\n",
    )
    .await;
    assert_eq!(status, 200, "{echo}");
    assert_eq!(echo["url"], format!("{app_origin}/a%2Fb?x=%2F"));
    assert_eq!(echo["ip"], "127.0.0.1");
    assert_eq!(echo["auth"], "Bearer business-token");
    assert_eq!(echo["cookie"], "session=1");
    for header in ["forwardedHost", "forwardedProto", "forwarded", "realIp"] {
        assert!(
            echo[header].is_null(),
            "{header} reached the Worker: {echo}"
        );
    }
    for path in [
        "/health/live",
        "/metrics",
        "/client/v4/accounts",
        "/operator/session/exchange",
        "/__open_compute_gateway_probe__",
    ] {
        let (status, echo) = external(ingress, &app_host, path, "").await;
        assert_eq!(status, 200, "{path}: {echo}");
        assert_eq!(echo["url"], format!("{app_origin}{path}"));
    }
    for host in [
        "app.example.com".to_owned(),
        format!("app.example.com:{}", ingress.port()),
        "example.com".to_owned(),
        "unknown.example.org".to_owned(),
    ] {
        let (status, _) = external(ingress, &host, "/", "").await;
        assert_eq!(status, 404, "{host}");
    }
    let (status, echo) = external(ingress, "beta.example.net", "/", "").await;
    assert_eq!(status, 200, "{echo}");
    assert_eq!(echo["url"], "http://beta.example.net/");

    let proxy = OperatorProxy::start(root.path(), tls_port, ingress);
    let echo = proxy.fetch(tls_port, "/orders?id=1").await;
    assert_eq!(echo["url"], format!("{app_origin}/orders?id=1"));
    assert_eq!(
        echo["ip"], "127.0.0.1",
        "external mode reports the proxy peer"
    );
    for header in ["forwardedHost", "forwardedProto", "forwarded"] {
        assert!(
            echo[header].is_null(),
            "{header} reached the Worker: {echo}"
        );
    }
    drop(proxy);

    successful(&binary, &["instance", "stop", "beta"]);
    wait_for_stopped_instance(address, &b.instance_id, &log).await;
    let (status, _) = external(ingress, "beta.example.net", "/", "").await;
    assert_eq!(
        status, 503,
        "a stopped owner is unavailable, never rerouted"
    );
    successful(&binary, &["instance", "start", "beta"]);

    let (status, _) = request_http(
        address,
        "DELETE",
        "127.0.0.1",
        "alpha-deployer",
        &format!(
            "/client/v4/accounts/{}/open-compute/workers/echo/public-origin",
            a.instance_id
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = external(ingress, &app_host, "/", "").await;
    assert_eq!(status, 404, "a revoked binding no longer routes");
    let (status, body) = set_origin(
        address,
        &a.instance_id,
        "alpha-deployer",
        "https://example.com",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, echo) = external(ingress, "example.com:443", "/", "").await;
    assert_eq!(status, 200, "{echo}");
    assert_eq!(echo["url"], "https://example.com/");
    process.ready(address).await;
    process.stop().await;

    authorize_hostnames(&config_a, &["app.example.com"]);
    assert!(
        registry.list_scope(ServiceScope::User).is_err(),
        "an authorization with an active binding cannot be removed"
    );
    authorize_hostnames(&config_a, &["example.com", "app.example.com"]);
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
}
