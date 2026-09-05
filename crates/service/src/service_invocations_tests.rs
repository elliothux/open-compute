use super::*;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::DataConfig;
use open_compute_core::{AccountId, RequestId, WorkerId};
use open_compute_storage::{
    NewVersion, NewVersionProducts, NewVersionService, PlatformStorage, VersionContentKind,
    WorkerRepository,
};
use std::collections::BTreeMap;

struct Fixture {
    _temp: tempfile::TempDir,
    storage: Arc<PlatformStorage>,
    caller_version: VersionId,
    target_version: VersionId,
    caller_digest: [u8; 32],
    target_digest: [u8; 32],
    caller_props: serde_json::Value,
}

fn fixture() -> Fixture {
    fixture_with_corrupt_props(false)
}

fn fixture_with_corrupt_props(corrupt_props: bool) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let request = RequestId::generate();
    let repo = WorkerRepository::new(storage.db());
    let caller = repo
        .create_worker(account, "registry-caller", request, 1, 1_000)
        .unwrap()
        .0;
    let target = repo
        .create_worker(account, "registry-target", request, 2, 1_000)
        .unwrap()
        .0;
    let target_descriptor =
        ServiceDescriptorV1::new("SELF".to_owned(), target.id, None, None).unwrap();
    let target_digest = target_descriptor.sha256().unwrap();
    let target_version = insert_ready(
        repo,
        account,
        target.id,
        [2; 32],
        &[NewVersionService {
            binding_name: "SELF".to_owned(),
            target_worker_id: target.id,
            entrypoint: None,
            props_json: None,
            descriptor_sha256: target_digest,
        }],
        request,
        3,
    );
    repo.promote(account, target.id, target_version, None, request, 5)
        .unwrap();
    let caller_props = serde_json::json!({
        "constructor": {"enabled": true},
        "z": [1, {"__proto__": "ordinary JSON data"}],
    });
    let caller_descriptor = ServiceDescriptorV1::new(
        "TARGET".to_owned(),
        target.id,
        None,
        Some(caller_props.clone()),
    )
    .unwrap();
    let caller_digest = caller_descriptor.sha256().unwrap();
    let caller_props_json = if corrupt_props {
        br#"{"z":[1,{"__proto__":"ordinary JSON data"}],"constructor":{"enabled":true}}"#.to_vec()
    } else {
        serde_json::to_vec(caller_descriptor.props.as_ref().unwrap()).unwrap()
    };
    let caller_version = insert_ready(
        repo,
        account,
        caller.id,
        [3; 32],
        &[NewVersionService {
            binding_name: "TARGET".to_owned(),
            target_worker_id: target.id,
            entrypoint: None,
            props_json: Some(caller_props_json),
            descriptor_sha256: caller_digest,
        }],
        request,
        6,
    );
    repo.promote(account, caller.id, caller_version, None, request, 8)
        .unwrap();
    Fixture {
        _temp: temp,
        storage,
        caller_version,
        target_version,
        caller_digest,
        target_digest,
        caller_props,
    }
}

fn insert_ready(
    repo: WorkerRepository<'_>,
    account: AccountId,
    worker: WorkerId,
    worker_digest: [u8; 32],
    services: &[NewVersionService],
    request: RequestId,
    now_ms: i64,
) -> VersionId {
    let version = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id: version,
            account_id: account,
            worker_id: worker,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some(worker_digest),
            artifact_size: Some(1),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: worker_digest,
            compatibility_date: "2026-08-30".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms,
        },
        &NewVersionProducts {
            services,
            ..NewVersionProducts::default()
        },
        1_000,
    )
    .unwrap();
    repo.begin_validation(version).unwrap();
    repo.mark_ready(version, now_ms + 1).unwrap();
    version
}

fn resolve_request(
    version: VersionId,
    binding_name: &str,
    digest: [u8; 32],
    parent_frame: Option<String>,
) -> ServiceResolveRequest {
    ServiceResolveRequest {
        caller_version_id: version,
        binding_name: binding_name.to_owned(),
        descriptor_sha256: hex::encode(digest),
        parent_frame,
        operation: ServiceOperation::Rpc,
    }
}

fn connect_request(
    version: VersionId,
    binding_name: &str,
    digest: [u8; 32],
) -> ServiceResolveRequest {
    ServiceResolveRequest {
        operation: ServiceOperation::Connect,
        ..resolve_request(version, binding_name, digest, None)
    }
}

#[test]
fn admission_delivers_canonical_arbitrary_json_props() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    let admission = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    assert_eq!(admission.target.props, Some(fixture.caller_props));
    registry
        .complete(&ServiceReleaseRequest {
            handle: admission.handle,
        })
        .unwrap();
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: admission.caller_frame,
        })
        .unwrap();
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[test]
fn admission_rejects_noncanonical_persisted_props_without_leaking_pins() {
    let fixture = fixture_with_corrupt_props(true);
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    assert_eq!(
        registry
            .resolve(&resolve_request(
                fixture.caller_version,
                "TARGET",
                fixture.caller_digest,
                None,
            ))
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );
    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[test]
fn connect_finalize_is_atomic_idempotent_and_releases_both_version_pins() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    let admission = registry
        .resolve(&connect_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
        ))
        .unwrap();
    let finalize = ServiceConnectFinalizeRequest {
        handle: admission.handle,
        caller_frame: admission.caller_frame,
    };
    assert_eq!(registry.counts(), (1, 1, 0));
    assert_eq!(pins.count(fixture.caller_version), 1);
    assert_eq!(pins.count(fixture.target_version), 1);

    registry.finalize_connect(&finalize).unwrap();
    registry.finalize_connect(&finalize).unwrap();

    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[test]
fn connect_finalize_rejects_a_non_connect_operation_without_releasing_it() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    let admission = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    assert_eq!(
        registry
            .finalize_connect(&ServiceConnectFinalizeRequest {
                handle: admission.handle.clone(),
                caller_frame: admission.caller_frame.clone(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );
    assert_eq!(registry.counts(), (1, 1, 0));
    registry
        .complete(&ServiceReleaseRequest {
            handle: admission.handle,
        })
        .unwrap();
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: admission.caller_frame,
        })
        .unwrap();
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[test]
fn connect_finalize_rejects_a_caller_frame_from_another_root() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    let first = registry
        .resolve(&connect_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
        ))
        .unwrap();
    let second = registry
        .resolve(&connect_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
        ))
        .unwrap();

    assert_eq!(
        registry
            .finalize_connect(&ServiceConnectFinalizeRequest {
                handle: first.handle.clone(),
                caller_frame: second.caller_frame.clone(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );
    assert_eq!(registry.counts(), (2, 2, 0));
    assert_eq!(pins.count(fixture.caller_version), 2);
    assert_eq!(pins.count(fixture.target_version), 2);

    for admission in [first, second] {
        registry
            .finalize_connect(&ServiceConnectFinalizeRequest {
                handle: admission.handle,
                caller_frame: admission.caller_frame,
            })
            .unwrap();
    }
    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[tokio::test]
async fn deadline_reaper_releases_an_unfinalized_connect_without_another_request() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::with_deadline(
        fixture.storage,
        pins.clone(),
        Duration::from_millis(5),
    );
    let admission = registry
        .resolve(&connect_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
        ))
        .unwrap();
    registry
        .retain(&ServiceRetainRequest {
            handle: admission.handle,
            owner: RetentionOwner::Target,
        })
        .unwrap();
    assert_eq!(registry.counts(), (1, 1, 1));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let reaper_registry = registry.clone();
    let reaper = tokio::spawn(async move {
        reaper_registry
            .reap_deadlines_until_shutdown(Duration::from_millis(1), async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while registry.counts() != (0, 0, 0) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
    let _ = shutdown_tx.send(());
    reaper.await.unwrap();
}

#[test]
fn returned_capability_holds_both_root_and_target_until_native_release() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    let admission = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    assert_eq!(pins.count(fixture.caller_version), 1);
    assert_eq!(pins.count(fixture.target_version), 1);
    let retention = registry
        .retain(&ServiceRetainRequest {
            handle: admission.handle.clone(),
            owner: RetentionOwner::Target,
        })
        .unwrap();
    registry
        .complete(&ServiceReleaseRequest {
            handle: admission.handle,
        })
        .unwrap();
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: admission.caller_frame,
        })
        .unwrap();
    assert_eq!(registry.counts(), (1, 0, 1));
    let capability = registry
        .begin_capability(&CapabilityBeginRequest {
            retention: retention.clone(),
            parent_frame: None,
        })
        .unwrap();
    registry
        .complete(&ServiceReleaseRequest {
            handle: capability.handle,
        })
        .unwrap();
    registry
        .release(&ServiceReleaseRequest { handle: retention })
        .unwrap();
    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
}

#[test]
fn recursive_self_calls_share_one_depth_budget_and_reject_the_seventeenth_hop() {
    let fixture = fixture();
    let registry = ServiceInvocationRegistry::new(fixture.storage, VersionPins::new());
    let first = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    let mut handles = vec![first.handle.clone()];
    let mut parent = first.frame.clone();
    for _ in 1..MAX_DEPTH {
        let admission = registry
            .resolve(&resolve_request(
                fixture.target_version,
                "SELF",
                fixture.target_digest,
                Some(parent),
            ))
            .unwrap();
        parent = admission.frame;
        handles.push(admission.handle);
    }
    assert_eq!(
        registry
            .resolve(&resolve_request(
                fixture.target_version,
                "SELF",
                fixture.target_digest,
                Some(parent),
            ))
            .unwrap_err()
            .code(),
        ErrorCode::ServiceLimitExceeded
    );
    for handle in handles.into_iter().rev() {
        registry
            .complete(&ServiceReleaseRequest { handle })
            .unwrap();
    }
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: first.caller_frame,
        })
        .unwrap();
    assert_eq!(registry.counts(), (0, 0, 0));
}

#[test]
fn sibling_concurrency_is_bounded_per_root_without_cross_root_interference() {
    let fixture = fixture();
    let registry = ServiceInvocationRegistry::new(fixture.storage, VersionPins::new());
    let first = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    let mut handles = vec![first.handle.clone()];
    for _ in 1..MAX_CONCURRENT_CALLS {
        handles.push(
            registry
                .resolve(&resolve_request(
                    fixture.target_version,
                    "SELF",
                    fixture.target_digest,
                    Some(first.frame.clone()),
                ))
                .unwrap()
                .handle,
        );
    }
    assert_eq!(
        registry
            .resolve(&resolve_request(
                fixture.target_version,
                "SELF",
                fixture.target_digest,
                Some(first.frame),
            ))
            .unwrap_err()
            .code(),
        ErrorCode::ServiceLimitExceeded
    );

    let independent = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    registry
        .complete(&ServiceReleaseRequest {
            handle: independent.handle,
        })
        .unwrap();
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: independent.caller_frame,
        })
        .unwrap();

    for handle in handles.into_iter().rev() {
        registry
            .complete(&ServiceReleaseRequest { handle })
            .unwrap();
    }
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: first.caller_frame,
        })
        .unwrap();
    assert_eq!(registry.counts(), (0, 0, 0));
}

#[test]
fn sequential_calls_share_one_total_budget_even_after_each_operation_drains() {
    let fixture = fixture();
    let registry = ServiceInvocationRegistry::new(fixture.storage, VersionPins::new());
    let first = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    registry
        .complete(&ServiceReleaseRequest {
            handle: first.handle.clone(),
        })
        .unwrap();
    for _ in 1..MAX_TOTAL_CALLS {
        let admission = registry
            .resolve(&resolve_request(
                fixture.caller_version,
                "TARGET",
                fixture.caller_digest,
                Some(first.caller_frame.clone()),
            ))
            .unwrap();
        registry
            .complete(&ServiceReleaseRequest {
                handle: admission.handle,
            })
            .unwrap();
    }
    assert_eq!(
        registry
            .resolve(&resolve_request(
                fixture.caller_version,
                "TARGET",
                fixture.caller_digest,
                Some(first.caller_frame.clone()),
            ))
            .unwrap_err()
            .code(),
        ErrorCode::ServiceLimitExceeded
    );
    registry
        .complete_root(&ServiceRootCompleteRequest {
            frame: first.caller_frame,
        })
        .unwrap();
    assert_eq!(registry.counts(), (0, 0, 0));
}

#[test]
fn confirmed_generation_exit_invalidates_handles_and_releases_every_pin() {
    let fixture = fixture();
    let pins = VersionPins::new();
    let registry = ServiceInvocationRegistry::new(fixture.storage, pins.clone());
    registry.activate_generation("generation-a");
    let admission = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    let retention = registry
        .retain(&ServiceRetainRequest {
            handle: admission.handle.clone(),
            owner: RetentionOwner::Target,
        })
        .unwrap();
    assert_eq!(registry.counts(), (1, 1, 1));

    registry.clear_generation("unrelated-private-claim");
    assert_eq!(registry.counts(), (1, 1, 1));
    registry.clear_after_child_exit();

    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
    assert_eq!(
        registry
            .begin_capability(&CapabilityBeginRequest {
                retention,
                parent_frame: None,
            })
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );

    registry.activate_generation("generation-a");
    let current = registry
        .resolve(&resolve_request(
            fixture.caller_version,
            "TARGET",
            fixture.caller_digest,
            None,
        ))
        .unwrap();
    registry.activate_generation("generation-b");
    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(pins.count(fixture.caller_version), 0);
    assert_eq!(pins.count(fixture.target_version), 0);
    registry.clear_generation("generation-a");
    assert_eq!(registry.counts(), (0, 0, 0));
    assert_eq!(
        registry
            .complete(&ServiceReleaseRequest {
                handle: current.handle,
            })
            .unwrap(),
        ()
    );
}
