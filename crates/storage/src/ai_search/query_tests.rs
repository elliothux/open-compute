use super::super::StagedAiSearchChunk;
use super::{new_item, store};
use open_compute_core::ErrorCode;

#[test]
fn mutable_config_and_completed_sync_jobs_are_generation_fenced() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory.path().join("data.sqlite"));
    store
        .create_completed_job("sync-1", Some("manual sync"), 10)
        .unwrap();
    assert_eq!(store.get_job("sync-1").unwrap().unwrap().state, "completed");
    assert!(
        store
            .update_public_config(1, br#"{"max_num_results":5}"#, 11)
            .unwrap()
    );
    assert!(
        !store
            .update_public_config(1, br#"{"max_num_results":6}"#, 12)
            .unwrap()
    );
    assert_eq!(store.inspect().unwrap().config_generation, 2);
    assert_eq!(
        store.update_public_config(2, b"[]", 13).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        store
            .create_completed_job("bad", Some("line\nbreak"), 14)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    store.checkpoint(false).unwrap();
    store.checkpoint(true).unwrap();
}

#[test]
fn query_catalog_exposes_only_active_generation_with_bounded_pagination() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory.path().join("data.sqlite"));
    store
        .enqueue_item_generation("job-1", &new_item(b"{}"))
        .unwrap();
    let claim = store.claim_due_job(10, 100).unwrap().unwrap();
    let chunks = [
        StagedAiSearchChunk {
            chunk_id: "chunk-0",
            ordinal: 0,
            start_byte: 0,
            end_byte: 5,
            text: "hello",
            embedding_f32le: Some(&[0, 0, 128, 63]),
            vector_norm: Some(1.0),
            metadata_json: b"{}",
        },
        StagedAiSearchChunk {
            chunk_id: "chunk-1",
            ordinal: 1,
            start_byte: 6,
            end_byte: 11,
            text: "world",
            embedding_f32le: Some(&[0, 0, 128, 63]),
            vector_norm: Some(1.0),
            metadata_json: b"{}",
        },
        StagedAiSearchChunk {
            chunk_id: "chunk-2",
            ordinal: 2,
            start_byte: 12,
            end_byte: 23,
            text: "hello again",
            embedding_f32le: Some(&[0, 0, 128, 63]),
            vector_norm: Some(1.0),
            metadata_json: b"{}",
        },
    ];
    assert!(
        store
            .activate_item_generation(&claim, "item-1", 1, &chunks, 11)
            .unwrap()
    );

    assert_eq!(store.keyword_chunks("hello", false, 10).unwrap().len(), 2);
    assert_eq!(
        store.keyword_chunks_at(1, "ell", true, 10).unwrap().len(),
        2
    );
    let mut visited = Vec::new();
    store
        .scan_keyword_chunks_at(1, "hello", false, |chunk| {
            visited.push(chunk.id);
            Ok(false)
        })
        .unwrap();
    assert_eq!(visited.len(), 1);
    assert_eq!(store.active_chunk_context("item-1", 1, 1).unwrap().len(), 3);
    assert!(
        store
            .active_chunk_context("missing", 0, 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .active_chunk_context_at(1, "chunk-1", 4)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    let item = store.get_item("item-1").unwrap().unwrap();
    assert_eq!(item.chunks_count, 3);
    assert_eq!(
        store.get_item_by_key("document.txt").unwrap(),
        Some(item.clone())
    );
    assert_eq!(store.get_desired_item("item-1").unwrap(), Some(item));
    assert_eq!(store.list_items(0, 10).unwrap().1, 1);
    assert_eq!(store.get_job("job-1").unwrap().unwrap().state, "completed");
    assert_eq!(store.list_jobs(0, 10).unwrap().1, 1);
    assert_eq!(
        store.active_chunks(Some("item-1"), 1, 1).unwrap().0[0].id,
        "chunk-1"
    );
    assert_eq!(store.active_chunks_at(0, None, 0, 10).unwrap().1, 0);

    let mut active = Vec::new();
    store
        .scan_active_chunks_at(1, |chunk| {
            active.push(chunk.id);
            Ok(())
        })
        .unwrap();
    assert_eq!(active, ["chunk-0", "chunk-1", "chunk-2"]);
    let inspection = store.inspect().unwrap();
    assert!(
        store
            .active_fence_matches(inspection.active_index_generation, inspection.active_epoch)
            .unwrap()
    );
    assert!(
        !store
            .active_fence_matches(2, inspection.active_epoch)
            .unwrap()
    );
    assert!(!store.item_logs("item-1", 0, 10).unwrap().is_empty());
    assert!(!store.job_logs("job-1", 0, 10).unwrap().is_empty());
    assert_eq!(
        store.list_items(0, 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        store.keyword_chunks("", false, 10).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
}
