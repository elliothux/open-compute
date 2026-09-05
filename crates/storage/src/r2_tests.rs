use super::*;
use crate::{
    CatalogDirection, CatalogSort, PlatformStorage, R2MultipartPartRecord, R2MultipartRepository,
    R2MultipartState, R2MultipartUploadRecord, R2ObjectListEntry, R2ObjectMutationKind,
    R2ObjectRecord, R2ObjectRepository, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, decode_catalog_cursor,
};
use open_compute_core::config::DataConfig;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, RequestId, ResourceId, ResourceState, SystemClock,
};

fn fixture() -> (tempfile::TempDir, PlatformStorage, ResourceRecord) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
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
    .unwrap();
    let resource_id = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(b"r2-catalog-test");
    let reserved = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: storage.identity().default_account_id,
                kind: BindingKind::R2Bucket,
                name: "images",
                idempotency_key: "r2-catalog-test",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reserved else {
        unreachable!()
    };
    (temp, storage, resource)
}

fn ready_bucket(storage: &PlatformStorage, name: &str, now_ms: i64) -> ResourceRecord {
    let account_id = storage.identity().default_account_id;
    let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id,
                kind: BindingKind::R2Bucket,
                name,
                idempotency_key: name,
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms,
                expires_at_ms: now_ms + 1_000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        unreachable!()
    };
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            512 * 1024 * 1024,
            &[1; 32],
        )
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource.id, now_ms + 1)
        .unwrap();
    resource
}

#[test]
fn locator_is_immutable_scoped_and_not_serialized() {
    let (_temp, storage, resource) = fixture();
    let repo = R2BucketRepository::new(storage.db());
    let prefix = format!("tenant/r2/v1/{}/", resource.id);
    let authority = [1_u8; 32];
    let record = repo
        .ensure_bucket(&resource, &prefix, 512 * 1024 * 1024, &authority)
        .unwrap();
    assert_eq!(record.physical_prefix, prefix);
    assert_eq!(repo.get(resource.account_id, resource.id).unwrap(), record);
    assert_eq!(
        repo.list(resource.account_id).unwrap(),
        vec![record.clone()]
    );
    assert_eq!(repo.list_all().unwrap(), vec![record.clone()]);
    let serialized = serde_json::to_string(&record).unwrap();
    assert!(!serialized.contains("tenant/r2"));
    assert!(!serialized.contains("providerConfig"));
    assert_eq!(
        repo.get(AccountId::generate(), resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.ensure_bucket(&resource, "bad", 1, &authority)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn locator_retires_only_after_deletion_fence_and_tombstone() {
    let (_temp, storage, resource) = fixture();
    let buckets = R2BucketRepository::new(storage.db());
    let resources = ResourceRepository::new(storage.db());
    buckets
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    resources.mark_ready(resource.id, 11).unwrap();
    resources
        .begin_delete(resource.account_id, resource.id, 12)
        .unwrap();
    assert!(
        resources
            .mark_tombstoned(resource.account_id, resource.id, RequestId::generate(), 13)
            .is_err()
    );
    buckets.mark_delete_started(resource.id, 14).unwrap();
    resources
        .mark_tombstoned(resource.account_id, resource.id, RequestId::generate(), 15)
        .unwrap();
    assert_eq!(
        buckets
            .get(resource.account_id, resource.id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}

#[test]
fn logical_object_list_preserves_cloudflare_keys_and_cursor_order() {
    let (_temp, storage, resource) = fixture();
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    let repo = R2ObjectRepository::new(storage.db());
    for (index, key) in ["", ".", "..", "a", "a/1", "a/2", "a/b/1", "b", "nul\0key"]
        .into_iter()
        .enumerate()
    {
        let version = uuid::Uuid::now_v7().hyphenated().to_string();
        repo.begin_put(
            &R2ObjectRecord {
                resource_id: resource.id,
                account_id: resource.account_id,
                object_key: key.to_owned(),
                object_version: version.clone(),
                ssec_key_md5: None,
                ssec_envelope: None,
            },
            20 + i64::try_from(index).unwrap(),
        )
        .unwrap();
        repo.finish_put(
            resource.account_id,
            resource.id,
            key,
            &version,
            40 + i64::try_from(index).unwrap(),
        )
        .unwrap();
    }

    let first = repo
        .list(resource.account_id, resource.id, "", None, None, 3)
        .unwrap();
    assert_eq!(
        first.entries.iter().map(list_entry_key).collect::<Vec<_>>(),
        ["", ".", ".."]
    );
    assert_eq!(first.next_after.as_deref(), Some(".."));
    let second = repo
        .list(
            resource.account_id,
            resource.id,
            "",
            None,
            first.next_after.as_deref(),
            3,
        )
        .unwrap();
    assert_eq!(
        second
            .entries
            .iter()
            .map(list_entry_key)
            .collect::<Vec<_>>(),
        ["a", "a/1", "a/2"]
    );

    let grouped = repo
        .list(resource.account_id, resource.id, "a/", Some("/"), None, 10)
        .unwrap();
    assert_eq!(
        grouped
            .entries
            .iter()
            .map(list_entry_key)
            .collect::<Vec<_>>(),
        ["a/1", "a/2", "a/b/"]
    );
    assert!(matches!(
        grouped.entries.last(),
        Some(R2ObjectListEntry::DelimitedPrefix(prefix)) if prefix == "a/b/"
    ));
    assert_eq!(
        repo.list(resource.account_id, resource.id, "nul\0", None, None, 10,)
            .unwrap()
            .entries
            .iter()
            .map(list_entry_key)
            .collect::<Vec<_>>(),
        ["nul\0key"]
    );
}

fn list_entry_key(entry: &R2ObjectListEntry) -> &str {
    match entry {
        R2ObjectListEntry::Object(object) => &object.object_key,
        R2ObjectListEntry::DelimitedPrefix(prefix) => prefix,
    }
}

#[test]
fn multipart_authority_is_account_scoped_and_fail_closed_on_races() {
    let (_temp, storage, resource) = fixture();
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    let repo = R2MultipartRepository::new(storage.db());
    let upload_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let record = R2MultipartUploadRecord {
        upload_id: upload_id.clone(),
        resource_id: resource.id,
        account_id: resource.account_id,
        object_key: "object".to_owned(),
        provider_upload_id: None,
        storage_class: "Standard".to_owned(),
        http_metadata: "{}".to_owned(),
        custom_metadata: "{}".to_owned(),
        ssec_key_md5: None,
        ssec_envelope: None,
        object_version: uuid::Uuid::now_v7().hyphenated().to_string(),
        completion_manifest: None,
        completed_metadata: None,
        state: R2MultipartState::Initiating,
    };
    repo.insert_initiating(&record, 20).unwrap();
    repo.record_provider_id(resource.account_id, resource.id, &upload_id, "provider", 21)
        .unwrap();
    repo.promote_open(resource.account_id, resource.id, &upload_id, 22)
        .unwrap();
    assert_eq!(
        repo.get(resource.account_id, resource.id, &upload_id)
            .unwrap()
            .unwrap()
            .provider_upload_id
            .as_deref(),
        Some("provider")
    );
    assert!(
        repo.get(AccountId::generate(), resource.id, &upload_id)
            .unwrap()
            .is_none()
    );
    repo.upsert_part(
        resource.account_id,
        resource.id,
        &upload_id,
        "object",
        &R2MultipartPartRecord {
            part_number: 1,
            etag: "etag".to_owned(),
            size: 9,
        },
        23,
    )
    .unwrap();
    assert_eq!(repo.list_parts(&upload_id).unwrap()[0].etag, "etag");
    let completing = repo
        .begin_complete(
            resource.account_id,
            resource.id,
            &upload_id,
            "object",
            r#"[{"partNumber":1,"etag":"etag"}]"#,
            24,
        )
        .unwrap();
    assert_eq!(
        completing.completion_manifest.as_deref(),
        Some(r#"[{"partNumber":1,"etag":"etag"}]"#)
    );
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "UPDATE r2_multipart_uploads SET completion_manifest = '{}' WHERE upload_id = ?1",
                [&upload_id],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.get(resource.account_id, resource.id, &upload_id)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute(
                "UPDATE r2_multipart_uploads SET completion_manifest = ?1 WHERE upload_id = ?2",
                [r#"[{"partNumber":1,"etag":"etag"}]"#, &upload_id],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.begin_abort(resource.account_id, resource.id, &upload_id, "object", 25)
            .unwrap_err()
            .code(),
        ErrorCode::R2MultipartInvalid
    );
    assert!(
        repo.revert_complete(resource.account_id, resource.id, &upload_id, "object", 26)
            .unwrap()
            .completion_manifest
            .is_none()
    );
    repo.begin_abort(resource.account_id, resource.id, &upload_id, "object", 27)
        .unwrap();
    assert_eq!(
        repo.begin_complete(
            resource.account_id,
            resource.id,
            &upload_id,
            "object",
            r#"[{"partNumber":1,"etag":"etag"}]"#,
            28,
        )
        .unwrap_err()
        .code(),
        ErrorCode::R2MultipartInvalid
    );
    repo.finish_abort(resource.account_id, resource.id, &upload_id, "object", 29)
        .unwrap();
    assert_eq!(
        repo.get(resource.account_id, resource.id, &upload_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Aborted
    );
    assert_eq!(
        repo.upsert_part(
            resource.account_id,
            resource.id,
            &upload_id,
            "object",
            &R2MultipartPartRecord {
                part_number: 2,
                etag: "other".to_owned(),
                size: 1,
            },
            26,
        )
        .unwrap_err()
        .code(),
        ErrorCode::R2MultipartInvalid
    );
}

#[test]
fn multipart_unknown_create_is_retained_until_scoped_abort_finishes() {
    let (_temp, storage, resource) = fixture();
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    let repo = R2MultipartRepository::new(storage.db());
    let upload_id = uuid::Uuid::now_v7().hyphenated().to_string();
    repo.insert_initiating(
        &R2MultipartUploadRecord {
            upload_id: upload_id.clone(),
            resource_id: resource.id,
            account_id: resource.account_id,
            object_key: "lost".to_owned(),
            provider_upload_id: None,
            storage_class: "Standard".to_owned(),
            http_metadata: "{}".to_owned(),
            custom_metadata: "{}".to_owned(),
            ssec_key_md5: None,
            ssec_envelope: None,
            object_version: uuid::Uuid::now_v7().hyphenated().to_string(),
            completion_manifest: None,
            completed_metadata: None,
            state: R2MultipartState::Initiating,
        },
        30,
    )
    .unwrap();
    repo.mark_create_unknown(resource.account_id, resource.id, &upload_id, 31)
        .unwrap();
    let retained = repo.list_for_resource(resource.id).unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].state, R2MultipartState::CreateUnknown);
    let aborting = repo
        .claim_unknown_for_abort(
            resource.account_id,
            resource.id,
            &upload_id,
            "provider-lost",
            32,
        )
        .unwrap();
    assert_eq!(aborting.state, R2MultipartState::Aborting);
    let aborted = repo
        .finish_abort(resource.account_id, resource.id, &upload_id, "lost", 33)
        .unwrap();
    assert_eq!(aborted.state, R2MultipartState::Aborted);
}

#[test]
fn object_authority_publishes_and_deletes_only_through_durable_intents() {
    let (_temp, storage, resource) = fixture();
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    let repo = R2ObjectRepository::new(storage.db());
    let version = uuid::Uuid::now_v7().hyphenated().to_string();
    let record = R2ObjectRecord {
        resource_id: resource.id,
        account_id: resource.account_id,
        object_key: "secret.bin".to_owned(),
        object_version: version.clone(),
        ssec_key_md5: None,
        ssec_envelope: None,
    };
    repo.begin_put(&record, 40).unwrap();
    let pending = repo
        .get_mutation(resource.account_id, resource.id, "secret.bin")
        .unwrap()
        .unwrap();
    assert_eq!(pending.kind, R2ObjectMutationKind::Put);
    let record_debug = format!("{record:?}");
    assert!(record_debug.contains("secret.bin"));
    assert!(record_debug.contains("ssec_envelope: None"));
    let pending_debug = format!("{pending:?}");
    assert!(pending_debug.contains("kind: Put"));
    assert!(pending_debug.contains("pending_ssec_envelope: None"));
    assert!(
        repo.get(resource.account_id, resource.id, "secret.bin")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repo.finish_put(resource.account_id, resource.id, "secret.bin", &version, 41,)
            .unwrap(),
        record
    );
    repo.begin_delete(
        resource.account_id,
        resource.id,
        &["secret.bin".to_owned()],
        42,
    )
    .unwrap();
    assert_eq!(
        repo.get_mutation(resource.account_id, resource.id, "secret.bin")
            .unwrap()
            .unwrap()
            .kind,
        R2ObjectMutationKind::Delete
    );
    repo.finish_delete(resource.account_id, resource.id, &["secret.bin".to_owned()])
        .unwrap();
    assert!(
        repo.get(resource.account_id, resource.id, "secret.bin")
            .unwrap()
            .is_none()
    );
}

#[test]
fn object_authority_rejects_invalid_records_and_stale_intents() {
    let (_temp, storage, resource) = fixture();
    R2BucketRepository::new(storage.db())
        .ensure_bucket(
            &resource,
            &format!("tenant/r2/v1/{}/", resource.id),
            1024,
            &[1_u8; 32],
        )
        .unwrap();
    let repo = R2ObjectRepository::new(storage.db());
    let base = R2ObjectRecord {
        resource_id: resource.id,
        account_id: resource.account_id,
        object_key: "object".to_owned(),
        object_version: "version".to_owned(),
        ssec_key_md5: None,
        ssec_envelope: None,
    };
    for invalid in [
        R2ObjectRecord {
            object_version: String::new(),
            ..base.clone()
        },
        R2ObjectRecord {
            ssec_key_md5: Some("md5".to_owned()),
            ..base.clone()
        },
        R2ObjectRecord {
            ssec_key_md5: Some("md5".to_owned()),
            ssec_envelope: Some("not-json".to_owned()),
            ..base.clone()
        },
    ] {
        assert_eq!(
            repo.begin_put(&invalid, 1).unwrap_err().code(),
            ErrorCode::ResourceInvariantViolation
        );
    }

    assert_eq!(
        repo.begin_delete(resource.account_id, resource.id, &["missing".to_owned()], 2,)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    repo.begin_put(&base, 3).unwrap();
    assert_eq!(
        repo.finish_put(
            resource.account_id,
            resource.id,
            &base.object_key,
            "wrong-version",
            4,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
    repo.cancel_put(resource.account_id, resource.id, &base.object_key)
        .unwrap();
    assert_eq!(
        repo.cancel_put(resource.account_id, resource.id, &base.object_key)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn bucket_catalog_pages_filter_sort_and_bind_cursors() {
    let (_temp, storage, initial) = fixture();
    let repository = R2BucketRepository::new(storage.db());
    repository
        .ensure_bucket(
            &initial,
            &format!("tenant/r2/v1/{}/", initial.id),
            512 * 1024 * 1024,
            &[1; 32],
        )
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(initial.id, 11)
        .unwrap();
    ready_bucket(&storage, "alpha-images", 20);
    ready_bucket(&storage, "beta-images", 30);

    for (sort, direction) in [
        (CatalogSort::Name, CatalogDirection::Asc),
        (CatalogSort::Name, CatalogDirection::Desc),
        (CatalogSort::CreatedAt, CatalogDirection::Asc),
        (CatalogSort::UpdatedAt, CatalogDirection::Desc),
    ] {
        let first = repository
            .list_page(initial.account_id, None, None, sort, direction, None, 1)
            .unwrap();
        assert_eq!(first.items.len(), 1);
        let cursor = decode_catalog_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
        let rest = repository
            .list_page(
                initial.account_id,
                None,
                Some(ResourceState::Ready),
                sort,
                direction,
                Some(cursor),
                10,
            )
            .unwrap();
        assert_eq!(rest.items.len(), 2);
        assert!(rest.next_cursor.is_none());
    }

    assert_eq!(
        repository
            .list_page(
                initial.account_id,
                Some("BETA"),
                None,
                CatalogSort::Name,
                CatalogDirection::Asc,
                None,
                10,
            )
            .unwrap()
            .items[0]
            .resource
            .name,
        "beta-images"
    );
    assert_eq!(
        repository
            .list_page(
                initial.account_id,
                Some(&initial.id.to_string()),
                None,
                CatalogSort::Name,
                CatalogDirection::Asc,
                None,
                10,
            )
            .unwrap()
            .items[0]
            .resource
            .id,
        initial.id
    );

    let first = repository
        .list_page(
            initial.account_id,
            None,
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            1,
        )
        .unwrap();
    let cursor = decode_catalog_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
    assert_eq!(
        repository
            .list_page(
                initial.account_id,
                None,
                None,
                CatalogSort::UpdatedAt,
                CatalogDirection::Asc,
                Some(cursor),
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
}
