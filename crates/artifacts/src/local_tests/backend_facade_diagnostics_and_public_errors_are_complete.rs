use super::*;

#[tokio::test]
async fn backend_facade_diagnostics_and_public_errors_are_complete() {
    let cases = [
        (
            BackendError::NotFound,
            ErrorCode::ObjectStorageUnavailable,
            "object storage object was not found",
            "object not found",
        ),
        (
            BackendError::PreconditionFailed,
            ErrorCode::ObjectStorageUnavailable,
            "object storage precondition failed",
            "object precondition failed",
        ),
        (
            BackendError::InvalidRange,
            ErrorCode::ObjectStorageIntegrityError,
            "object storage range is invalid",
            "object range is invalid",
        ),
        (
            BackendError::Corrupt,
            ErrorCode::ObjectStorageIntegrityError,
            "object storage integrity verification failed",
            "object storage integrity failure",
        ),
        (
            BackendError::Unavailable,
            ErrorCode::ObjectStorageUnavailable,
            "object storage is unavailable",
            "object storage unavailable",
        ),
        (
            BackendError::Capacity,
            ErrorCode::ObjectStorageCapacity,
            "object storage capacity is exhausted",
            "object storage capacity exhausted",
        ),
        (
            BackendError::InvalidKey,
            ErrorCode::ConfigInvalid,
            "object storage key is invalid",
            "object key is invalid",
        ),
        (
            BackendError::CustomerKeyInvalid,
            ErrorCode::ObjectStorageIntegrityError,
            "object storage customer key is invalid",
            "customer encryption key is invalid",
        ),
        (
            BackendError::MultipartInvalid,
            ErrorCode::ObjectStorageIntegrityError,
            "object storage multipart state is invalid",
            "multipart upload is invalid",
        ),
        (
            BackendError::AuthorityMismatch,
            ErrorCode::ObjectStorageAuthorityMismatch,
            "object storage authority does not match this platform",
            "object storage authority mismatch",
        ),
    ];
    for (backend, code, message, display) in cases {
        assert_eq!(backend.to_string(), display);
        let public = crate::error::from_backend(backend);
        assert_eq!(public.code(), code);
        assert_eq!(public.message(), message);
    }
    let missing = crate::error::from_backend(BackendError::NotFound);
    assert!(crate::error::is_not_found(&missing));
    assert_eq!(
        crate::error::integrity_error().code(),
        ErrorCode::ArtifactIntegrityError
    );

    let memory = ObjectSource::Bytes(Bytes::from_static(b"abc"));
    assert_eq!(memory.length(), 3);
    assert_eq!(format!("{memory:?}"), "ObjectSource::Bytes { length: 3 }");
    let source_root = tempfile::tempdir().unwrap();
    let source_path = source_root.path().join("source");
    write_private(&source_path, b"abc");
    let source = ObjectSource::File {
        file: crate::backend::open_private_source(&source_path, 3).unwrap(),
        length: 3,
    };
    assert_eq!(source.length(), 3);
    assert!(format!("{source:?}").contains("length: 3"));
    let customer = CustomerKey::new([0x5a; 32]);
    assert_eq!(format!("{customer:?}"), "CustomerKey { .. }");

    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender.send(Ok(Bytes::from_static(b"abc"))).await.unwrap();
    sender
        .send(Err(std::io::Error::other("bounded stream failure")))
        .await
        .unwrap();
    drop(sender);
    let body = crate::ObjectBody::from_local(receiver);
    assert_eq!(format!("{body:?}"), "ObjectBody { .. }");
    let mut reader = body.into_async_read();
    let mut observed = Vec::new();
    assert!(reader.read_to_end(&mut observed).await.is_err());
    assert_eq!(observed, b"abc");

    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = Fixture::new();
    assert_eq!(backend.kind(), ObjectStorageKind::Local);
    assert_eq!(backend.prefix(), config.prefix);
    assert_eq!(backend.r2_prefix(), config.r2_prefix);
    assert_eq!(backend.max_object_bytes(), LIMIT);
    assert!(backend.available_bytes().unwrap().is_some());
    assert!(format!("{backend:?}").contains("authority_sha256"));
    let (inspected_id, inspected_authority, available) =
        ObjectBackend::inspect_local_authority(&config).unwrap();
    assert_eq!(inspected_id, platform_id);
    assert_eq!(inspected_authority, backend.authority_sha256());
    assert!(available > 0);
    backend.recover().await.unwrap();
    assert!(!backend.delete_many(&[]).await.unwrap());
    assert_eq!(
        ObjectBackend::open_local(&config, platform_id, 0)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let mut relative = config.clone();
    relative.path = PathBuf::from("relative-object-root");
    assert_eq!(
        ObjectBackend::open_local(&relative, platform_id, LIMIT)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageIntegrityError
    );
    drop(backend);
    let (reopened, discovered_id) = ObjectBackend::open_local_existing(&config, LIMIT).unwrap();
    assert_eq!(discovered_id, platform_id);
    assert_eq!(reopened.authority_sha256(), inspected_authority);
    drop(reopened);
    drop(_temp);
}
