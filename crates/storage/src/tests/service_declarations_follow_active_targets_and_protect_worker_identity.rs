use super::*;
use crate::workers::EffectiveResourceLimits;

#[test]
fn service_declarations_follow_active_targets_and_protect_worker_identity() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();
    let workers = WorkerRepository::new(storage.db());
    let (caller, _) = workers
        .create_worker(account, "service-caller", request, 1, 1_000_000)
        .unwrap();
    let (target, _) = workers
        .create_worker(account, "service-target", request, 2, 1_000_000)
        .unwrap();
    let target_v1 = insert_ready(&workers, account, target.id, [1; 32], request, 3);
    workers
        .promote(account, target.id, target_v1, None, request, 5)
        .unwrap();

    let caller_version = VersionId::generate();
    let descriptor = [7; 32];
    let service = crate::NewVersionService {
        binding_name: "CATALOG".to_owned(),
        target: crate::ServiceTarget::Worker {
            worker_id: target.id,
        },
        entrypoint: Some("CatalogApi".to_owned()),
        props_json: Some(br#"{"mode":"readonly"}"#.to_vec()),
        descriptor_sha256: descriptor,
    };
    let self_service = crate::NewVersionService {
        binding_name: "SELF".to_owned(),
        target: crate::ServiceTarget::Worker {
            worker_id: caller.id,
        },
        entrypoint: None,
        props_json: None,
        descriptor_sha256: [6; 32],
    };
    let extension_service = crate::NewVersionService {
        binding_name: "FILES".to_owned(),
        target: crate::ServiceTarget::Extension {
            name: "local-files".to_owned(),
        },
        entrypoint: None,
        props_json: None,
        descriptor_sha256: [5; 32],
    };
    let declarations = [service, self_service, extension_service];
    workers
        .insert_staging_version(
            &NewVersion {
                id: caller_version,
                account_id: account,
                worker_id: caller.id,
                content_kind: crate::VersionContentKind::Worker,
                artifact_sha256: Some([2; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [3; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                resource_limits: EffectiveResourceLimits::standard_defaults(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: request,
                now_ms: 6,
            },
            &crate::NewVersionProducts {
                services: &declarations,
                ..crate::NewVersionProducts::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(caller_version).unwrap();
    workers.mark_ready(caller_version, 7).unwrap();
    workers
        .promote(account, caller.id, caller_version, None, request, 8)
        .unwrap();

    let services = crate::ServiceRepository::new(storage.db());
    let first = services
        .resolve(caller_version, "CATALOG", &descriptor)
        .unwrap();
    assert!(matches!(
        first.target,
        crate::ResolvedServiceDestination::Worker { version_id, .. } if version_id == target_v1
    ));
    assert_eq!(first.service.entrypoint.as_deref(), Some("CatalogApi"));
    assert_eq!(
        first.service.props_json.as_deref(),
        Some(br#"{"mode":"readonly"}"#.as_slice())
    );
    assert!(matches!(
        services
            .resolve(caller_version, "FILES", &[5; 32])
            .unwrap()
            .target,
        crate::ResolvedServiceDestination::Extension { ref name } if name == "local-files"
    ));
    assert_eq!(
        services.inbound_referrers(account, target.id, 10).unwrap(),
        vec![crate::ServiceReferrer {
            caller_worker_id: caller.id,
            caller_version_id: caller_version,
            binding_name: "CATALOG".to_owned(),
        }]
    );
    assert_eq!(
        workers
            .delete_worker(account, target.id, &[target_v1], request, 9)
            .unwrap_err()
            .code(),
        ErrorCode::ServiceTargetReferenced
    );

    let target_v2 = insert_ready(&workers, account, target.id, [4; 32], request, 10);
    workers
        .promote(account, target.id, target_v2, Some(target_v1), request, 12)
        .unwrap();
    assert!(matches!(
        services
            .resolve(caller_version, "CATALOG", &descriptor)
            .unwrap()
            .target,
        crate::ResolvedServiceDestination::Worker { version_id, .. } if version_id == target_v2
    ));
    assert_eq!(
        services
            .resolve(caller_version, "CATALOG", &[8; 32])
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );
    workers
        .begin_force_delete(account, target.id, request, 12)
        .unwrap();
    assert_eq!(workers.force_delete_intents().unwrap().len(), 1);
    assert!(
        workers
            .delete_worker(account, target.id, &[target_v1, target_v2], request, 13)
            .is_err()
    );
    assert!(insert_ready_result(workers, account, target.id, [5; 32], request, 13).is_err());
    workers
        .finish_force_delete(account, target.id, &[target_v1, target_v2], request, 14)
        .unwrap();
    assert!(workers.force_delete_intents().unwrap().is_empty());
    assert!(
        services
            .resolve(caller_version, "CATALOG", &descriptor)
            .is_err()
    );
    assert_eq!(
        workers
            .delete_worker(account, caller.id, &[], request, 15)
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    workers
        .delete_worker(account, caller.id, &[caller_version], request, 16)
        .unwrap();
}
