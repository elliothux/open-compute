//! Fresh-process Queue commit-before-response crash evidence.

use open_compute_core::{AccountId, QueueId};
use open_compute_storage::{
    QueueConfig, QueueContentType, QueueEnqueueRequest, QueueMessageInput, QueueProjection,
    SchedulerStore,
};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn p2_2_queue_commit_child() {
    let Some(database) = std::env::var_os("OPEN_COMPUTE_P2_2_COMMIT_DB") else {
        return;
    };
    let queue: QueueId = std::env::var("OPEN_COMPUTE_P2_2_COMMIT_QUEUE")
        .unwrap()
        .parse()
        .unwrap();
    let marker = PathBuf::from(std::env::var_os("OPEN_COMPUTE_P2_2_COMMIT_MARKER").unwrap());
    let store = SchedulerStore::open(Path::new(&database), 1_000, 1).unwrap();
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"commit-before-response".to_vec(),
                    delay_seconds: Some(0),
                }],
            },
            10,
        )
        .unwrap();
    let mut marker_file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(marker)
        .unwrap();
    marker_file.write_all(b"committed").unwrap();
    marker_file.sync_all().unwrap();
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}

struct CommitChild(Child);

impl Drop for CommitChild {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn p2_2_sigkill_after_commit_preserves_message_and_counters() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("scheduler.sqlite");
    let marker = temp.path().join("committed");
    let queue = QueueId::generate();
    let account = AccountId::generate();
    let store = SchedulerStore::open(&database, 1_000, 1).unwrap();
    store
        .create_queue_projection(&QueueProjection {
            queue_id: queue,
            account_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config: QueueConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    drop(store);
    let mut child = CommitChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commit_crash::p2_2_queue_commit_child",
                "--nocapture",
            ])
            .env("OPEN_COMPUTE_P2_2_COMMIT_DB", &database)
            .env("OPEN_COMPUTE_P2_2_COMMIT_QUEUE", queue.to_string())
            .env("OPEN_COMPUTE_P2_2_COMMIT_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.is_file() {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline, "commit child marker timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = Command::new("/bin/kill")
        .args(["-KILL", &child.0.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!child.0.wait().unwrap().success());
    let reopened = SchedulerStore::open(&database, 1_000, 100).unwrap();
    let metrics = reopened.queue_metrics(queue, 1, 1).unwrap();
    assert_eq!(metrics.backlog_count, 1);
    assert_eq!(metrics.backlog_bytes, 22);
    assert_eq!(metrics.oldest_message_timestamp_ms, Some(10));
    assert!(reopened.queue_counter_mismatches().unwrap().is_empty());
}
