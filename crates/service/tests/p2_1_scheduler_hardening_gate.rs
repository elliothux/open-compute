//! P2.1 scheduler kernel, persistence, and fresh-process crash Gate.

#![cfg(feature = "test-support")]

use open_compute_core::{
    DeploymentId, DurableObjectId, ResourceId, SchedulerFaultPoint, SchedulerKind,
};
use open_compute_storage::{
    AlarmProjection, ClaimResult, ClaimedJob, SchedulerStore, scheduler_migration_registry,
};
use rusqlite::Connection;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn object(namespace: ResourceId, byte: u8) -> DurableObjectId {
    let mut bytes = [byte; open_compute_core::DURABLE_OBJECT_ID_BYTES];
    bytes[..open_compute_core::DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES].copy_from_slice(
        &open_compute_core::durable_object_namespace_prefix(namespace),
    );
    DurableObjectId::for_namespace(bytes, namespace).unwrap()
}

fn projection(namespace: ResourceId, token: &str) -> AlarmProjection {
    AlarmProjection {
        namespace_resource_id: namespace,
        object_id: object(namespace, 7),
        object_generation: 1,
        row_token: token.to_owned(),
        due_at_ms: 10,
        target_deployment_id: DeploymentId::generate(),
        execution_generation: 1,
        retry_count: 0,
    }
}

fn open(path: &Path, now_ms: i64) -> SchedulerStore {
    SchedulerStore::open(path, 1_000, now_ms).unwrap()
}

fn mark(path: &Path) {
    let mut marker = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .unwrap();
    marker.write_all(b"ready").unwrap();
    marker.sync_all().unwrap();
}

fn park_forever() -> ! {
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}

#[test]
fn p2_1_fault_child() {
    let Some(case) = std::env::var_os("OPEN_COMPUTE_P2_1_FAULT_CASE") else {
        return;
    };
    let database = PathBuf::from(std::env::var_os("OPEN_COMPUTE_P2_1_DB").unwrap());
    let marker = PathBuf::from(std::env::var_os("OPEN_COMPUTE_P2_1_MARKER").unwrap());
    let claim_path = PathBuf::from(std::env::var_os("OPEN_COMPUTE_P2_1_CLAIM").unwrap());
    let case = case.to_string_lossy();
    let store = open(&database, 10);
    if case == format!("{:?}", SchedulerFaultPoint::DuringProjectionRefresh) {
        let connection = Connection::open(&database).unwrap();
        let namespace: String = connection
            .query_row(
                "SELECT namespace_resource_id FROM scheduled_jobs LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);
        let namespace = namespace.parse().unwrap();
        store
            .upsert_alarm(&projection(namespace, "refreshed-token-01"), 11)
            .unwrap();
        mark(&marker);
        park_forever();
    }
    let [claim] = store.claim_due(10, 50, 1).unwrap().try_into().unwrap();
    fs::write(&claim_path, serde_json::to_vec(&claim).unwrap()).unwrap();
    if case == format!("{:?}", SchedulerFaultPoint::AfterCompleteCommit) {
        assert!(store.finish_claim(&claim, ClaimResult::Delete, 11).unwrap());
    }
    mark(&marker);
    park_forever();
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wait_marker(marker: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.is_file() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fault child exited before marker"
        );
        assert!(Instant::now() < deadline, "fault child marker timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn kill(child: &Child) {
    let status = Command::new("/bin/kill")
        .args(["-KILL", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn run_fault_case(point: SchedulerFaultPoint) {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("scheduler.sqlite");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&database)
        .unwrap();
    let namespace = ResourceId::generate();
    let store = open(&database, 1);
    store
        .upsert_alarm(&projection(namespace, "original-token-01"), 1)
        .unwrap();
    drop(store);
    let marker = temp.path().join("marker");
    let claim_path = temp.path().join("claim.json");
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "p2_1_fault_child", "--nocapture"])
            .env("OPEN_COMPUTE_P2_1_FAULT_CASE", format!("{point:?}"))
            .env("OPEN_COMPUTE_P2_1_DB", &database)
            .env("OPEN_COMPUTE_P2_1_MARKER", &marker)
            .env("OPEN_COMPUTE_P2_1_CLAIM", &claim_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_marker(&marker, &mut child.0);
    kill(&child.0);
    let status = child.0.wait().unwrap();
    assert!(!status.success());

    let reopened = open(&database, 1_000);
    match point {
        SchedulerFaultPoint::AfterCompleteCommit => {
            assert_eq!(reopened.workload_summary(1_000).unwrap().ready, 0);
        }
        SchedulerFaultPoint::DuringProjectionRefresh => {
            let [claim] = reopened
                .claim_due(1_000, 50, 1)
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(claim.row_token, "refreshed-token-01");
        }
        SchedulerFaultPoint::AfterClaimCommit
        | SchedulerFaultPoint::BeforeDispatch
        | SchedulerFaultPoint::AfterDispatchBeforeComplete => {
            let stale: ClaimedJob =
                serde_json::from_slice(&fs::read(&claim_path).unwrap()).unwrap();
            let [recovered] = reopened
                .claim_due(1_000, 50, 1)
                .unwrap()
                .try_into()
                .unwrap();
            assert_ne!(recovered.claim_token, stale.claim_token);
            assert!(
                !reopened
                    .finish_claim(&stale, ClaimResult::Delete, 1_001)
                    .unwrap()
            );
        }
    }
}

#[test]
fn p2_1_five_fresh_process_crash_boundaries_recover_exactly() {
    for point in [
        SchedulerFaultPoint::AfterClaimCommit,
        SchedulerFaultPoint::BeforeDispatch,
        SchedulerFaultPoint::AfterDispatchBeforeComplete,
        SchedulerFaultPoint::AfterCompleteCommit,
        SchedulerFaultPoint::DuringProjectionRefresh,
    ] {
        run_fault_case(point);
    }
}

#[test]
fn p2_1_schema_and_product_scope_remain_frozen() {
    assert_eq!(scheduler_migration_registry().len(), 2);
    assert_eq!(scheduler_migration_registry()[0].0, 1);
    assert_eq!(scheduler_migration_registry()[1].0, 2);
    assert_eq!(
        SchedulerKind::ALL.map(SchedulerKind::as_str),
        ["do_alarm", "queue", "cron", "workflow"]
    );

    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("scheduler.sqlite");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&database)
        .unwrap();
    drop(open(&database, 1));
    let connection = Connection::open(&database).unwrap();
    let result = connection.execute(
        "INSERT INTO scheduled_jobs (
           id, kind, namespace_resource_id, object_id, object_generation, row_token,
           due_at_ms, target_deployment_id, execution_generation, state, retry_count,
           created_at_ms, updated_at_ms
         ) VALUES (
           'future', 'queue', 'future', 'future', 1, 'future-token-0001',
           1, 'future', 1, 'scheduled', 0, 1, 1
         )",
        [],
    );
    assert!(result.is_err(), "future workload row bypassed schema fence");
    let config = open_compute_core::PlatformConfig::from_toml_str(
        "[scheduler.pools.queue]\nenabled = true\nmax_in_flight = 1\nclaim_batch = 256\n",
    )
    .unwrap();
    assert!(config.scheduler.pool(SchedulerKind::Queue).enabled);
}
