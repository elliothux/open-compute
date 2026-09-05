use super::*;

#[test]
fn cancelled_checksum_task_keeps_file_quota_and_cpu_slot_until_finished() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let fixture = fixture().await;
        let service = &fixture.service;
        let slots = service.checksums.available_permits();
        let (path, mut file) = service
            .staging
            .create(fixture.resource, &uuid::Uuid::now_v7().to_string())
            .unwrap();
        std::io::Write::write_all(&mut file, b"checksum").unwrap();
        drop(file);
        let mut reservation = StagingReservation::new(service.staging_bytes.clone(), 1024, None);
        reservation.add(8).unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        ready.await.unwrap();
        let mut hashing = Box::pin(service.hash_staged_put(
            PutHeader {
                key: "checksum".to_owned(),
                options: PutWireOptions::default(),
            },
            8,
            StagingFile::new(path.clone()),
            reservation,
        ));
        assert!(futures::poll!(&mut hashing).is_pending());
        drop(hashing);
        assert!(path.exists());
        assert_eq!(service.staging_bytes.load(Ordering::Acquire), 8);
        assert_eq!(service.checksums.available_permits(), slots - 1);
        // The async scheduler is still usable while the CPU worker is unavailable.
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        blocker.await.unwrap();
        // One blocking worker makes this a deterministic drain barrier for the queued hash.
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        assert!(!path.exists());
        assert_eq!(service.staging_bytes.load(Ordering::Acquire), 0);
        assert_eq!(service.checksums.available_permits(), slots);
    });
}

#[tokio::test]
async fn checksum_failure_releases_staging_and_permit() {
    let fixture = fixture().await;
    let service = &fixture.service;
    let (path, file) = service
        .staging
        .create(fixture.resource, &uuid::Uuid::now_v7().to_string())
        .unwrap();
    drop(file);
    let mut reservation = StagingReservation::new(service.staging_bytes.clone(), 1024, None);
    reservation.add(1).unwrap();
    let slots = service.checksums.available_permits();
    let result = service
        .hash_staged_put(
            PutHeader {
                key: "invalid".to_owned(),
                options: PutWireOptions::default(),
            },
            1,
            StagingFile::new(path.clone()),
            reservation,
        )
        .await;
    assert_eq!(
        result.err().unwrap().code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    assert!(!path.exists());
    assert_eq!(service.staging_bytes.load(Ordering::Acquire), 0);
    assert_eq!(service.checksums.available_permits(), slots);
}
