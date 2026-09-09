use super::*;

#[tokio::test]
async fn version_commit_reservation_fences_artifact_gc() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let reservation = store.reserve_version_artifact().await;
    let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
    let gc_store = store.clone();
    let gc = tokio::spawn(async move {
        let _fence = gc_store.fence_version_gc().await;
        let _ = acquired_tx.send(());
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut acquired_rx)
            .await
            .is_err(),
        "GC must wait until the version reference can be committed"
    );
    drop(reservation);
    tokio::time::timeout(Duration::from_secs(1), &mut acquired_rx)
        .await
        .expect("GC fence acquisition deadline")
        .expect("GC fence sender");
    gc.await.expect("GC fence task");
}
