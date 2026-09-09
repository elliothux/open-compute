//! In-process HTTP S3-compatible test server.

#![allow(
    missing_docs,
    reason = "test-only mock fields are intentionally direct fixture controls"
)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    None,
    Auth,
    Permission,
    ServerError,
    Timeout,
    CorruptMetadata,
    CorruptBody,
    DeleteFail,
    PutResponseLoss,
    DeleteResponseLoss,
    CreateResponseLoss,
    CompleteResponseLoss,
    AbortResponseLoss,
    MidstreamReset,
    NotFound,
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    #[allow(
        dead_code,
        reason = "shared test support is consumed by a subset of integration targets"
    )]
    pub query: String,
    pub has_authorization: bool,
    pub authorization: Option<String>,
    /// Present when the request carried SSE-C headers. Never stores the key.
    pub ssec_algorithm: Option<String>,
    /// Public SSE-C key MD5 header, if any.
    pub ssec_key_md5: Option<String>,
    /// Physical S3 storage class requested by the adapter, if any.
    pub storage_class: Option<String>,
}

#[derive(Clone)]
pub(crate) struct StoredObject {
    pub body: Vec<u8>,
    pub sha256: String,
    pub etag: String,
    pub metadata: HashMap<String, String>,
    pub response_headers: HashMap<String, String>,
    #[allow(
        dead_code,
        reason = "shared test support is consumed by a subset of integration targets"
    )]
    pub modified: SystemTime,
    pub storage_class: String,
    pub ssec_key_md5: Option<String>,
}

struct MultipartUpload {
    key: String,
    parts: std::collections::BTreeMap<i32, (String, Vec<u8>)>,
    metadata: HashMap<String, String>,
    response_headers: HashMap<String, String>,
    storage_class: String,
    ssec_key_md5: Option<String>,
}

#[derive(Clone)]
pub struct MockS3 {
    pub endpoint: String,
    state: Arc<Mutex<Inner>>,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl std::fmt::Debug for MockS3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockS3")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

struct Inner {
    bucket: String,
    objects: HashMap<String, StoredObject>,
    uploads: HashMap<String, MultipartUpload>,
    fault: Fault,
    recorded: Vec<Recorded>,
    get_chunk_size: usize,
    get_chunk_delay: Duration,
    omit_last_modified: bool,
    head_barrier: Option<Arc<tokio::sync::Barrier>>,
    conditional_put_race: Option<Vec<u8>>,
}

impl MockS3 {
    pub async fn spawn(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(Inner {
            bucket: bucket.to_string(),
            objects: HashMap::new(),
            uploads: HashMap::new(),
            fault: Fault::None,
            recorded: Vec::new(),
            get_chunk_size: usize::MAX,
            get_chunk_delay: Duration::ZERO,
            omit_last_modified: false,
            head_barrier: None,
            conditional_put_race: None,
        }));
        let (tx, mut rx) = oneshot::channel();
        let state_clone = Arc::clone(&state);
        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    acc = listener.accept() => {
                        match acc {
                            Ok((stream, _)) => {
                                let state = Arc::clone(&state_clone);
                                tokio::spawn(async move {
                                    let _ = handle_conn(stream, state).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
        Self {
            endpoint: format!("http://{addr}"),
            state,
            shutdown: Arc::new(Mutex::new(Some(tx))),
            join: Arc::new(Mutex::new(Some(join))),
        }
    }

    pub fn set_fault(&self, fault: Fault) {
        self.state.lock().expect("lock").fault = fault;
    }

    pub fn synchronize_next_heads(&self, participants: usize) {
        self.state.lock().expect("lock").head_barrier =
            Some(Arc::new(tokio::sync::Barrier::new(participants)));
    }

    /// Insert one competing object immediately before the next conditional create fence.
    pub fn race_next_conditional_put(&self, body: Vec<u8>) {
        self.state.lock().expect("lock").conditional_put_race = Some(body);
    }

    /// Number of provider multipart uploads that have not completed or aborted.
    pub fn multipart_upload_count(&self) -> usize {
        self.state.lock().expect("lock").uploads.len()
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.state.lock().expect("lock").recorded.clone()
    }

    /// Drop recorded requests so the next observer sees only subsequent traffic.
    pub fn clear_recorded(&self) {
        self.state.lock().expect("lock").recorded.clear();
    }

    /// Snapshot of object keys currently stored.
    pub fn keys(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("lock")
            .objects
            .keys()
            .cloned()
            .collect()
    }

    pub fn object_count(&self) -> usize {
        self.state.lock().expect("lock").objects.len()
    }

    pub fn put_raw(&self, key: &str, body: Vec<u8>) {
        let sha256 = hex::encode(Sha256::digest(&body));
        self.state.lock().expect("lock").objects.insert(
            key.to_string(),
            StoredObject {
                etag: hex::encode(md5::Md5::digest(&body)),
                body,
                sha256,
                metadata: HashMap::new(),
                response_headers: HashMap::new(),
                modified: SystemTime::now(),
                storage_class: "STANDARD".to_owned(),
                ssec_key_md5: None,
            },
        );
    }

    pub fn corrupt_body(&self, key: &str) {
        if let Some(obj) = self.state.lock().expect("lock").objects.get_mut(key) {
            obj.body.push(0xff);
        }
    }

    pub fn set_get_chunking(&self, chunk_size: usize, delay: Duration) {
        let mut g = self.state.lock().expect("lock");
        g.get_chunk_size = chunk_size.max(1);
        g.get_chunk_delay = delay;
    }

    pub fn set_omit_last_modified(&self, omit: bool) {
        self.state.lock().expect("lock").omit_last_modified = omit;
    }

    pub fn artifact_gets(&self) -> usize {
        self.recorded()
            .iter()
            .filter(|r| r.method == "GET" && r.path.contains("/artifacts/v1/sha256/"))
            .count()
    }
}

impl Drop for MockS3 {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.lock().expect("lock").take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.lock().expect("lock").take() {
            join.abort();
        }
    }
}

mod handler;
mod http;

use handler::handle_conn;
use http::*;

#[cfg(test)]
mod tests;
