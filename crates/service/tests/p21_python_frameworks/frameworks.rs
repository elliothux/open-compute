//! Ordinary Version uploads using reviewed framework packages and unmodified SDK bytes.

use super::python_support::PYTHON_SECRETS;
use super::python_support::capture::Capture;
use super::python_support::fixture::{Fixture, RequestTarget};
use serde_json::{Value, json};
use std::fs;

#[derive(Clone, Copy)]
enum Framework {
    Django,
    Flask,
    FastApi,
}

impl Framework {
    fn name(self) -> &'static str {
        match self {
            Self::Django => "django",
            Self::Flask => "flask",
            Self::FastApi => "fastapi",
        }
    }

    fn package_files(self) -> &'static [&'static str] {
        match self {
            Self::Django => &["app_urls.py"],
            Self::Flask => &["templates/page.html"],
            Self::FastApi => &[],
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_django_framework_http_smoke() {
    qualify(Framework::Django).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_flask_framework_http_smoke() {
    qualify(Framework::Flask).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_fastapi_framework_http_smoke() {
    qualify(Framework::FastApi).await;
}

async fn qualify(framework: Framework) {
    let name = framework.name();
    let script = format!("python-{name}-fixture");
    let capture = Capture::load(
        &format!("test/fixtures/python-frameworks/{name}"),
        &format!("test/applications/python-frameworks/{name}/src"),
        framework.package_files(),
    )
    .await;
    let mut fixture = Fixture::new(None).await;
    let first = upload(&fixture, &capture, &script, "first", PYTHON_SECRETS[0]).await;
    let first_record = fixture.record(&script, &first);
    fixture.promote(&script, &first).await;
    assert_eq!(fixture.active(&script), first_record.version_id.to_string());
    verify_application(&fixture, framework, &script, "first").await;

    fixture.process.stop().await;
    for lease in [
        "child.lease",
        "python-prepare.lease",
        "python-compile.lease",
    ] {
        assert!(!fixture.data.join("runtime").join(lease).exists());
    }
    for address in [fixture.public, fixture.admin] {
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    }
    let log = fs::read(&fixture.log).unwrap();
    for secret in PYTHON_SECRETS {
        assert!(
            !log.windows(secret.len())
                .any(|part| part == secret.as_bytes())
        );
    }
    println!(
        "python-framework-evidence: {}",
        json!({
            "framework":name,"uploadSha256":capture.sha256,
            "version":first,
            "firstPreparedIdentitySha256":hex::encode(first_record.prepared_identity_sha256),
            "activeVersion":fixture.active(&script),"httpAdapter":name
        })
    );
}

async fn upload(
    fixture: &Fixture,
    capture: &Capture,
    script: &str,
    revision: &str,
    secret: &str,
) -> String {
    let (status, _, bytes, version) = fixture
        .upload_version(
            script,
            &Capture::content_type(),
            capture.render(
                &[
                    json!({"name":"REVISION","type":"plain_text","text":revision}),
                    json!({"name":"TOKEN","type":"secret_text","text":secret}),
                ],
                None,
            ),
        )
        .await;
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "framework Version upload/prepare must succeed");
    assert_eq!(value["success"], true);
    version.unwrap().to_string()
}

async fn verify_application(fixture: &Fixture, framework: Framework, script: &str, revision: &str) {
    if matches!(framework, Framework::FastApi) {
        verify_asgi(fixture, script, revision).await;
    } else {
        verify_wsgi(fixture, framework, script, revision).await;
    }
    let (status, _, _) = fixture
        .request(
            "/missing",
            "GET",
            "text/plain",
            Vec::new(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _, body) = fixture
        .request(
            "/fail",
            "GET",
            "text/plain",
            Vec::new(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 500);
    assert!(!String::from_utf8_lossy(&body).contains("Traceback"));
}

async fn verify_wsgi(fixture: &Fixture, framework: Framework, script: &str, revision: &str) {
    let (status, headers, body) = fixture
        .request(
            "/echo?v=one&v=two",
            "POST",
            "text/plain",
            b"hello-body".to_vec(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(headers["x-app"], framework.name());
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"method":"POST","body":"hello-body","query":["one","two"],"revision":revision})
    );
    // Flask context-bearing streams are explicitly deferred: workers-py#287.
    // Its ordinary HTTP/template adapter remains in scope.
    if matches!(framework, Framework::Django) {
        let initial = fixture.invoke(script, "/state").await["closed"]
            .as_u64()
            .unwrap();
        let (status, _, body) = fixture
            .request(
                "/stream?value=stream-body",
                "GET",
                "text/plain",
                Vec::new(),
                RequestTarget::Worker(script),
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"django:stream-body");
        assert_eq!(
            fixture.invoke(script, "/state").await["closed"],
            initial + 1
        );
    }
    let (status, _, body) = fixture
        .request(
            "/echo",
            "HEAD",
            "text/plain",
            Vec::new(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 201);
    assert!(body.is_empty());
    if matches!(framework, Framework::Flask) {
        let (status, _, body) = fixture
            .request(
                "/template?value=%3Ctag%3E%26",
                "GET",
                "text/plain",
                Vec::new(),
                RequestTarget::Worker(script),
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"<p>&lt;tag&gt;&amp;</p>");
    }
}

async fn verify_asgi(fixture: &Fixture, script: &str, revision: &str) {
    let (status, _, body) = fixture
        .request(
            "/echo",
            "POST",
            "application/json",
            br#"{"count":"3","label":"native-wasm"}"#.to_vec(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(
        status,
        201,
        "FastAPI POST /echo ({revision}) failed: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "count":3,"label":"native-wasm","revision":revision
        })
    );
    let (status, _, body) = fixture
        .request(
            "/echo",
            "POST",
            "application/json",
            br#"{"count":0,"label":42}"#.to_vec(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 422);
    let body: Value = serde_json::from_slice(&body).unwrap();
    let errors = body["detail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(errors, ["greater_than_equal", "string_type"]);
    assert_eq!(fixture.invoke(script, "/sync").await, json!({"sync":true}));
    let (status, _, body) = fixture
        .request(
            "/stream?value=stream-body",
            "GET",
            "text/plain",
            Vec::new(),
            RequestTarget::Worker(script),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, b"fastapi:stream-body");
    assert_eq!(
        fixture.invoke(script, "/openapi.json").await["components"]["schemas"]["Input"]["properties"]
            ["count"]["minimum"].as_f64(),
        Some(1.0)
    );
}
