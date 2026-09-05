use crate::local::LocalFaultPoint;
use crate::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ObjectBackend, ObjectHttpMetadata,
    ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, PutMode, PutOptions, R2BucketIdentity,
    R2ChecksumAlgorithm, R2GetResult, R2HttpMetadata, R2MultipartCreateOptions, R2ObjectStore,
    R2PutOptions, R2Range, R2SsecKey, R2StorageClass, R2UploadSource, UserObjectKey, hash_bytes,
};
use bytes::Bytes;
use md5::Digest as _;
use open_compute_core::{
    ErrorCode, LocalObjectStorageConfig, ObjectStorageKind, PlatformId, ResourceId,
};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt as _;

const LIMIT: u64 = 4 * 1024 * 1024;

struct Fixture {
    _temp: tempfile::TempDir,
    config: LocalObjectStorageConfig,
    platform_id: PlatformId,
    backend: ObjectBackend,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = LocalObjectStorageConfig {
            path: temp.path().join("objects"),
            free_space_soft_bytes: 1,
            free_space_hard_bytes: 1,
            partial_grace_ms: 1,
            ..LocalObjectStorageConfig::default()
        };
        let platform_id = PlatformId::generate();
        let backend = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
        Self {
            _temp: temp,
            config,
            platform_id,
            backend,
        }
    }

    fn object_file(&self, key: &ObjectKey) -> PathBuf {
        let mut path = self.config.path.join("objects");
        for segment in key.as_str().split('/') {
            path.push(segment);
        }
        path.join("object.ocobj")
    }
}

fn options(mode: PutMode) -> PutOptions {
    PutOptions {
        mode,
        metadata: ObjectMetadata {
            user: BTreeMap::from([("name".to_owned(), "value".to_owned())]),
            http: ObjectHttpMetadata {
                content_type: Some("text/plain".to_owned()),
                cache_control: Some("max-age=60".to_owned()),
                ..ObjectHttpMetadata::default()
            },
            ..ObjectMetadata::default()
        },
        customer_key: None,
    }
}

async fn bytes(backend: &ObjectBackend, key: &ObjectKey, options: GetOptions) -> Bytes {
    backend
        .get(key, options)
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes()
}

#[test]
fn physical_key_grammar_rejects_ambiguous_or_unsafe_names() {
    for value in [
        "", "/a", "a/", "a//b", ".", "..", "a/./b", "a/../b", "a\\b", "a\0b", "a\nb",
    ] {
        assert_eq!(ObjectKey::new(value).unwrap_err(), BackendError::InvalidKey);
    }
    assert_eq!(
        ObjectKey::new(format!("a/{}", "x".repeat(256))).unwrap_err(),
        BackendError::InvalidKey
    );
    assert!(ObjectKey::new("system/a.b-c_d=1+2@x").is_ok());
}

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

#[tokio::test]
async fn local_contract_put_head_get_range_list_conditions_delete_and_restart() {
    let fixture = Fixture::new();
    let first = ObjectKey::new("system/contracts/a").unwrap();
    let second = ObjectKey::new("system/contracts/a/child").unwrap();
    let first_meta = fixture
        .backend
        .put(
            &first,
            ObjectSource::Bytes(Bytes::from_static(b"0123456789")),
            options(PutMode::CreateOnly),
        )
        .await
        .unwrap();
    assert_eq!(first_meta.size, 10);
    assert_eq!(
        first_meta.user.get("name").map(String::as_str),
        Some("value")
    );
    assert_eq!(
        fixture
            .backend
            .put(
                &first,
                ObjectSource::Bytes(Bytes::from_static(b"different")),
                options(PutMode::CreateOnly),
            )
            .await
            .unwrap_err(),
        BackendError::PreconditionFailed
    );
    fixture
        .backend
        .put(
            &second,
            ObjectSource::Bytes(Bytes::from_static(b"child")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    assert_eq!(
        bytes(
            &fixture.backend,
            &first,
            GetOptions {
                range: Some(ObjectRange { start: 2, end: 5 }),
                ..GetOptions::default()
            }
        )
        .await,
        Bytes::from_static(b"2345")
    );
    let replacement = fixture
        .backend
        .put(
            &first,
            ObjectSource::Bytes(Bytes::from_static(b"replacement")),
            options(PutMode::IfMatch(first_meta.etag.clone())),
        )
        .await
        .unwrap();
    assert_ne!(replacement.etag, first_meta.etag);
    assert_eq!(
        fixture
            .backend
            .put(
                &first,
                ObjectSource::Bytes(Bytes::from_static(b"stale")),
                options(PutMode::IfMatch(first_meta.etag)),
            )
            .await
            .unwrap_err(),
        BackendError::PreconditionFailed
    );
    let page = fixture
        .backend
        .list("system/contracts/", 1, None)
        .await
        .unwrap();
    assert_eq!(page.objects.len(), 1);
    let cursor = page.next_cursor.unwrap();
    assert_eq!(
        fixture
            .backend
            .list("system/contracts/", 10, Some("not-a-local-cursor"))
            .await
            .unwrap_err(),
        BackendError::InvalidKey
    );
    let page = fixture
        .backend
        .list("system/contracts/", 10, Some(&cursor))
        .await
        .unwrap();
    assert_eq!(page.objects.len(), 1);
    fixture.backend.delete(&first).await.unwrap();
    fixture.backend.delete(&first).await.unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&first, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::NotFound
    );

    let fingerprint = fixture.backend.authority_sha256();
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert_eq!(reopened.authority_sha256(), fingerprint);
    assert_eq!(
        bytes(&reopened, &second, GetOptions::default()).await,
        Bytes::from_static(b"child")
    );
    drop(_temp);
}

#[tokio::test]
async fn local_create_only_is_atomic_under_concurrency() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/race/value").unwrap();
    let mut tasks = Vec::new();
    for value in 0_u8..16 {
        let backend = fixture.backend.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            backend
                .put(
                    &key,
                    ObjectSource::Bytes(Bytes::from(vec![value; 64])),
                    options(PutMode::CreateOnly),
                )
                .await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => successes += 1,
            Err(BackendError::PreconditionFailed) => {}
            Err(error) => panic!("unexpected create-only result: {error}"),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(
        bytes(&fixture.backend, &key, GetOptions::default())
            .await
            .len(),
        64
    );
}

#[tokio::test]
async fn opened_file_sources_require_exact_private_single_link_files() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/source/value").unwrap();
    let path = fixture.config.path.parent().unwrap().join("source");
    fs::write(&path, b"source").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mismatched = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: mismatched,
                    length: 5,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    let link = path.with_extension("link");
    fs::hard_link(&path, &link).unwrap();
    let linked = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: linked,
                    length: 6,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::remove_file(link).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let public = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: public,
                    length: 6,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
}

#[tokio::test]
async fn local_object_tree_rejects_symlink_ancestors_and_special_leaves() {
    let fixture = Fixture::new();
    let outside = fixture.config.path.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.config.path.join("objects/escape")).unwrap();
    let escape = ObjectKey::new("escape/value").unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &escape,
                ObjectSource::Bytes(Bytes::from_static(b"blocked")),
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());

    let key = ObjectKey::new("system/special/value").unwrap();
    let parent = fixture.object_file(&key).parent().unwrap().to_owned();
    fs::create_dir_all(&parent).unwrap();
    for ancestor in parent
        .ancestors()
        .take_while(|path| *path != fixture.config.path)
    {
        fs::set_permissions(ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(
        std::process::Command::new("mkfifo")
            .arg(fixture.object_file(&key))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
}

#[tokio::test]
async fn local_envelope_truncation_and_plaintext_tamper_fail_closed() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/corruption/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"authenticated payload")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let path = fixture.object_file(&key);
    let original = fs::read(&path).unwrap();
    fs::write(&path, &original[..8]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::write(&path, &original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut tampered = original;
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    fs::write(&path, tampered).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let body = fixture
        .backend
        .get(&key, GetOptions::default())
        .await
        .unwrap()
        .body;
    assert!(body.collect().await.is_err());
}

#[tokio::test]
async fn local_ssec_is_authenticated_chunked_and_never_persists_plaintext() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/encrypted").unwrap();
    let plaintext = Bytes::from(vec![b'Q'; 150_000]);
    let customer = CustomerKey::new([7; 32]);
    let mut put = options(PutMode::Replace);
    put.customer_key = Some(customer.clone());
    fixture
        .backend
        .put(&key, ObjectSource::Bytes(plaintext.clone()), put)
        .await
        .unwrap();
    let persisted = fs::read(fixture.object_file(&key)).unwrap();
    assert!(
        !persisted
            .windows(1024)
            .any(|window| window == &plaintext[..1024])
    );
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::CustomerKeyInvalid
    );
    assert_eq!(
        fixture
            .backend
            .head(
                &key,
                HeadOptions {
                    customer_key: Some(CustomerKey::new([8; 32])),
                },
            )
            .await
            .unwrap_err(),
        BackendError::CustomerKeyInvalid
    );
    let ranged = bytes(
        &fixture.backend,
        &key,
        GetOptions {
            range: Some(ObjectRange {
                start: 65_530,
                end: 65_550,
            }),
            customer_key: Some(customer.clone()),
            ..GetOptions::default()
        },
    )
    .await;
    assert_eq!(ranged, Bytes::from(vec![b'Q'; 21]));

    let path = fixture.object_file(&key);
    let mut persisted = fs::read(&path).unwrap();
    let last = persisted.len() - 1;
    persisted[last] ^= 1;
    fs::write(&path, persisted).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = fixture
        .backend
        .get(
            &key,
            GetOptions {
                customer_key: Some(customer),
                ..GetOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(output.body.collect().await.is_err());
}

#[tokio::test]
async fn local_multipart_persists_encrypted_parts_and_publishes_atomically() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/multipart").unwrap();
    let customer = CustomerKey::new([9; 32]);
    let upload = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), Some(customer.clone()))
        .await
        .unwrap();
    let first = fixture
        .backend
        .upload_part(
            &key,
            &upload,
            1,
            ObjectSource::Bytes(Bytes::from_static(b"first-part-secret")),
            Some(customer.clone()),
        )
        .await
        .unwrap();
    let second = fixture
        .backend
        .upload_part(
            &key,
            &upload,
            2,
            ObjectSource::Bytes(Bytes::from_static(b"second-part-secret")),
            Some(customer.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.backend.list_multipart(&key).await.unwrap(),
        vec![upload.clone()]
    );
    for file in regular_files(&fixture.config.path.join("multipart")) {
        let persisted = fs::read(file).unwrap();
        assert!(!persisted.windows(11).any(|window| window == b"part-secret"));
    }
    fixture
        .backend
        .complete_multipart(&key, &upload, &[first, second], Some(customer.clone()))
        .await
        .unwrap();
    assert!(
        fixture
            .backend
            .list_multipart(&key)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        bytes(
            &fixture.backend,
            &key,
            GetOptions {
                customer_key: Some(customer),
                ..GetOptions::default()
            },
        )
        .await,
        Bytes::from_static(b"first-part-secretsecond-part-secret")
    );

    let aborted = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    fixture
        .backend
        .abort_multipart(&key, &aborted)
        .await
        .unwrap();
    fixture
        .backend
        .abort_multipart(&key, &aborted)
        .await
        .unwrap();
}

#[tokio::test]
async fn local_typed_r2_round_trips_metadata_conditions_ssec_and_multipart() {
    let fixture = Fixture::new();
    let store = R2ObjectStore::new(fixture.backend.clone());
    let resource_id = ResourceId::generate();
    let locator = store
        .locator(resource_id, &store.physical_prefix(resource_id))
        .unwrap();
    store
        .ensure_identity(
            &locator,
            &R2BucketIdentity {
                schema_version: 1,
                platform_id: fixture.platform_id,
                resource_id,
                created_at_ms: 1,
            },
        )
        .await
        .unwrap();

    let body = b"local typed R2 body";
    let source_path = fixture.config.path.parent().unwrap().join("r2-source");
    write_private(&source_path, body);
    let source = R2UploadSource {
        path: source_path,
        length: body.len() as u64,
        checksums: hash_bytes(body),
        version: uuid::Uuid::now_v7().hyphenated().to_string(),
    };
    let key = UserObjectKey::parse("folder/中文 + %.txt").unwrap();
    let ssec = R2SsecKey::parse_hex(&"ab".repeat(32)).unwrap();
    let put = store
        .put_file(
            &locator,
            &key,
            &source,
            &R2PutOptions {
                http_metadata: R2HttpMetadata {
                    content_type: Some("text/plain; charset=utf-8".to_owned()),
                    ..R2HttpMetadata::default()
                },
                custom_metadata: BTreeMap::from([("author".to_owned(), "local".to_owned())]),
                checksum: Some(R2ChecksumAlgorithm::Sha256(source.checksums.sha256)),
                storage_class: R2StorageClass::InfrequentAccess,
                ssec: Some(ssec.clone()),
                ..R2PutOptions::default()
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(put.etag, hex::encode(source.checksums.md5));
    assert_eq!(put.storage_class, "InfrequentAccess");
    assert_eq!(put.ssec_key_md5.as_deref(), Some(ssec.md5_hex().as_str()));
    let R2GetResult::Body(download) = store
        .get(
            &locator,
            &key,
            Some(R2Range {
                offset: Some(6),
                length: Some(5),
                suffix: None,
            }),
            None,
            Some(&ssec),
        )
        .await
        .unwrap()
    else {
        panic!("expected local R2 body")
    };
    assert_eq!(
        download.body.collect().await.unwrap().into_bytes(),
        &body[6..11]
    );
    assert!(store.get(&locator, &key, None, None, None).await.is_err());

    let multipart_key = UserObjectKey::parse("multipart").unwrap();
    let version = uuid::Uuid::now_v7().hyphenated().to_string();
    let upload_id = store
        .create_multipart_upload(
            &locator,
            &multipart_key,
            &version,
            &R2MultipartCreateOptions::default(),
        )
        .await
        .unwrap();
    let part_path = fixture.config.path.parent().unwrap().join("r2-part");
    write_private(&part_path, b"multipart-local");
    let part_source = crate::R2PartSource {
        path: part_path,
        length: 15,
    };
    let part = store
        .upload_part(&locator, &multipart_key, &upload_id, 1, &part_source, None)
        .await
        .unwrap();
    assert_eq!(part.etag, hex::encode(md5::Md5::digest(b"multipart-local")));
    let part_digest = hex::decode(&part.etag).unwrap();
    let expected_complete_etag = format!("{}-1", hex::encode(md5::Md5::digest(part_digest)));
    let completed = store
        .complete_multipart_upload(&locator, &multipart_key, &upload_id, &[part], None)
        .await
        .unwrap();
    assert_eq!(completed.version, version);
    assert_eq!(completed.etag, expected_complete_etag);
    assert!(
        store
            .list_multipart_upload_ids(&locator, &multipart_key)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn local_root_marker_lock_capacity_corruption_and_relocation_fail_closed() {
    let fixture = Fixture::new();
    assert!(matches!(
        ObjectBackend::open_local(&fixture.config, fixture.platform_id, LIMIT),
        Err(error) if error.code() == ErrorCode::DataDirInUse
    ));
    let key = ObjectKey::new("system/security/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"verified")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let object = fixture.object_file(&key);
    let hardlink = object.with_file_name("evidence-hardlink");
    fs::hard_link(&object, &hardlink).unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::remove_file(hardlink).unwrap();

    let mut full = fixture.config.clone();
    full.path = fixture.config.path.parent().unwrap().join("capacity-root");
    full.free_space_hard_bytes = u64::MAX;
    let full_backend = ObjectBackend::open_local(&full, PlatformId::generate(), LIMIT).unwrap();
    assert_eq!(
        full_backend
            .put(
                &ObjectKey::new("system/full").unwrap(),
                ObjectSource::Bytes(Bytes::from_static(b"x")),
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Capacity
    );
    drop(full_backend);

    let Fixture {
        _temp,
        mut config,
        platform_id,
        backend,
    } = fixture;
    let fingerprint = backend.authority_sha256();
    drop(backend);
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());
    let moved = config.path.parent().unwrap().join("moved-objects");
    fs::rename(&config.path, &moved).unwrap();
    config.path = moved;
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert_eq!(reopened.authority_sha256(), fingerprint);
    drop(reopened);
    let evidence = config.path.join("unexpected-evidence");
    fs::write(&evidence, b"preserve-for-operator").unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(ObjectBackend::open_local(&config, platform_id, LIMIT).is_err());
    assert_eq!(fs::read(&evidence).unwrap(), b"preserve-for-operator");
    drop(_temp);
}

#[test]
fn local_root_rejects_symlinks_and_insecure_existing_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let config = LocalObjectStorageConfig {
        path: link,
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
        ..LocalObjectStorageConfig::default()
    };
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());

    let insecure = temp.path().join("insecure");
    fs::create_dir(&insecure).unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o755)).unwrap();
    let config = LocalObjectStorageConfig {
        path: insecure,
        free_space_soft_bytes: 1,
        free_space_hard_bytes: 1,
        ..LocalObjectStorageConfig::default()
    };
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());
}

#[tokio::test]
async fn stale_owned_partial_is_recovered_only_after_grace() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/recovery/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"value")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let parent = fixture.object_file(&key).parent().unwrap().to_owned();
    let partial = parent.join(format!(".partial-{}", uuid::Uuid::now_v7()));
    fs::write(&partial, b"owned-crash-remnant").unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&partial)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2))
        .unwrap();
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert!(partial.exists());
    reopened.recover().await.unwrap();
    assert!(!partial.exists());
    drop(reopened);
    drop(_temp);
}

#[tokio::test]
async fn multipart_restart_reconciliation_is_state_driven_and_retryable() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/restart-multipart").unwrap();
    let publishing = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    let part = fixture
        .backend
        .upload_part(
            &key,
            &publishing,
            1,
            ObjectSource::Bytes(Bytes::from_static(b"restart-safe")),
            None,
        )
        .await
        .unwrap();
    set_multipart_status(
        &fixture.config.path,
        &publishing,
        serde_json::json!({
            "kind": "publishing",
            "etag": "pending-publication"
        }),
    );
    let aborting = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    set_multipart_status(
        &fixture.config.path,
        &aborting,
        serde_json::json!({"kind": "aborting"}),
    );
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    reopened.recover().await.unwrap();
    assert!(!config.path.join("multipart").join(&aborting).exists());
    assert!(config.path.join("multipart").join(&publishing).exists());
    reopened
        .complete_multipart(&key, &publishing, &[part], None)
        .await
        .unwrap();
    assert_eq!(
        bytes(&reopened, &key, GetOptions::default()).await,
        Bytes::from_static(b"restart-safe")
    );
    drop(reopened);
    drop(_temp);
}

#[tokio::test]
async fn put_delete_and_fsync_faults_recover_without_torn_objects() {
    for (fault, published) in [
        (LocalFaultPoint::BeforeEnvelopeFsync, false),
        (LocalFaultPoint::AfterEnvelopeFsync, false),
        (LocalFaultPoint::BeforePublishRename, false),
        (LocalFaultPoint::AfterPublishRename, true),
    ] {
        let fixture = Fixture::new();
        let key = ObjectKey::new("system/fault/put").unwrap();
        fixture.backend.inject_local_fault(fault);
        assert_eq!(
            fixture
                .backend
                .put(
                    &key,
                    ObjectSource::Bytes(Bytes::from_static(b"atomic-body")),
                    options(PutMode::Replace),
                )
                .await
                .unwrap_err(),
            BackendError::Unavailable,
            "{fault:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
        let Fixture {
            _temp,
            config,
            platform_id,
            backend,
        } = fixture;
        drop(backend);
        let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
        reopened.recover().await.unwrap();
        if published {
            assert_eq!(
                bytes(&reopened, &key, GetOptions::default()).await,
                Bytes::from_static(b"atomic-body"),
                "{fault:?}"
            );
        } else {
            assert_eq!(
                reopened
                    .head(&key, HeadOptions::default())
                    .await
                    .unwrap_err(),
                BackendError::NotFound,
                "{fault:?}"
            );
        }
        drop(reopened);
        drop(_temp);
    }

    let fixture = Fixture::new();
    let key = ObjectKey::new("system/fault/delete").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"delete-me")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    fixture
        .backend
        .inject_local_fault(LocalFaultPoint::AfterDeleteUnlink);
    assert_eq!(
        fixture.backend.delete(&key).await.unwrap_err(),
        BackendError::Unavailable
    );
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    reopened.recover().await.unwrap();
    assert_eq!(
        reopened
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::NotFound
    );
    drop(reopened);
    drop(_temp);
}

#[tokio::test]
async fn multipart_commit_and_abort_faults_reconcile_from_durable_intent() {
    for fault in [
        LocalFaultPoint::MultipartIntentCommitted,
        LocalFaultPoint::MultipartBeforePublish,
        LocalFaultPoint::MultipartAfterPublish,
        LocalFaultPoint::MultipartBeforeRetire,
    ] {
        let fixture = Fixture::new();
        let key = ObjectKey::new("tenant/r2/fault-multipart").unwrap();
        let upload_id = fixture
            .backend
            .create_multipart(&key, ObjectMetadata::default(), None)
            .await
            .unwrap();
        let part = fixture
            .backend
            .upload_part(
                &key,
                &upload_id,
                1,
                ObjectSource::Bytes(Bytes::from_static(b"multipart-body")),
                None,
            )
            .await
            .unwrap();
        fixture.backend.inject_local_fault(fault);
        assert_eq!(
            fixture
                .backend
                .complete_multipart(&key, &upload_id, std::slice::from_ref(&part), None)
                .await
                .unwrap_err(),
            BackendError::Unavailable,
            "{fault:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
        let Fixture {
            _temp,
            config,
            platform_id,
            backend,
        } = fixture;
        drop(backend);
        let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
        reopened.recover().await.unwrap();
        if matches!(
            fault,
            LocalFaultPoint::MultipartIntentCommitted | LocalFaultPoint::MultipartBeforePublish
        ) {
            reopened
                .complete_multipart(&key, &upload_id, &[part], None)
                .await
                .unwrap();
        } else {
            assert!(!config.path.join("multipart").join(&upload_id).exists());
        }
        assert_eq!(
            bytes(&reopened, &key, GetOptions::default()).await,
            Bytes::from_static(b"multipart-body"),
            "{fault:?}"
        );
        drop(reopened);
        drop(_temp);
    }

    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/fault-abort").unwrap();
    let upload_id = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    fixture
        .backend
        .inject_local_fault(LocalFaultPoint::MultipartAbortIntent);
    assert_eq!(
        fixture
            .backend
            .abort_multipart(&key, &upload_id)
            .await
            .unwrap_err(),
        BackendError::Unavailable
    );
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    reopened.recover().await.unwrap();
    assert!(!config.path.join("multipart").join(upload_id).exists());
    drop(reopened);
    drop(_temp);
}

#[tokio::test]
async fn recovery_preserves_and_rejects_unowned_symlink_evidence() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/recovery/evidence").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"value")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let outside = fixture
        .config
        .path
        .parent()
        .unwrap()
        .join("outside-evidence");
    fs::write(&outside, b"preserve").unwrap();
    let partial = fixture
        .object_file(&key)
        .parent()
        .unwrap()
        .join(format!(".partial-{}", uuid::Uuid::now_v7()));
    std::os::unix::fs::symlink(&outside, &partial).unwrap();
    assert_eq!(
        fixture.backend.recover().await.unwrap_err(),
        BackendError::Corrupt
    );
    assert!(partial.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&outside).unwrap(), b"preserve");
}

fn set_multipart_status(root: &Path, upload_id: &str, status: serde_json::Value) {
    let path = root.join("multipart").join(upload_id).join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["status"] = status;
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn write_private(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                output.push(entry.path());
            }
        }
    }
    output
}
