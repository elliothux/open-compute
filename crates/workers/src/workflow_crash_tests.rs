//! SIGKILL at public repository boundaries, with fresh owners reopening both WAL databases.

use super::*;
use open_compute_storage::scheduler::WorkflowState;
use std::io::{BufRead as _, Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const CHILD: &str = "workflows::tests::crash_matrix::workflow_crash_child";
const MARKER: &str = "WORKFLOW_CRASH_BOUNDARY";

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1024 * 1024 * 1024,
        free_space_hard_bytes: 256 * 1024 * 1024,
    }
}

#[test]
fn workflow_disk_pressure_refuses_create_before_identity_reservation() {
    let (_temp, storage, scheduler, definition) = fixture();
    let mut config = storage_config(storage.data_dir().root());
    drop(scheduler);
    drop(storage);
    // A conservative operator floor exceeds this fixture filesystem's capacity;
    // no disk filling, free-space mock, or production fault hook is needed.
    config.free_space_hard_bytes = 1 << 60;
    config.free_space_soft_bytes = 1 << 61;
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler = SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5000, 0).unwrap();
    let limits = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &limits);
    assert_eq!(
        controller
            .create(
                storage.identity().default_account_id,
                definition,
                Some("pressure"),
                "null",
                10
            )
            .unwrap_err()
            .code(),
        ErrorCode::StoragePressure
    );
    assert!(
        WorkflowRepository::new(storage.db())
            .live_reservations(None, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        scheduler
            .workflow_instance_ids(None, 10)
            .unwrap()
            .is_empty()
    );
}

fn checkpoint(selected: &str, point: &str) {
    if selected == point {
        println!("{MARKER}");
        std::io::stdout().flush().unwrap();
        // The parent owns stdin and kills this exact child. No destructor, SQLite close,
        // checkpoint, or graceful platform cleanup can run after this boundary.
        let mut byte = [0];
        let _ = std::io::stdin().read_exact(&mut byte);
        panic!("crash parent closed the pipe without killing the fixture");
    }
}

#[test]
fn workflow_crash_child() {
    let Ok(root) = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_DATA") else {
        return;
    };
    let cut = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_POINT").unwrap();
    let definition = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_DEFINITION")
        .unwrap()
        .parse()
        .unwrap();
    let storage =
        PlatformStorage::bootstrap(&storage_config(Path::new(&root)), &SystemClock).unwrap();
    let scheduler =
        SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5000, 10).unwrap();
    let repository = WorkflowRepository::new(storage.db());
    let limits = WorkflowsConfig::default();
    checkpoint(&cut, "before-reserve");
    let reservation = repository
        .reserve_instance(
            storage.identity().default_account_id,
            definition,
            Some("durable-id"),
            &limits,
            10,
        )
        .unwrap();
    checkpoint(&cut, "reserved");
    scheduler
        .insert_workflow(&reservation.identity, "null", &limits)
        .unwrap();
    checkpoint(&cut, "inserted");
    repository
        .finalize_instance(&reservation.identity, 11)
        .unwrap();
    checkpoint(&cut, "finalized");
    let run = WorkflowController::new(&storage, &scheduler, &limits)
        .claim(12)
        .unwrap()
        .unwrap();
    checkpoint(&cut, "run-claimed");
    let descriptor = step();
    let WorkflowStepGrant::Run { step_token } = scheduler
        .claim_workflow_step(&run.fence, &descriptor, 13, &limits)
        .unwrap()
    else {
        panic!("new callback was not granted");
    };
    checkpoint(&cut, "step-claimed");
    if cut == "step-failed" {
        scheduler
            .fail_workflow_step(
                &run.fence,
                0,
                &step_token,
                ErrorCode::WorkflowExecutionFailed,
                14,
                &limits,
            )
            .unwrap();
        checkpoint(&cut, "step-failed");
    }
    scheduler
        .complete_workflow_step(&run.fence, 0, &step_token, "42", 14, &limits)
        .unwrap();
    checkpoint(&cut, "step-completed");
    scheduler
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "42".into(),
                final_ordinal: 1,
            },
            15,
            &limits,
        )
        .unwrap();
    checkpoint(&cut, "terminal");
    repository
        .release_instance(&reservation.identity, 16)
        .unwrap();
    checkpoint(&cut, "released");
    panic!("unrecognized crash boundary");
}

fn step() -> WorkflowStepIdentity {
    WorkflowStepIdentity {
        ordinal: 0,
        name: "durable-step".into(),
        name_count: 1,
        config_json: "null".into(),
    }
}

struct Evidence(Option<tempfile::TempDir>);

impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(temp) = self.0.take()
        {
            let path = temp.keep();
            let failed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.p2-4-run/failed");
            std::fs::create_dir_all(&failed).unwrap();
            let destination = failed.join(format!("workflow-saga-{}", RequestId::generate()));
            std::fs::rename(&path, &destination).unwrap();
            eprintln!("Workflow saga failure evidence: {}", destination.display());
        }
    }
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

#[test]
fn workflow_sigkill_create_run_step_and_release_boundaries() {
    for cut in [
        "before-reserve",
        "reserved",
        "inserted",
        "finalized",
        "run-claimed",
        "step-claimed",
        "step-completed",
        "step-failed",
        "terminal",
        "released",
    ] {
        let (temp, storage, scheduler, definition) = fixture();
        let root = storage.data_dir().root().to_owned();
        let config = storage_config(&root);
        let _evidence = Evidence(Some(temp));
        drop(scheduler);
        drop(storage);
        let stderr = std::fs::File::create(root.join("crash-child.stderr")).unwrap();
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
                .env("OPEN_COMPUTE_WORKFLOW_CRASH_DATA", &root)
                .env("OPEN_COMPUTE_WORKFLOW_CRASH_POINT", cut)
                .env(
                    "OPEN_COMPUTE_WORKFLOW_CRASH_DEFINITION",
                    definition.to_string(),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(stderr)
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let reached = std::io::BufReader::new(stdout)
                .lines()
                .any(|line| line.is_ok_and(|line| line.contains(MARKER)));
            let _ = sender.send(reached);
        });
        let reached = receiver.recv_timeout(Duration::from_secs(30));
        // Kill through the owned process handle and reap even if the checkpoint failed.
        if child.0.try_wait().unwrap().is_none() {
            child.0.kill().unwrap();
        }
        let status = child.0.wait().unwrap();
        reader.join().unwrap();
        assert!(
            matches!(reached, Ok(true)),
            "checkpoint {cut}: {reached:?}; {status}"
        );
        assert_eq!(status.signal(), Some(9), "{cut}");

        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let scheduler = SchedulerStore::open(&root.join("scheduler.sqlite"), 5000, 120000).unwrap();
        let account = storage.identity().default_account_id;
        let limits = WorkflowsConfig::default();
        let repository = WorkflowRepository::new(storage.db());
        let controller = WorkflowController::new(&storage, &scheduler, &limits);
        if matches!(cut, "before-reserve" | "reserved") {
            assert!(
                scheduler
                    .workflow_instance_ids(None, 10)
                    .unwrap()
                    .is_empty()
            );
            if cut == "reserved" {
                let reservation = repository.find_instance(definition, "durable-id").unwrap();
                assert!(
                    repository
                        .instance_referrers_intact(&reservation.identity)
                        .unwrap()
                );
                controller
                    .reconcile(&mut WorkflowReconcileCursor::default(), 10, 11)
                    .unwrap();
                assert_eq!(
                    repository
                        .find_instance(definition, "durable-id")
                        .unwrap()
                        .state,
                    WorkflowRefState::Creating
                );
            }
            controller
                .reconcile(&mut WorkflowReconcileCursor::default(), 10, 120000)
                .unwrap();
            assert_eq!(
                repository
                    .find_instance(definition, "durable-id")
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowInstanceNotFound
            );
            controller
                .create(account, definition, Some("durable-id"), "null", 120000)
                .unwrap();
        }
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 10, 120000)
            .unwrap();
        let identity = repository
            .find_instance(definition, "durable-id")
            .unwrap()
            .identity;
        assert_eq!(
            scheduler.workflow_instance_ids(None, 10).unwrap(),
            vec![identity.instance_id]
        );
        assert_eq!(
            controller
                .create(account, definition, Some("durable-id"), "null", 120000)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceAlreadyExists
        );
        if matches!(cut, "terminal" | "released") {
            assert!(controller.claim(121001).unwrap().is_none());
        } else {
            let run = controller.claim(121001).unwrap().unwrap();
            let grant = scheduler
                .claim_workflow_step(&run.fence, &step(), 121002, &limits)
                .unwrap();
            let completion = match (cut, grant) {
                ("step-completed", WorkflowStepGrant::Complete { output_json }) => {
                    assert_eq!(output_json, "42");
                    WorkflowCompletion::Complete {
                        output_json,
                        final_ordinal: 1,
                    }
                }
                ("step-failed", WorkflowStepGrant::Failed { error_code, .. }) => {
                    assert_eq!(error_code, "WORKFLOW_EXECUTION_FAILED");
                    WorkflowCompletion::Errored {
                        code: ErrorCode::WorkflowExecutionFailed,
                    }
                }
                (_, WorkflowStepGrant::Run { step_token }) => {
                    scheduler
                        .complete_workflow_step(&run.fence, 0, &step_token, "42", 121003, &limits)
                        .unwrap();
                    WorkflowCompletion::Complete {
                        output_json: "42".into(),
                        final_ordinal: 1,
                    }
                }
                (_, unexpected) => panic!("wrong replay after {cut}: {unexpected:?}"),
            };
            scheduler
                .finish_workflow(&run.fence, &completion, 121004, &limits)
                .unwrap();
            controller
                .reconcile(&mut WorkflowReconcileCursor::default(), 10, 121005)
                .unwrap();
        }
        scheduler
            .verify_workflow_history(identity.instance_id)
            .unwrap();
        assert!(!repository.instance_referrers_intact(&identity).unwrap());
        let record = scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            record.state,
            if cut == "step-failed" {
                WorkflowState::Errored
            } else {
                WorkflowState::Complete
            }
        );
        assert_eq!(
            repository
                .find_instance(definition, "durable-id")
                .unwrap()
                .state,
            WorkflowRefState::Released
        );
        assert!(controller.claim(200000).unwrap().is_none());
    }
}
