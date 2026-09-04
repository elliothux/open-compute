use super::*;

#[test]
fn backup_kind_messages_remain_product_specific() {
    assert_eq!(BackupKind::Kv.prefix(), "backups/kv/");
    assert_eq!(BackupKind::D1.prefix(), "backups/d1/");
    for (kind, product) in [(BackupKind::Kv, "KV"), (BackupKind::D1, "D1")] {
        assert!(kind.key_error().starts_with(product));
        assert!(kind.size_error().starts_with(product));
        assert!(kind.staging_error().starts_with(product));
        assert!(kind.manifest_size_error().starts_with(product));
        assert!(kind.canonical_error().starts_with(product));
    }
}

#[tokio::test]
async fn artifact_fence_debug_views_do_not_expose_guard_state() {
    let gate = Arc::new(RwLock::new(()));
    let reservation = ArtifactVersionReservation {
        _guard: gate.clone().read_owned().await,
    };
    assert_eq!(
        format!("{reservation:?}"),
        "ArtifactVersionReservation { .. }"
    );
    drop(reservation);
    let fence = ArtifactGcFence {
        _guard: gate.write_owned().await,
    };
    assert_eq!(format!("{fence:?}"), "ArtifactGcFence { .. }");
}
