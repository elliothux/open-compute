use open_compute_artifacts::{AiSearchObjectRef, AiSearchObjectStore};
use open_compute_core::{BindingKind, RequestId, ResourceId};
use open_compute_storage::{
    AI_SEARCH_SCHEMA_VERSION, AiSearchCatalog, AiSearchInstanceStorageContract, AiSearchPaths,
    AiSearchStore, NewAiSearchItemGeneration, PlatformStorage, StagedAiSearchChunk,
    VECTORIZE_SCHEMA_VERSION, VectorMutationInput, VectorMutationKind, VectorizeEngine,
    VectorizeIndexRepository, VectorizePaths,
};
use open_compute_workers::{
    AiSearchInstanceResourceDriver, AiSearchInstanceSpec, AiSearchNamespaceResourceDriver,
    CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins,
    VectorizeIndexSpec, VectorizeResourceDriver,
};
use sha2::{Digest as _, Sha256};
use std::path::Path;

pub(super) struct P5SnapshotFixture {
    pub(super) vectorize_id: ResourceId,
    pub(super) ai_search_id: ResourceId,
    pub(super) object: AiSearchObjectRef,
    pub(super) object_key: String,
}

pub(super) async fn seed(
    storage: &PlatformStorage,
    objects: &AiSearchObjectStore,
    object_path: &Path,
) -> P5SnapshotFixture {
    let account = storage.identity().default_account_id;
    let vectorize_id = create_vectorize(storage, account);
    let vectorize_record = VectorizeIndexRepository::new(storage.db())
        .get(account, vectorize_id)
        .expect("Vectorize catalog");
    let vectorize_path = VectorizePaths::open(storage.data_dir().root())
        .expect("Vectorize paths")
        .resolve_storage_key(&vectorize_record.storage_key, account, vectorize_id)
        .expect("Vectorize path");
    let vectorize = VectorizeEngine::open(
        &vectorize_path,
        &vectorize_id.to_string(),
        32,
        "cosine",
        100,
        16 * 1024 * 1024,
        5_000,
    )
    .expect("Vectorize engine");
    vectorize
        .enqueue(
            VectorMutationKind::Upsert,
            &[VectorMutationInput {
                id: "snapshot-vector".to_owned(),
                namespace: Some("restore".to_owned()),
                values: Some(
                    std::iter::once(1.0)
                        .chain(std::iter::repeat_n(0.0, 31))
                        .collect(),
                ),
                metadata: Some(serde_json::json!({"source": "snapshot"})),
            }],
            1_010,
        )
        .expect("enqueue Vectorize fixture");
    vectorize
        .apply_next(1_011)
        .expect("apply Vectorize fixture");

    let namespace_id = create_namespace(storage, account);
    let model_contract_json = br#"{"dimensions":1,"metric":"cosine","tokenizer":"qwen3","tokenizerRevision":"snapshot","tokenizerArtifactSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#.to_vec();
    let model_contract_sha256 = Sha256::digest(&model_contract_json).into();
    let public_config_json = br#"{"chunk":true,"chunk_overlap":0,"chunk_size":64,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.to_vec();
    let ai_search_id = create_instance(
        storage,
        account,
        namespace_id,
        public_config_json.clone(),
        model_contract_json.clone(),
        model_contract_sha256,
    );
    let body = b"snapshot active AI Search chunk";
    super::write_mode(object_path, body, 0o600);
    let object = AiSearchObjectRef::new(
        account,
        ai_search_id,
        Sha256::digest(body).into(),
        body.len() as u64,
    )
    .expect("AI Search object identity");
    let object_key = objects
        .put_file(&object, object_path)
        .await
        .expect("upload AI Search source object");
    let record = AiSearchCatalog::new(storage.db())
        .get_instance(account, ai_search_id)
        .expect("AI Search catalog");
    let path = AiSearchPaths::open(storage.data_dir().root())
        .expect("AI Search paths")
        .resolve_storage_key(&record.storage_key, account, ai_search_id)
        .expect("AI Search path");
    let store = AiSearchStore::open(
        &path,
        &AiSearchInstanceStorageContract {
            resource_id: &ai_search_id.to_string(),
            model_contract_sha256,
            model_contract_json: &model_contract_json,
            public_config_json: &public_config_json,
            dimensions: 1,
            vector_enabled: true,
            keyword_enabled: true,
        },
        1_020,
    )
    .expect("AI Search store");
    store
        .enqueue_item_generation(
            "snapshot-job",
            &NewAiSearchItemGeneration {
                item_id: "snapshot-item",
                key: "snapshot.txt",
                source: "upload",
                generation: 1,
                index_generation: 1,
                object_key: &object_key,
                object_sha256: object.sha256,
                object_size: object.size,
                content_type: "text/plain",
                metadata_json: br#"{"source":"snapshot"}"#,
                now_ms: 1_021,
            },
        )
        .expect("enqueue AI Search fixture");
    let claim = store
        .claim_due_job(1_021, 100)
        .expect("claim AI Search fixture")
        .expect("due AI Search fixture");
    store
        .activate_item_generation(
            &claim,
            "snapshot-item",
            1,
            &[StagedAiSearchChunk {
                chunk_id: "snapshot-chunk",
                ordinal: 0,
                start_byte: 0,
                end_byte: u64::try_from(body.len()).expect("body length"),
                text: "snapshot active AI Search chunk",
                embedding_f32le: Some(&1.0_f32.to_le_bytes()),
                vector_norm: Some(1.0),
                metadata_json: br#"{"source":"snapshot"}"#,
            }],
            1_022,
        )
        .expect("activate AI Search fixture");

    P5SnapshotFixture {
        vectorize_id,
        ai_search_id,
        object,
        object_key,
    }
}

pub(super) fn assert_restored(storage: &PlatformStorage, fixture: &P5SnapshotFixture) {
    let account = storage.identity().default_account_id;
    let vectorize_record = VectorizeIndexRepository::new(storage.db())
        .get(account, fixture.vectorize_id)
        .expect("restored Vectorize catalog");
    let vectorize_path = VectorizePaths::open(storage.data_dir().root())
        .expect("restored Vectorize paths")
        .resolve_storage_key(&vectorize_record.storage_key, account, fixture.vectorize_id)
        .expect("restored Vectorize path");
    let vectorize = VectorizeEngine::open(
        &vectorize_path,
        &fixture.vectorize_id.to_string(),
        vectorize_record.dimensions,
        &vectorize_record.metric,
        vectorize_record.quota_vectors,
        vectorize_record.quota_bytes,
        5_000,
    )
    .expect("restored Vectorize engine");
    let mut rows = Vec::new();
    vectorize
        .scan_candidates(Some("restore"), None, |row| {
            rows.push(row);
            Ok(())
        })
        .expect("query restored Vectorize authority");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "snapshot-vector");
    assert_eq!(rows[0].values[0], 1.0);

    let record = AiSearchCatalog::new(storage.db())
        .get_instance(account, fixture.ai_search_id)
        .expect("restored AI Search catalog");
    let path = AiSearchPaths::open(storage.data_dir().root())
        .expect("restored AI Search paths")
        .resolve_storage_key(&record.storage_key, account, fixture.ai_search_id)
        .expect("restored AI Search path");
    let authority = open_compute_storage::inspect_ai_search_instance(
        &path,
        &fixture.ai_search_id.to_string(),
        record.model_contract_sha256,
        5_000,
    )
    .expect("restored AI Search authority");
    let store = AiSearchStore::open(
        &path,
        &AiSearchInstanceStorageContract {
            resource_id: &authority.resource_id,
            model_contract_sha256: authority.model_contract_sha256,
            model_contract_json: &authority.inspection.indexing_model_contract_json,
            public_config_json: &authority.inspection.indexing_public_config_json,
            dimensions: authority.dimensions,
            vector_enabled: authority.vector_enabled,
            keyword_enabled: authority.keyword_enabled,
        },
        record.resource.created_at_ms,
    )
    .expect("restored AI Search store");
    let chunks = store
        .keyword_chunks("snapshot", false, 10)
        .expect("query restored AI Search chunks");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].id, "snapshot-chunk");
    assert_eq!(chunks[0].text, "snapshot active AI Search chunk");
}

fn create_vectorize(
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
) -> ResourceId {
    let controller = ResourceController::new(
        storage,
        ResourcePins::new(),
        VectorizeResourceDriver::new(
            storage,
            VectorizeIndexSpec {
                dimensions: 32,
                metric: "cosine".to_owned(),
                quota_vectors: 100,
                quota_bytes: 16 * 1024 * 1024,
            },
            5_000,
        ),
    );
    created(controller.create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::VectorizeIndex,
        name: "snapshot-vectorize".to_owned(),
        idempotency_key: "snapshot-vectorize".to_owned(),
        driver_schema_version: VECTORIZE_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms: 1_010,
    }))
}

fn create_namespace(
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
) -> ResourceId {
    let controller = ResourceController::new(
        storage,
        ResourcePins::new(),
        AiSearchNamespaceResourceDriver::new(storage),
    );
    created(controller.create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::AiSearchNamespace,
        name: "snapshot-ai-search".to_owned(),
        idempotency_key: "snapshot-ai-search".to_owned(),
        driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms: 1_020,
    }))
}

fn create_instance(
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
    namespace: ResourceId,
    public_config_json: Vec<u8>,
    model_contract_json: Vec<u8>,
    model_contract_sha256: [u8; 32],
) -> ResourceId {
    let controller = ResourceController::new(
        storage,
        ResourcePins::new(),
        AiSearchInstanceResourceDriver::new(
            storage,
            AiSearchInstanceSpec {
                namespace_resource_id: namespace,
                instance_key: "snapshot".to_owned(),
                public_config_json,
                model_contract_json,
                model_contract_sha256,
                dimensions: 1,
                vector_enabled: true,
                keyword_enabled: true,
            },
            5_000,
        ),
    );
    created(controller.create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::AiSearchInstance,
        name: "snapshot-ai-search-instance".to_owned(),
        idempotency_key: "snapshot-ai-search-instance".to_owned(),
        driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms: 1_021,
    }))
}

fn created(outcome: Result<CreateResourceOutcome, open_compute_core::PlatformError>) -> ResourceId {
    match outcome.expect("create P5 snapshot resource") {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected P5 snapshot resource replay"),
    }
}
