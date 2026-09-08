use super::*;

pub(super) async fn create_product_set(
    storage: &PlatformStorage,
    objects: &open_compute_artifacts::R2ObjectStore,
    pins: &ResourcePins,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) -> (
    ProductBindings,
    open_compute_storage::DurableObjectMigrationPlan,
) {
    let kv = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::KvNamespace,
        "combined-kv",
        "p0-exit-create-kv",
        now_ms(),
    )
    .await;
    let kv_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::KvNamespace,
        "combined-kv-other",
        "p0-exit-create-kv-other",
        now_ms(),
    )
    .await;
    let r2 = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::R2Bucket,
        "combined-r2",
        "p0-exit-create-r2",
        now_ms(),
    )
    .await;
    let r2_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::R2Bucket,
        "combined-r2-other",
        "p0-exit-create-r2-other",
        now_ms(),
    )
    .await;
    let d1 = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1",
        "p0-exit-create-d1",
        now_ms(),
    )
    .await;
    let d1_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1-other",
        "p0-exit-create-d1-other",
        now_ms(),
    )
    .await;
    let d1_corrupt = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1-corrupt",
        "p0-exit-create-d1-corrupt",
        now_ms(),
    )
    .await;
    let do_repository = open_compute_storage::DurableObjectRepository::new(storage);
    let do_plan = open_compute_storage::DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "p0-exit-v1".to_owned(),
        new_sqlite_classes: vec!["AppObject".to_owned(), "OtherObject".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    do_repository
        .prepare_worker_migration(account, worker, &do_plan, 1_000_000)
        .unwrap();
    let objects = do_repository
        .namespace_for_worker_upload(account, worker, "AppObject", Some("p0-exit-v1"))
        .unwrap()
        .resource
        .id;
    let objects_other = do_repository
        .namespace_for_worker_upload(account, worker, "OtherObject", Some("p0-exit-v1"))
        .unwrap()
        .resource
        .id;
    (
        ProductBindings {
            kv,
            kv_other,
            r2,
            r2_other,
            d1,
            d1_other,
            d1_corrupt,
            objects,
            objects_other,
        },
        do_plan,
    )
}

pub(super) async fn apply_primary_d1_migration(
    stack: &GateStack,
    account: open_compute_core::AccountId,
    database: ResourceId,
) {
    let sql = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)";
    let migration = D1Migration {
        id: 1,
        name: "0001_notes.sql".into(),
        sha256: Sha256::digest(sql.as_bytes()).into(),
        sql: sql.into(),
    };
    let applied = stack
        .d1
        .apply_migrations(account, database, vec![migration.clone()], now_ms())
        .await
        .unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        stack
            .d1
            .apply_migrations(account, database, vec![migration], now_ms())
            .await
            .unwrap(),
        applied
    );
}

pub(super) async fn create_backup(router: &axum::Router, uri: &str, key: &str) -> String {
    let (status, body) = admin_json(router, "POST", uri, Value::Null, Some(key)).await;
    assert_v4_envelope(status, &body);
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["result"]["id"].as_str().unwrap().to_owned();
    let (replay_status, replay) = admin_json(router, "POST", uri, Value::Null, Some(key)).await;
    assert_v4_envelope(replay_status, &replay);
    assert_eq!(replay_status, StatusCode::OK, "{replay}");
    assert_eq!(replay["result"]["id"], id);
    id
}

pub(super) async fn restore_resource(
    router: &axum::Router,
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
    kind: BindingKind,
    uri: &str,
    name: &str,
    key: &str,
) -> ResourceId {
    let body = json!({"name": name});
    let (status, restored) = admin_json(router, "POST", uri, body.clone(), Some(key)).await;
    assert_v4_envelope(status, &restored);
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_public_id(restored["result"]["id"].as_str().unwrap());
    assert_eq!(restored["result"]["name"], name);
    let (replay_status, replay) = admin_json(router, "POST", uri, body, Some(key)).await;
    assert_v4_envelope(replay_status, &replay);
    assert_eq!(replay_status, StatusCode::OK, "{replay}");
    assert_eq!(replay["result"]["id"], restored["result"]["id"]);
    ResourceRepository::new(storage.db())
        .list(account, Some(kind))
        .unwrap()
        .into_iter()
        .find(|resource| resource.name == name)
        .unwrap()
        .id
}

pub(super) async fn alarm_status(
    stack: &GateStack,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    generation: u64,
) -> Value {
    let response = dispatch(
        &stack.transport,
        account,
        worker,
        version,
        generation,
        "/alarm-status",
    )
    .await;
    assert_eq!(
        response.status,
        200,
        "{}; runtime={:?}; diagnostics={:?}",
        response.body,
        stack.supervisor.snapshot(),
        stack.supervisor.last_diagnostics()
    );
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
pub(super) fn response_json(response: &support::DispatchResponse) -> Value {
    assert_ok(response);
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
pub(super) fn assert_ok(response: &support::DispatchResponse) {
    assert_eq!(response.status, 200, "{}", response.body);
}

#[track_caller]
pub(super) fn assert_snapshot(value: &Value, release: &str, kv: &str, d1: &str) {
    assert_eq!(value["release"], release);
    for key in ["kv", "r2", "d1", "durableObject"] {
        assert_eq!(value["facade"][key], true, "{key}: {value}");
    }
    for key in [
        "workers",
        "kv",
        "r2",
        "d1",
        "durableObjects",
        "websocket",
        "adversarialValues",
        "maliciousWorker",
    ] {
        assert_eq!(value["conformance"][key], true, "{key}: {value}");
    }
    assert_eq!(value["kv"]["text"], kv);
    assert_eq!(value["kv"]["json"], json!({"ok": true, "product": "kv"}));
    assert_eq!(value["kv"]["binary"], json!([1, 2]));
    assert_eq!(value["kv"]["stream"], "stream-value");
    assert_eq!(value["kv"]["isolated"], "isolated-kv");
    assert_eq!(value["r2"]["body"], "hello-r2");
    assert_eq!(value["r2"]["range"], "ell");
    assert_eq!(value["r2"]["size"], 8);
    assert_eq!(value["r2"]["custom"], "seed");
    assert_eq!(value["r2"]["contentType"], "text/plain");
    assert_eq!(value["r2"]["metadataReturnUndefined"], true);
    assert_eq!(value["r2"]["isolated"], "isolated-r2");
    assert_eq!(value["d1"]["first"], d1);
    assert_eq!(value["d1"]["sessionCount"], 3);
    assert_eq!(value["d1"]["isolated"], "isolated-d1");
    assert_eq!(value["durableObject"]["rpc"]["count"], 1);
    assert_eq!(value["durableObject"]["fetch"]["count"], 1);
    assert_eq!(value["durableObject"]["isolated"]["count"], 1);
    assert_eq!(value["durableObject"]["rpc"]["alarmConformance"], true);
}

pub(super) fn all_resources(bindings: ProductBindings) -> [ResourceId; 9] {
    [
        bindings.kv,
        bindings.kv_other,
        bindings.r2,
        bindings.r2_other,
        bindings.d1,
        bindings.d1_other,
        bindings.d1_corrupt,
        bindings.objects,
        bindings.objects_other,
    ]
}
