use super::*;
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::run::daemon_control::RegisteredTokens;
use open_compute_core::SecretString;
use std::time::SystemTime;

#[test]
fn global_cache_clean_touches_only_known_unpinned_scope_entries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    assert_eq!(
        clean_global_cache(&root, true, false).unwrap(),
        CacheCleanReport::default()
    );
    assert!(!root.exists());

    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let update = cache.join("update-check.json");
    let unknown = cache.join("operator-data");
    std::fs::write(&update, b"cache").unwrap();
    std::fs::write(&unknown, b"keep").unwrap();
    assert_eq!(clean_global_cache(&root, true, false).unwrap().bytes, 5);
    assert!(update.exists());
    assert_eq!(clean_global_cache(&root, false, true).unwrap().skipped, 1);
    assert!(update.exists());
    assert_eq!(clean_global_cache(&root, false, false).unwrap().bytes, 5);
    assert!(!update.exists());
    assert_eq!(std::fs::read(&unknown).unwrap(), b"keep");
}

#[test]
fn online_global_clean_requires_healthy_instance_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ocd");
    let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("data");
    crate::setup::create_instance(&root, ServiceScope::User, &config, &data, None).unwrap();
    let record = registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    let id = record.instance_id().unwrap();
    let plan = DaemonPlan {
        manifest_digest: crate::instance_registry::manifest_digest(&root).unwrap(),
        root: root.clone(),
        scope: ServiceScope::User,
        registry,
        records: vec![record.clone()],
        credentials: Vec::new(),
    };
    let (api, _commands) = DaemonApi::channel(
        &[record],
        vec![RegisteredTokens {
            instance_id: id,
            deployer: SecretString::new("deployer"),
            read_only: SecretString::new("reader"),
        }],
        SecretString::new("admin"),
    )
    .unwrap();
    let update = root.join("cache/update-check.json");
    std::fs::create_dir_all(update.parent().unwrap()).unwrap();
    std::fs::write(&update, b"cache").unwrap();
    assert_eq!(
        clean_global_online(&plan, &HashMap::new(), &api, None, true)
            .unwrap()
            .skipped,
        1
    );
    assert!(update.exists());
    let (shutdown, _receiver) = watch::channel(false);
    let active = HashMap::from([(id, shutdown)]);
    assert_eq!(
        clean_global_online(&plan, &active, &api, None, false)
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    api.mark(&id, "running", None).unwrap();
    assert_eq!(
        clean_global_online(&plan, &active, &api, None, false)
            .unwrap()
            .skipped,
        1
    );
    assert!(update.exists());
}
