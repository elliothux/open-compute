use super::*;

#[path = "catalog_tests.rs"]
mod catalog_tests;

#[path = "query_tests.rs"]
mod query_tests;

fn new_item<'a>(metadata: &'a [u8]) -> NewAiSearchItemGeneration<'a> {
    NewAiSearchItemGeneration {
        item_id: "item-1",
        key: "document.txt",
        source: "builtin",
        generation: 1,
        index_generation: 1,
        object_key: "ai-search/v1/a/i/objects/sha256/00/0011",
        object_sha256: [7; 32],
        object_size: 12,
        content_type: "text/plain",
        metadata_json: metadata,
        now_ms: 10,
    }
}

fn store(path: &Path) -> AiSearchStore {
    let model_contract_json =
        br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#;
    let model_contract_sha256 = Sha256::digest(model_contract_json).into();
    AiSearchStore::open(
        path,
        &AiSearchInstanceStorageContract {
            resource_id: "instance-1",
            model_contract_sha256,
            model_contract_json,
            public_config_json: br#"{"chunk":true,"chunk_overlap":10,"chunk_size":1,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#,
            dimensions: 1,
            vector_enabled: true,
            keyword_enabled: true,
        },
        1,
    )
    .expect("store")
}

#[test]
fn storage_key_resolves_before_instance_directory_exists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = AiSearchPaths::open(directory.path()).expect("paths");
    let account = open_compute_core::AccountId::generate();
    let resource = open_compute_core::ResourceId::generate();
    let path = paths
        .resolve_storage_key(
            &AiSearchPaths::storage_key(account, resource),
            account,
            resource,
        )
        .expect("resolve unpublished instance");
    assert_eq!(path, paths.instance_path(account, resource));
    assert!(path.parent().is_some_and(|parent| !parent.exists()));
}

#[test]
fn item_quota_rejects_the_mutation_atomically() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    {
        let mut connection = store.lock().expect("connection");
        let transaction = connection.transaction().expect("transaction");
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO items
                     (id, source, key, status, desired_generation, metadata_json,
                      created_at_ms, updated_at_ms)
                     VALUES (?1, 'builtin', ?2, 'queued', 1, X'7b7d', 1, 1)",
                )
                .expect("statement");
            for ordinal in 0..MAX_ITEMS_PER_INSTANCE {
                statement
                    .execute(rusqlite::params![
                        format!("seed-{ordinal}"),
                        format!("seed-{ordinal}.txt")
                    ])
                    .expect("seed item");
            }
        }
        transaction.commit().expect("commit seed");
    }
    let item = NewAiSearchItemGeneration {
        item_id: "overflow-item",
        key: "overflow.txt",
        source: "builtin",
        generation: 1,
        index_generation: 1,
        object_key: "ai-search/v1/a/i/objects/sha256/00/0022",
        object_sha256: [8; 32],
        object_size: 12,
        content_type: "text/plain",
        metadata_json: b"{}",
        now_ms: 20,
    };
    let error = store
        .enqueue_item_generation("overflow-job", &item)
        .expect_err("quota");
    assert_eq!(error.code(), ErrorCode::QuotaExceeded);
    assert!(store.get_job("overflow-job").expect("job lookup").is_none());
}

#[test]
fn claim_activation_is_atomic_and_fenced() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let claim = store.claim_due_job(10, 100).expect("claim").expect("due");
    let chunks = [StagedAiSearchChunk {
        chunk_id: "chunk-1",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .activate_item_generation(&claim, "item-1", 1, &chunks, 20)
            .expect("activate")
    );
    assert_eq!(
        store.item_state("item-1").expect("state"),
        Some(("completed".into(), Some(1)))
    );
    let keyword = store
        .keyword_chunks("\"hello\"", false, 10)
        .expect("parameterized FTS query");
    assert_eq!(keyword.len(), 1);
    assert_eq!(keyword[0].id, "chunk-1");
    assert!(
        !store
            .activate_item_generation(&claim, "item-1", 1, &chunks, 21)
            .expect("stale")
    );
}

#[test]
fn expired_claim_recovery_invalidates_old_token() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let old = store.claim_due_job(10, 5).expect("claim").expect("due");
    let new = store.claim_due_job(15, 5).expect("reclaim").expect("due");
    assert_ne!(old.claim_token, new.claim_token);
    assert_eq!(new.attempt, 2);
    assert!(
        !store
            .activate_item_generation(&old, "item-1", 1, &[], 16)
            .expect("old fence")
    );
}

#[test]
fn cancellation_fences_in_flight_result() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let claim = store.claim_due_job(10, 100).expect("claim").expect("due");
    assert!(store.request_cancel("job-1", 11).expect("cancel"));
    assert!(
        !store
            .activate_item_generation(&claim, "item-1", 1, &[], 12)
            .expect("fenced")
    );
    assert!(store.acknowledge_cancel(&claim, 13).expect("ack"));
    assert_eq!(
        store.item_state("item-1").expect("state"),
        Some(("skipped".into(), None))
    );
}

#[test]
fn permanent_failure_settles_job_and_item_authority() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let claim = store.claim_due_job(10, 100).expect("claim").expect("due");
    assert!(store.fail_claim(&claim, false, 0, 11).expect("fail"));
    assert_eq!(
        store.item_state("item-1").expect("state"),
        Some(("error".into(), None))
    );
    assert!(store.claim_due_job(12, 100).expect("claim").is_none());
}

#[test]
fn retryable_job_claim_renews_and_requeues_with_a_new_attempt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    assert_eq!(
        store.claim_due_job(10, 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    let first = store.claim_due_job(10, 100).unwrap().unwrap();
    assert!(store.renew_claim(&first, 20, 100).unwrap());
    assert!(store.fail_claim(&first, true, 50, 21).unwrap());
    assert!(store.claim_due_job(49, 100).unwrap().is_none());
    let second = store.claim_due_job(50, 100).unwrap().unwrap();
    assert_eq!(second.attempt, 2);
    assert_ne!(second.claim_token, first.claim_token);
    assert!(!store.renew_claim(&first, 51, 100).unwrap());
}

#[test]
fn readonly_instance_inspection_validates_identity_and_contract() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("data.sqlite");
    let store = store(&path);
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let expected: [u8; 32] = Sha256::digest(
        br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#,
    )
    .into();
    let authority = inspect_ai_search_instance(&path, "instance-1", expected, 100)
        .expect("readonly inspection");
    assert_eq!(authority.dimensions, 1);
    assert!(authority.vector_enabled);
    assert!(authority.keyword_enabled);
    assert_eq!(authority.inspection.item_count, 1);
    assert_eq!(authority.inspection.pending_job_count, 1);
    assert_eq!(
        inspect_ai_search_instance(&path, "instance-1", [0; 32], 100)
            .expect_err("contract mismatch")
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn identity_and_metadata_mismatch_fail_closed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("data.sqlite");
    let store = store(&path);
    let invalid = new_item(b"[]");
    assert_eq!(
        store
            .enqueue_item_generation("job-1", &invalid)
            .expect_err("metadata")
            .code(),
        ErrorCode::LimitInvalid
    );
    drop(store);
    let model_contract_json =
        br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#;
    assert_eq!(
        AiSearchStore::open(
            &path,
            &AiSearchInstanceStorageContract {
                resource_id: "instance-2",
                model_contract_sha256: Sha256::digest(model_contract_json).into(),
                model_contract_json,
                public_config_json: br#"{"chunk":true,"chunk_overlap":10,"chunk_size":1,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#,
                dimensions: 1,
                vector_enabled: true,
                keyword_enabled: true,
            },
            1,
        )
        .expect_err("identity")
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn snapshot_object_inventory_is_read_only_exact_and_identity_bound() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("data.sqlite");
    let store = store(&path);
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    drop(store);
    let references = inspect_ai_search_object_references(&path, "instance-1", 500).unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].object_key, new_item(b"{}").object_key);
    assert_eq!(references[0].object_sha256, [7; 32]);
    assert_eq!(references[0].object_size, 12);
    assert_eq!(
        inspect_ai_search_object_references(&path, "instance-2", 500)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn ingest_crash_reconcile_and_exact_gc_are_durable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    let item = new_item(b"{}");
    store
        .reserve_ingest_intent(
            "intent-1",
            item.item_id,
            item.object_key,
            item.object_sha256,
            item.object_size,
            10,
        )
        .expect("reserve");
    assert_eq!(store.reconcile_abandoned_ingests(10, 12).unwrap(), 1);
    let first = store
        .claim_due_object_gc(12, 5)
        .expect("claim")
        .expect("due");
    assert_eq!(first.object_key, item.object_key);
    assert_eq!(first.object_sha256, item.object_sha256);
    assert_eq!(first.object_size, item.object_size);
    assert!(store.renew_object_gc_claim(&first, 13, 10).unwrap());
    assert!(store.retry_object_gc_claim(&first, 20, 13).unwrap());
    assert!(store.claim_due_object_gc(19, 5).unwrap().is_none());
    let second = store
        .claim_due_object_gc(20, 5)
        .expect("reclaim")
        .expect("due");
    assert_eq!(second.attempt, 2);
    assert!(store.complete_object_gc_claim(&second).unwrap());
    assert_eq!(store.pending_object_gc_count().unwrap(), 0);
}

#[test]
fn uploaded_ingest_commit_and_instance_delete_preserve_gc_authority() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    let item = new_item(b"{}");
    store
        .reserve_ingest_intent(
            "intent-1",
            item.item_id,
            item.object_key,
            item.object_sha256,
            item.object_size,
            10,
        )
        .expect("reserve");
    assert!(
        store
            .mark_ingest_uploaded(
                "intent-1",
                item.object_key,
                item.object_sha256,
                item.object_size,
                11,
            )
            .expect("uploaded")
    );
    store
        .commit_uploaded_ingest("intent-1", "job-1", &item)
        .expect("commit");
    assert_eq!(store.get_job("job-1").unwrap().unwrap().state, "queued");
    assert_eq!(store.get_item("item-1").unwrap().unwrap().status, "queued");
    assert_eq!(store.prepare_instance_delete_and_enqueue_gc(12).unwrap(), 1);
    assert!(store.get_item("item-1").unwrap().is_none());
    assert_eq!(store.pending_object_gc_count().unwrap(), 1);
    let claim = store
        .claim_due_object_gc(12, 10)
        .expect("claim")
        .expect("gc retained in instance database");
    assert_eq!(claim.object_key, item.object_key);
}

#[test]
fn full_reindex_is_generation_fenced_and_survives_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("data.sqlite");
    let store = store(&path);
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let claim = store.claim_due_job(10, 100).unwrap().unwrap();
    let initial = [StagedAiSearchChunk {
        chunk_id: "chunk-initial",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .activate_item_generation(&claim, "item-1", 1, &initial, 11)
            .unwrap()
    );
    let model = br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#;
    let public = br#"{"chunk":true,"chunk_overlap":0,"chunk_size":1,"custom_metadata":[{"data_type":"text","field_name":"language"}],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#;
    let contract = AiSearchInstanceStorageContract {
        resource_id: "instance-1",
        model_contract_sha256: Sha256::digest(model).into(),
        model_contract_json: model,
        public_config_json: public,
        dimensions: 1,
        vector_enabled: true,
        keyword_enabled: true,
    };
    assert!(
        store
            .begin_full_reindex(1, &contract, "reindex", 20)
            .unwrap()
    );
    let pending = store.inspect().unwrap();
    assert_eq!(pending.config_generation, 2);
    assert_eq!(pending.active_index_generation, 1);
    assert_eq!(pending.pending_job_count, 1);
    drop(store);

    let reopened = AiSearchStore::open(&path, &contract, 1).expect("reopen");
    let claim = reopened.claim_due_job(20, 100).unwrap().unwrap();
    assert_eq!(claim.config_generation, 2);
    assert_eq!(claim.index_generation, 2);
    assert_eq!(claim.item.generation, 2);
    let replacement = [StagedAiSearchChunk {
        chunk_id: "chunk-reindexed",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        reopened
            .activate_item_generation(&claim, "item-1", 2, &replacement, 21)
            .unwrap()
    );
    let active = reopened.inspect().unwrap();
    assert_eq!(active.active_index_generation, 2);
    assert_eq!(active.pending_job_count, 0);
    assert_eq!(
        reopened.item_state("item-1").unwrap(),
        Some(("completed".into(), Some(2)))
    );
}

#[test]
fn staged_batches_resume_from_the_durable_next_ordinal() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let first = store.claim_due_job(10, 5).unwrap().unwrap();
    let chunk0 = [StagedAiSearchChunk {
        chunk_id: "chunk-0",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .stage_item_generation_batch(&first, 0, &chunk0, 11)
            .unwrap()
    );
    let resumed = store.claim_due_job(15, 100).unwrap().unwrap();
    assert_eq!(resumed.next_batch_ordinal, 1);
    let chunk1 = [StagedAiSearchChunk {
        chunk_id: "chunk-1",
        ordinal: 1,
        start_byte: 6,
        end_byte: 11,
        text: "world",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .stage_item_generation_batch(&resumed, 1, &chunk1, 16)
            .unwrap()
    );
    assert!(
        store
            .complete_staged_item_generation(&resumed, 2, 17)
            .unwrap()
    );
    assert_eq!(store.active_chunks(None, 0, 10).unwrap().0.len(), 2);
}

#[test]
fn staged_batches_enforce_cumulative_logical_bytes_atomically() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let claim = store.claim_due_job(10, 100).unwrap().unwrap();
    let first = [StagedAiSearchChunk {
        chunk_id: "chunk-0",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .stage_item_generation_batch_with_logical_limit(&claim, 0, &first, 11, 21)
            .unwrap()
    );
    let second = [StagedAiSearchChunk {
        chunk_id: "chunk-1",
        ordinal: 1,
        start_byte: 6,
        end_byte: 11,
        text: "world",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    let error = store
        .stage_item_generation_batch_with_logical_limit(&claim, 1, &second, 12, 21)
        .expect_err("second batch crosses the cumulative quota");
    assert_eq!(error.code(), ErrorCode::QuotaExceeded);
    assert_eq!(store.get_job("job-1").unwrap().unwrap().state, "claimed");
    assert_eq!(store.active_chunks(None, 0, 10).unwrap().1, 0);
}

#[test]
fn replacement_generation_quota_excludes_superseded_item_bytes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue initial");
    let initial_claim = store.claim_due_job(10, 100).unwrap().unwrap();
    let initial = [StagedAiSearchChunk {
        chunk_id: "old",
        ordinal: 0,
        start_byte: 0,
        end_byte: 14,
        text: "old-generation",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .activate_item_generation(&initial_claim, "item-1", 1, &initial, 11)
            .unwrap()
    );
    let replacement = NewAiSearchItemGeneration {
        generation: 2,
        object_key: "ai-search/v1/a/i/objects/sha256/00/0022",
        object_sha256: [8; 32],
        now_ms: 20,
        ..new_item(b"{}")
    };
    store
        .enqueue_item_generation("job-2", &replacement)
        .expect("enqueue replacement");
    let replacement_claim = store.claim_due_job(20, 100).unwrap().unwrap();
    let staged = [StagedAiSearchChunk {
        chunk_id: "new",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .stage_item_generation_batch_with_logical_limit(&replacement_claim, 0, &staged, 21, 11,)
            .expect("superseded bytes are excluded")
    );
}

#[test]
fn failed_full_reindex_restores_old_contract_and_active_chunks() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("data.sqlite");
    let store = store(&path);
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let initial_claim = store.claim_due_job(10, 100).unwrap().unwrap();
    let initial = [StagedAiSearchChunk {
        chunk_id: "chunk-active",
        ordinal: 0,
        start_byte: 0,
        end_byte: 5,
        text: "hello",
        embedding_f32le: Some(&[0, 0, 128, 63]),
        vector_norm: Some(1.0),
        metadata_json: b"{}",
    }];
    assert!(
        store
            .activate_item_generation(&initial_claim, "item-1", 1, &initial, 11)
            .unwrap()
    );
    let replacement_model = br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev2","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#;
    let replacement_digest: [u8; 32] = Sha256::digest(replacement_model).into();
    let replacement_public = br#"{"chunk":true,"chunk_overlap":0,"chunk_size":1,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#;
    assert!(
        store
            .begin_full_reindex(
                1,
                &AiSearchInstanceStorageContract {
                    resource_id: "instance-1",
                    model_contract_sha256: replacement_digest,
                    model_contract_json: replacement_model,
                    public_config_json: replacement_public,
                    dimensions: 1,
                    vector_enabled: true,
                    keyword_enabled: true,
                },
                "replace",
                20,
            )
            .unwrap()
    );
    let replacement_claim = store.claim_due_job(20, 100).unwrap().unwrap();
    assert!(store.fail_claim(&replacement_claim, false, 0, 21).unwrap());
    let inspection = store.inspect().unwrap();
    assert!(!inspection.reindex_pending);
    assert_eq!(inspection.active_index_generation, 1);
    assert_eq!(
        store.active_chunks(None, 0, 10).unwrap().0[0].id,
        "chunk-active"
    );
    drop(store);
    let authority = inspect_ai_search_instance(&path, "instance-1", replacement_digest, 100)
        .expect("failed catalog digest remains recoverable");
    assert_ne!(authority.model_contract_sha256, replacement_digest);
}

#[test]
fn failed_full_reindex_preserves_non_null_desired_generation_without_active_content() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .expect("enqueue");
    let replacement_model = br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"rev2","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#;
    assert!(
        store
            .begin_full_reindex(
                1,
                &AiSearchInstanceStorageContract {
                    resource_id: "instance-1",
                    model_contract_sha256: Sha256::digest(replacement_model).into(),
                    model_contract_json: replacement_model,
                    public_config_json: br#"{"chunk":true,"chunk_overlap":0,"chunk_size":1,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#,
                    dimensions: 1,
                    vector_enabled: true,
                    keyword_enabled: true,
                },
                "replace-empty",
                20,
            )
            .unwrap()
    );
    let replacement_claim = store.claim_due_job(20, 100).unwrap().unwrap();
    assert!(store.fail_claim(&replacement_claim, false, 0, 21).unwrap());
    assert!(!store.inspect().unwrap().reindex_pending);
    assert_eq!(
        store.item_state("item-1").unwrap(),
        Some(("error".into(), None))
    );
    assert_eq!(
        store
            .get_item("item-1")
            .unwrap()
            .unwrap()
            .desired_generation,
        2
    );
}
