//! Real daemon, public v4 API, read-only authority observations and encrypted objects.

use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request};
use open_compute_artifacts::{
    ArtifactRef, ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::{AiConfig, DataConfig, PlatformConfig, Redactor, SystemClock};
use open_compute_runtime::verify_runtime_binary;
use open_compute_storage::PlatformStorage;
use open_compute_storage::worker_repository::PythonPreparedArtifactRecord;
use open_compute_workers::python_artifact::{PreparedPythonIdentity, restore_prepared_python};
use serde_json::{Value, json};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) enum RequestTarget<'a> {
    Admin,
    ReadOnly,
    Unauthenticated,
    AssetUpload(&'a str),
    Worker(&'a str),
}

pub(crate) struct Fixture {
    pub(crate) process: platform_process::Process,
    pub(crate) mock: MockS3,
    pub(crate) root: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) config: platform_process::ProcessConfig,
    pub(crate) log: PathBuf,
    pub(crate) public: SocketAddr,
    pub(crate) admin: SocketAddr,
    pub(crate) internal_account: String,
    pub(crate) public_account: String,
    pub(crate) client: platform_process::Client,
    _evidence: platform_process::Evidence,
}

impl Fixture {
    pub(crate) async fn new(ai: Option<AiConfig>) -> Self {
        let workerd = PathBuf::from(
            std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
                .expect("OPEN_COMPUTE_TEST_WORKERD must select the formally verified runtime"),
        );
        verify_runtime_binary(
            &repo_root().join("packages/runtime/workerd.lock.json"),
            &workerd,
            Duration::from_secs(10),
            &Redactor::new(),
        )
        .await
        .unwrap();
        let runs = repo_root().join(".temp/p21-python-main-run");
        fs::create_dir_all(&runs).unwrap();
        let evidence = platform_process::Evidence(Some(
            tempfile::Builder::new()
                .prefix("main-")
                .tempdir_in(&runs)
                .unwrap(),
        ));
        let root = evidence.0.as_ref().unwrap().path().to_owned();
        let data = root.join("data");
        let storage = PlatformStorage::bootstrap(&storage_config(&data), &SystemClock).unwrap();
        let internal_account = storage.identity().instance_id.to_string();
        drop(storage);
        let mock = MockS3::spawn("open-compute").await;
        let (public, admin) = platform_process::distinct_addresses();
        let config = platform_process::config(&root, &data, &mock.endpoint, public, admin);
        if let Some(ai) = ai {
            let mut parsed =
                PlatformConfig::from_toml_str(&fs::read_to_string(&config).unwrap()).unwrap();
            parsed.ai = ai;
            let source = toml::to_string(&parsed).unwrap();
            PlatformConfig::from_toml_str(&source).unwrap();
            fs::write(&config, source).unwrap();
        }
        let log = root.join("ocd.log");
        let mut process = platform_process::spawn(&config, &log);
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http();
        platform_process::ready(&client, admin, &mut process).await;
        let mut fixture = Self {
            process,
            mock,
            root,
            data,
            config,
            log,
            public,
            admin,
            internal_account,
            public_account: String::new(),
            client,
            _evidence: evidence,
        };
        let (status, _, bytes) = fixture
            .request(
                "/client/v4/accounts",
                "GET",
                "application/json",
                Vec::new(),
                RequestTarget::Admin,
            )
            .await;
        assert_eq!(status, 200);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let accounts = value["result"].as_array().unwrap();
        assert_eq!(accounts.len(), 1);
        fixture.public_account = accounts[0]["id"].as_str().unwrap().to_owned();
        fixture
    }

    pub(crate) async fn request(
        &self,
        path: &str,
        method: &str,
        content_type: &str,
        bytes: Vec<u8>,
        target: RequestTarget<'_>,
    ) -> (u16, HeaderMap, Vec<u8>) {
        let address = match target {
            RequestTarget::Admin
            | RequestTarget::ReadOnly
            | RequestTarget::Unauthenticated
            | RequestTarget::AssetUpload(_) => self.admin,
            RequestTarget::Worker(_) => self.public,
        };
        let mut request = Request::builder()
            .method(method)
            .uri(format!("http://{address}{path}"));
        if !content_type.is_empty() {
            request = request.header("content-type", content_type);
        }
        request = match target {
            RequestTarget::Admin => request.header("authorization", format!("Bearer {TOKEN}")),
            RequestTarget::ReadOnly => {
                request.header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
            }
            RequestTarget::Unauthenticated => request,
            RequestTarget::AssetUpload(token) => {
                request.header("authorization", format!("Bearer {token}"))
            }
            RequestTarget::Worker(script) => request.header(
                "host",
                format!("{script}.{}.localhost", self.internal_account),
            ),
        };
        let response = tokio::time::timeout(
            Duration::from_secs(90),
            self.client
                .request(request.body(Body::from(bytes)).unwrap()),
        )
        .await
        .expect("bounded daemon request timed out")
        .expect("daemon request failed");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = tokio::time::timeout(
            Duration::from_secs(30),
            to_bytes(Body::new(response.into_body()), 8 * 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap()
        .to_vec();
        for secret in PYTHON_SECRETS.into_iter().chain([TOKEN, READ_ONLY_TOKEN]) {
            assert!(
                headers.values().all(|value| !value
                    .as_bytes()
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes())),
                "response header exposed a secret or authentication credential"
            );
            assert!(
                !body
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes()),
                "response exposed a secret or authentication credential"
            );
        }
        (status, headers, body)
    }

    pub(crate) async fn api(&self, suffix: &str, method: &str, value: Option<Value>) -> Value {
        let (status, _, bytes) = self
            .request(
                &format!("/client/v4/accounts/{}{suffix}", self.public_account),
                method,
                if value.is_some() {
                    "application/json"
                } else {
                    ""
                },
                value.map_or_else(Vec::new, |v| serde_json::to_vec(&v).unwrap()),
                RequestTarget::Admin,
            )
            .await;
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            status, 200,
            "v4 operation {method} {suffix} failed: {}",
            value["errors"]
        );
        assert_eq!(value["success"], true);
        value["result"].clone()
    }

    pub(crate) async fn upload_version(
        &self,
        script: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> (u16, HeaderMap, Vec<u8>) {
        // The public API creates a new script with PUT. Version POST requires
        // an existing script and leaves its active deployment unchanged.
        let exists: bool = self
            .connection()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workers WHERE name=?1 AND deleted_at_ms IS NULL)",
                [script],
                |row| row.get(0),
            )
            .unwrap();
        let (suffix, method) = if exists {
            ("/versions", "POST")
        } else {
            ("", "PUT")
        };
        self.request(
            &format!(
                "/client/v4/accounts/{}/workers/scripts/{script}{suffix}?bindings_inherit=strict",
                self.public_account
            ),
            method,
            content_type,
            bytes,
            RequestTarget::Admin,
        )
        .await
    }

    pub(crate) async fn invoke(&self, script: &str, path: &str) -> Value {
        let (status, _, bytes) = self
            .request(
                path,
                "GET",
                "application/json",
                Vec::new(),
                RequestTarget::Worker(script),
            )
            .await;
        let filesystem = (status != 200).then(|| {
            rustix::fs::statvfs(&self.data).map(|stat| (stat.f_blocks, stat.f_bfree, stat.f_bavail))
        });
        let failure = (status != 200).then(|| {
            let response = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
            json!({
                "code": response["error"]["code"],
                "outcome": response["error"]["outcome"],
                "cloudflareCode": response["error"]["cloudflareCode"],
            })
        });
        assert_eq!(
            status, 200,
            "Worker {script} invocation {path} failed; error={failure:?}; filesystem (blocks, free, available)={filesystem:?}"
        );
        serde_json::from_slice(&bytes).unwrap()
    }

    pub(crate) async fn upload_javascript(
        &self,
        script: &str,
        source: &str,
        bindings: &[Value],
        exports: Option<Value>,
        assets: Option<Value>,
    ) -> String {
        // A test-owned JavaScript protocol fixture must never substitute for
        // the reviewed official Python capture or its unchanged SDK modules.
        let mut metadata = json!({
            "main_module":"index.js", "compatibility_date":"2026-09-08",
            "compatibility_flags":[], "bindings":bindings,
        });
        if let Some(exports) = exports {
            metadata["exports"] = exports;
        }
        if let Some(assets) = assets {
            metadata["assets"] = assets;
        }
        let boundary = "python-javascript-protocol-fixture";
        assert!(!source.contains(boundary));
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n{source}\r\n\
             --{boundary}--\r\n"
        );
        let (status, _, bytes) = self
            .upload_version(
                script,
                &format!("multipart/form-data; boundary={boundary}"),
                body.into_bytes(),
            )
            .await;
        assert_eq!(status, 200, "JavaScript Version admission failed");
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["success"], true);
        let version = response["result"]["id"].as_str().unwrap().to_owned();
        let id = uuid::Uuid::parse_str(&version).unwrap().to_string();
        let (state, prepared): (String, i64) = self.connection().query_row(
            "SELECT v.state, (SELECT count(*) FROM version_python_prepared p WHERE p.version_id=v.id)
             FROM worker_versions v JOIN workers w ON w.id=v.worker_id WHERE v.id=?1 AND w.name=?2",
            rusqlite::params![id, script], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(state, "ready");
        assert_eq!(prepared, 0, "JavaScript must not own a Python snapshot");
        version
    }

    pub(crate) async fn promote(&self, script: &str, version: &str) {
        self.api(&format!("/workers/scripts/{script}/deployments"), "POST",
            Some(json!({"strategy":"percentage", "versions":[{"version_id":version, "percentage":100}]}))).await;
    }

    pub(crate) async fn restart(&mut self) {
        self.process.stop().await;
        self.process.restart(&self.config, &self.log);
        platform_process::ready(&self.client, self.admin, &mut self.process).await;
    }

    pub(crate) fn record(&self, script: &str, version: &str) -> PythonPreparedArtifactRecord {
        let version = uuid::Uuid::parse_str(version).unwrap().to_string();
        let connection = self.connection();
        let (state, identity, metadata, digest, size, created): (String, Vec<u8>, Vec<u8>, Vec<u8>, u64, i64) = connection.query_row(
            "SELECT v.state,p.prepared_identity_sha256,p.identity_json,p.artifact_sha256,p.artifact_size,p.created_at_ms
             FROM version_python_prepared p JOIN worker_versions v ON v.id=p.version_id JOIN workers w ON w.id=v.worker_id
             WHERE v.id=?1 AND w.name=?2", rusqlite::params![version, script],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        ).expect("ready Python Version must own a prepared record");
        assert_eq!(state, "ready");
        let record = PythonPreparedArtifactRecord {
            version_id: version.parse().unwrap(),
            prepared_identity_sha256: identity.try_into().unwrap(),
            identity_json: metadata,
            artifact_sha256: digest.try_into().unwrap(),
            artifact_size: size,
            created_at_ms: created,
        };
        let identity: PreparedPythonIdentity =
            serde_json::from_slice(&record.identity_json).unwrap();
        assert_eq!(identity.sha256().unwrap(), record.prepared_identity_sha256);
        record
    }

    pub(crate) fn active(&self, script: &str) -> String {
        self.connection().query_row("SELECT d.version_id FROM workers w JOIN worker_deployments d ON d.id=w.active_deployment_id WHERE w.name=?1",
            [script], |row| row.get(0)).unwrap()
    }

    pub(crate) fn connection(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.data.join("control.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    pub(crate) fn scheduler_connection(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.data.join("scheduler.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    pub(crate) async fn ciphertext(&mut self, record: &PythonPreparedArtifactRecord) -> Vec<u8> {
        assert!(
            self.process.0.try_wait().is_ok_and(|state| state.is_some()),
            "decrypt only after releasing daemon data lock"
        );
        let storage =
            PlatformStorage::bootstrap(&storage_config(&self.data), &SystemClock).unwrap();
        let config =
            PlatformConfig::from_toml_str(&fs::read_to_string(&self.config).unwrap()).unwrap();
        let s3 = config.object_storage.as_s3().unwrap();
        let credentials = resolve_s3_credentials_with(s3, &MapEnv::new()).unwrap();
        let artifacts = ArtifactStore::new(
            ObjectBackend::connect_s3(s3, &credentials, 128 * 1024 * 1024 + 65_564).unwrap(),
        );
        let identity: PreparedPythonIdentity =
            serde_json::from_slice(&record.identity_json).unwrap();
        let snapshot = restore_prepared_python(record, &identity, &artifacts, storage.crypto())
            .await
            .unwrap();
        assert!(snapshot.expose().len() >= 16);
        drop(snapshot);
        let reference = ArtifactRef::new(
            1,
            &hex::encode(record.artifact_sha256),
            record.artifact_size,
        )
        .unwrap();
        let bytes = artifacts.open(&reference).await.unwrap();
        for secret in PYTHON_SECRETS {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes())
            );
        }
        bytes.to_vec()
    }
}

fn storage_config(root: &std::path::Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}
