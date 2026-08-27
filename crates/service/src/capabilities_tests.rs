use super::*;

#[test]
fn workflow_defaults_preserve_the_signed_p2_3_snapshot_policy() {
    // This is the persisted P2.3 wire shape, not another production policy type.
    #[derive(Serialize)]
    struct PreviousPolicy<'a> {
        schema_version: u32,
        sqlite_busy_timeout_ms: u64,
        free_space_soft_bytes: u64,
        free_space_hard_bytes: u64,
        hardening: &'a HardeningConfig,
        workers: &'a WorkersConfig,
        kv: &'a KvConfig,
        r2: &'a R2Config,
        d1: &'a D1Config,
        durable_objects: &'a DurableObjectsConfig,
        scheduler: &'a SchedulerConfig,
        cache: &'a CacheConfig,
    }
    let mut loaded = LoadedConfig {
        path: "/unused/policy.toml".into(),
        config: open_compute_core::PlatformConfig::from_toml_str("").unwrap(),
    };
    let c = &loaded.config;
    let old = PreviousPolicy {
        schema_version: 1,
        sqlite_busy_timeout_ms: c.storage.sqlite_busy_timeout_ms,
        free_space_soft_bytes: c.storage.free_space_soft_bytes,
        free_space_hard_bytes: c.storage.free_space_hard_bytes,
        hardening: &c.hardening,
        workers: &c.workers,
        kv: &c.kv,
        r2: &c.r2,
        d1: &c.d1,
        durable_objects: &c.durable_objects,
        scheduler: &c.scheduler,
        cache: &c.cache,
    };
    let mut digest = Sha256::new();
    digest.update(b"open-compute/snapshot-config-policy/v1\0");
    digest.update(serde_json::to_vec(&old).unwrap());
    let expected = hex::encode(digest.finalize());
    assert_eq!(platform_config_policy_sha256(&loaded).unwrap(), expected);
    loaded.config.workflows.max_steps = 512;
    assert_ne!(platform_config_policy_sha256(&loaded).unwrap(), expected);
}
