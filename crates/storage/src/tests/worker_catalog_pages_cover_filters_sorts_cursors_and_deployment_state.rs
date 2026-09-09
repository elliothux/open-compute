use super::*;

#[test]
fn worker_catalog_pages_cover_filters_sorts_cursors_and_deployment_state() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repository = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let mut workers = Vec::new();
    for (index, name) in ["alpha-worker", "beta-worker", "gamma-worker"]
        .into_iter()
        .enumerate()
    {
        workers.push(
            repository
                .create_worker(account, name, request, index as i64 + 1, 100)
                .unwrap()
                .0,
        );
    }
    let version = insert_ready(&repository, account, workers[0].id, [9; 32], request, 10);
    repository
        .promote(account, workers[0].id, version, None, request, 12)
        .unwrap();

    for (sort, direction) in [
        (CatalogSort::Name, CatalogDirection::Asc),
        (CatalogSort::Name, CatalogDirection::Desc),
        (CatalogSort::CreatedAt, CatalogDirection::Asc),
        (CatalogSort::UpdatedAt, CatalogDirection::Desc),
    ] {
        let first = repository
            .list_workers_page(account, None, None, sort, direction, None, 1)
            .unwrap();
        assert_eq!(first.items.len(), 1);
        let cursor = decode_catalog_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
        let second = repository
            .list_workers_page(account, None, None, sort, direction, Some(cursor), 10)
            .unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(second.next_cursor.is_none());
    }
    assert_eq!(
        repository
            .list_workers_page(
                account,
                Some("BETA"),
                Some(false),
                CatalogSort::Name,
                CatalogDirection::Asc,
                None,
                10,
            )
            .unwrap()
            .items[0]
            .name,
        "beta-worker"
    );
    let worker_id_search = workers[0].id.to_string();
    assert_eq!(
        repository
            .list_workers_page(
                account,
                Some(&worker_id_search),
                Some(true),
                CatalogSort::Name,
                CatalogDirection::Asc,
                None,
                10,
            )
            .unwrap()
            .items[0]
            .id,
        workers[0].id
    );
    let first = repository
        .list_workers_page(
            account,
            None,
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            1,
        )
        .unwrap();
    let cursor = decode_catalog_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
    assert!(
        repository
            .list_workers_page(
                account,
                None,
                None,
                CatalogSort::UpdatedAt,
                CatalogDirection::Asc,
                Some(cursor),
                10,
            )
            .is_err()
    );
}
