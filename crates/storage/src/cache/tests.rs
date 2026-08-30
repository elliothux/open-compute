use super::*;
use open_compute_core::{AccountId, ErrorCode, ResponseCacheConfig, WorkerId};
use std::collections::BTreeMap;
use tempfile::TempDir;

fn manager() -> (TempDir, CacheManager, AccountId, WorkerId) {
    let temp = TempDir::new().unwrap();
    crate::fs::create_root_first_run(temp.path().join("data").as_path()).unwrap();
    crate::fs::create_dir_secure(&temp.path().join("data/cache")).unwrap();
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let manager =
        CacheManager::open(&temp.path().join("data"), ResponseCacheConfig::default()).unwrap();
    (temp, manager, account, worker)
}

fn identity(account: AccountId, worker: WorkerId, surface: CacheSurface) -> CacheIdentity {
    CacheIdentity {
        account_id: account,
        worker_id: worker,
        surface,
        entrypoint: (surface == CacheSurface::Automatic).then(|| "default".to_owned()),
        version_scope: if surface == CacheSurface::Automatic {
            open_compute_core::DeploymentId::generate().to_string()
        } else {
            "shared".to_owned()
        },
        cache_name: (surface == CacheSurface::CacheApiNamed).then(|| "pages".to_owned()),
        canonical_url: "https://example.com/path?a=1&a=2".to_owned(),
        method: CacheMethod::Get,
    }
}

fn response(generation: u64, fresh: i64, swr: i64, sie: i64, tag: &str) -> CacheStoredResponse {
    CacheStoredResponse {
        status: 200,
        headers: vec![CacheHeader {
            name: "content-type".to_owned(),
            value: "text/plain".to_owned(),
        }],
        body: CacheBodyRef {
            sha256: "11".repeat(32),
            size: 4,
        },
        vary: vec!["accept-language".to_owned()],
        tags: vec![tag.to_owned()],
        fresh_until_ms: fresh,
        stale_while_revalidate_until_ms: swr,
        stale_if_error_until_ms: sie,
        generation,
    }
}

#[test]
fn default_named_worker_and_vary_identities_do_not_cross() {
    let (_temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let mut headers = BTreeMap::from([("accept-language".to_owned(), "en".to_owned())]);
    for (index, surface) in [CacheSurface::CacheApiDefault, CacheSurface::CacheApiNamed]
        .into_iter()
        .enumerate()
    {
        let key = identity(account, worker, surface);
        let fence = engine.prepare_put(&key).unwrap();
        engine
            .put(&CachePut {
                identity: key.clone(),
                request_headers: headers.clone(),
                response: response(index as u64 + 1, 1_000, 1_000, 1_000, "news"),
                expected_fence_generation: fence,
                refresh_token: None,
                now_ms: 10,
            })
            .unwrap();
        assert_eq!(
            engine.lookup(&key, &headers, 11).unwrap().status,
            CacheLookupStatus::Hit
        );
    }
    headers.insert("accept-language".to_owned(), "fr".to_owned());
    assert_eq!(
        engine
            .lookup(
                &identity(account, worker, CacheSurface::CacheApiDefault),
                &headers,
                11
            )
            .unwrap()
            .status,
        CacheLookupStatus::Miss
    );
    let other_worker = WorkerId::generate();
    let other = manager.engine(account, other_worker, 1).unwrap();
    assert_eq!(
        other
            .lookup(
                &identity(account, other_worker, CacheSurface::CacheApiDefault),
                &headers,
                11
            )
            .unwrap()
            .status,
        CacheLookupStatus::Miss
    );
    let other_account = AccountId::generate();
    let other = manager.engine(other_account, worker, 1).unwrap();
    assert_eq!(
        other
            .lookup(
                &identity(other_account, worker, CacheSurface::CacheApiDefault),
                &headers,
                11
            )
            .unwrap()
            .status,
        CacheLookupStatus::Miss
    );
}

#[test]
fn automatic_cache_isolates_deployments_unless_the_key_is_explicitly_shared() {
    let (_temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let headers = BTreeMap::from([("accept-language".to_owned(), "en".to_owned())]);
    let deployment_a = identity(account, worker, CacheSurface::Automatic);
    let mut deployment_b = deployment_a.clone();
    deployment_b.version_scope = open_compute_core::DeploymentId::generate().to_string();
    let fence = engine.prepare_put(&deployment_a).unwrap();
    engine
        .put(&CachePut {
            identity: deployment_a.clone(),
            request_headers: headers.clone(),
            response: response(1, 1_000, 1_000, 1_000, "release"),
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 10,
        })
        .unwrap();
    assert_eq!(
        engine.lookup(&deployment_a, &headers, 11).unwrap().status,
        CacheLookupStatus::Hit
    );
    assert_eq!(
        engine.lookup(&deployment_b, &headers, 11).unwrap().status,
        CacheLookupStatus::Miss
    );

    let mut shared = deployment_a;
    shared.version_scope = "shared".to_owned();
    let fence = engine.prepare_put(&shared).unwrap();
    engine
        .put(&CachePut {
            identity: shared.clone(),
            request_headers: headers.clone(),
            response: response(1, 1_000, 1_000, 1_000, "release"),
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 10,
        })
        .unwrap();
    assert_eq!(
        engine.lookup(&shared, &headers, 11).unwrap().status,
        CacheLookupStatus::Hit
    );
}

#[test]
fn stale_refresh_is_single_owner_and_purge_fences_late_commit() {
    let (_temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let key = identity(account, worker, CacheSurface::Automatic);
    let headers = BTreeMap::from([("accept-language".to_owned(), "en".to_owned())]);
    let fence = engine.prepare_put(&key).unwrap();
    engine
        .put(&CachePut {
            identity: key.clone(),
            request_headers: headers.clone(),
            response: response(1, 20, 200, 300, "news"),
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 10,
        })
        .unwrap();
    let first = engine.lookup(&key, &headers, 30).unwrap();
    let second = engine.lookup(&key, &headers, 30).unwrap();
    assert_eq!(first.status, CacheLookupStatus::Updating);
    assert_eq!(second.status, CacheLookupStatus::Stale);
    assert!(first.refresh_token.is_some());
    assert_eq!(
        engine
            .purge(
                &CachePurge {
                    tags: vec!["news".to_owned()],
                    path_prefixes: Vec::new(),
                    purge_everything: false
                },
                35
            )
            .unwrap(),
        1
    );
    let error = engine
        .put(&CachePut {
            identity: key,
            request_headers: headers,
            response: response(2, 100, 200, 300, "news"),
            expected_fence_generation: fence,
            refresh_token: first.refresh_token,
            now_ms: 40,
        })
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CacheResultUnknown);
}

#[test]
fn corrupt_identity_and_path_are_rejected_and_references_survive_reopen() {
    let (temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let key = identity(account, worker, CacheSurface::CacheApiDefault);
    let fence = engine.prepare_put(&key).unwrap();
    engine
        .put(&CachePut {
            identity: key,
            request_headers: BTreeMap::new(),
            response: CacheStoredResponse {
                vary: Vec::new(),
                ..response(1, 100, 100, 100, "tag")
            },
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 2,
        })
        .unwrap();
    drop(engine);
    drop(manager);
    let reopened =
        CacheManager::open(&temp.path().join("data"), ResponseCacheConfig::default()).unwrap();
    let stats = reopened.worker_stats(account, worker, 3).unwrap();
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.body_bytes, 4);
    assert!(stats.metadata_bytes > 0);
    assert_eq!(stats.open_databases, 0);
    assert_eq!(
        reopened.referenced_bodies().unwrap(),
        vec![CacheBodyRef {
            sha256: "11".repeat(32),
            size: 4
        }]
    );
    assert_eq!(reopened.purge_worker(account, worker, 4).unwrap(), 1);
    assert!(reopened.referenced_bodies().unwrap().is_empty());
    assert_eq!(
        reopened.worker_stats(account, worker, 5).unwrap().entries,
        0
    );
}

#[test]
fn schema_fingerprint_rejects_ddl_drift() {
    let (temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    engine.verify().unwrap();
    let database = temp
        .path()
        .join("data/cache")
        .join(account.to_string())
        .join(worker.to_string())
        .join("cache.sqlite");
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute_batch("DROP INDEX cache_entries_url;")
        .unwrap();
    drop(connection);
    assert_eq!(engine.verify().unwrap_err().code(), ErrorCode::CacheCorrupt);
}

#[test]
fn persisted_response_metadata_is_revalidated_before_use() {
    let (temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let key = identity(account, worker, CacheSurface::CacheApiDefault);
    let fence = engine.prepare_put(&key).unwrap();
    engine
        .put(&CachePut {
            identity: key.clone(),
            request_headers: BTreeMap::new(),
            response: CacheStoredResponse {
                vary: Vec::new(),
                ..response(1, 100, 100, 100, "tag")
            },
            expected_fence_generation: fence,
            refresh_token: None,
            now_ms: 2,
        })
        .unwrap();
    let database = temp
        .path()
        .join("data/cache")
        .join(account.to_string())
        .join(worker.to_string())
        .join("cache.sqlite");
    rusqlite::Connection::open(database)
        .unwrap()
        .execute(
            "UPDATE cache_entries SET body_sha256 = ?1",
            ["zz".repeat(32)],
        )
        .unwrap();
    assert_eq!(
        engine.lookup(&key, &BTreeMap::new(), 3).unwrap_err().code(),
        ErrorCode::CacheCorrupt
    );
}

#[test]
fn purge_canonicalizes_case_insensitive_tags_and_absolute_url_prefixes() {
    let (_temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let key = identity(account, worker, CacheSurface::CacheApiDefault);
    for generation in 1..=2 {
        let now = i64::try_from(generation * 20).unwrap();
        let fence = engine.prepare_put(&key).unwrap();
        engine
            .put(&CachePut {
                identity: key.clone(),
                request_headers: BTreeMap::new(),
                response: CacheStoredResponse {
                    vary: Vec::new(),
                    ..response(generation, now + 100, now + 100, now + 100, "news")
                },
                expected_fence_generation: fence,
                refresh_token: None,
                now_ms: now,
            })
            .unwrap();
        let purge = if generation == 1 {
            CachePurge {
                tags: vec![" NEWS ".to_owned()],
                path_prefixes: Vec::new(),
                purge_everything: false,
            }
        } else {
            CachePurge {
                tags: Vec::new(),
                path_prefixes: vec!["https://EXAMPLE.com:443/path".to_owned()],
                purge_everything: false,
            }
        };
        assert_eq!(engine.purge(&purge, now + 10).unwrap(), 1);
    }
}

#[test]
fn response_status_and_purge_tombstone_history_are_bounded() {
    let (temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let key = identity(account, worker, CacheSurface::CacheApiDefault);
    for status in [199, 600] {
        let fence = engine.prepare_put(&key).unwrap();
        let error = engine
            .put(&CachePut {
                identity: key.clone(),
                request_headers: BTreeMap::new(),
                response: CacheStoredResponse {
                    status,
                    vary: Vec::new(),
                    ..response(1, 100, 100, 100, "tag")
                },
                expected_fence_generation: fence,
                refresh_token: None,
                now_ms: 2,
            })
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::CacheProtocolError);
    }

    for now_ms in 3..=70 {
        engine
            .purge(
                &CachePurge {
                    purge_everything: true,
                    ..CachePurge::default()
                },
                now_ms,
            )
            .unwrap();
    }
    let database = temp
        .path()
        .join("data/cache")
        .join(account.to_string())
        .join(worker.to_string())
        .join("cache.sqlite");
    let retained: u64 = rusqlite::Connection::open(database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM cache_tombstones", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, 64);
}

#[test]
fn cache_identity_metadata_and_purge_validation_reject_every_ambiguous_shape() {
    let (temp, manager, account, worker) = manager();
    let engine = manager.engine(account, worker, 1).unwrap();
    let base = identity(account, worker, CacheSurface::Automatic);
    let mut invalid = Vec::new();
    for mutate in [
        |value: &mut CacheIdentity| value.canonical_url = "x".repeat(9_000),
        |value: &mut CacheIdentity| value.canonical_url = "not-a-url".to_owned(),
        |value: &mut CacheIdentity| value.canonical_url = "ftp://example.com/".to_owned(),
        |value: &mut CacheIdentity| {
            value.canonical_url = "https://example.com/#fragment".to_owned();
        },
        |value: &mut CacheIdentity| value.canonical_url = "https://EXAMPLE.com:443/path".to_owned(),
        |value: &mut CacheIdentity| value.entrypoint = None,
        |value: &mut CacheIdentity| value.entrypoint = Some("9bad".to_owned()),
        |value: &mut CacheIdentity| value.cache_name = Some("named".to_owned()),
        |value: &mut CacheIdentity| value.version_scope = "not-a-deployment".to_owned(),
    ] {
        let mut value = base.clone();
        mutate(&mut value);
        invalid.push(value);
    }
    let default = identity(account, worker, CacheSurface::CacheApiDefault);
    for mutate in [
        |value: &mut CacheIdentity| value.entrypoint = Some("default".to_owned()),
        |value: &mut CacheIdentity| value.cache_name = Some("named".to_owned()),
        |value: &mut CacheIdentity| value.version_scope = "not-shared".to_owned(),
    ] {
        let mut value = default.clone();
        mutate(&mut value);
        invalid.push(value);
    }
    let named = identity(account, worker, CacheSurface::CacheApiNamed);
    for mutate in [
        |value: &mut CacheIdentity| value.cache_name = None,
        |value: &mut CacheIdentity| value.cache_name = Some(String::new()),
        |value: &mut CacheIdentity| value.cache_name = Some("bad\nname".to_owned()),
        |value: &mut CacheIdentity| value.cache_name = Some("x".repeat(300)),
        |value: &mut CacheIdentity| value.entrypoint = Some("default".to_owned()),
        |value: &mut CacheIdentity| value.version_scope = "not-shared".to_owned(),
    ] {
        let mut value = named.clone();
        mutate(&mut value);
        invalid.push(value);
    }
    for value in invalid {
        assert_eq!(
            engine.prepare_put(&value).unwrap_err().code(),
            ErrorCode::CacheKeyInvalid
        );
    }

    let mut head = default.clone();
    head.method = CacheMethod::Head;
    assert_eq!(
        default.canonical_bytes().unwrap(),
        head.canonical_bytes().unwrap()
    );
    assert_eq!(default.base_hash().unwrap(), head.base_hash().unwrap());
    assert_eq!(CacheMethod::Head.key_class(), "GET");

    assert_eq!(
        engine
            .lookup(
                &default,
                &BTreeMap::from([("Bad-Header".to_owned(), "value".to_owned())]),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine
            .lookup(&default, &BTreeMap::new(), -1)
            .unwrap_err()
            .code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine.stats(-1).unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine.delete(&base, &BTreeMap::new()).unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine.purge(&CachePurge::default(), 1).unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine
            .purge(
                &CachePurge {
                    path_prefixes: vec!["https://example.com/#fragment".to_owned()],
                    ..CachePurge::default()
                },
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine
            .purge(
                &CachePurge {
                    path_prefixes: vec!["/path".to_owned(); 65],
                    ..CachePurge::default()
                },
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        engine
            .purge(
                &CachePurge {
                    purge_everything: true,
                    ..CachePurge::default()
                },
                -1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        CacheEngine::open_or_create(
            temp.path().join("negative.sqlite"),
            account,
            worker,
            -1,
            ResponseCacheConfig::default(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::CacheProtocolError
    );
}

#[test]
fn cache_metadata_validators_cover_headers_vary_tags_and_deadlines() {
    use super::model::{
        canonical_url, corrupt, key_invalid, limit_error, protocol_error, validate_headers,
        validate_request_headers, validate_tags, validate_vary, vary_fingerprint,
    };

    let valid = vec![CacheHeader {
        name: "content-type".to_owned(),
        value: "text/plain".to_owned(),
    }];
    validate_headers(&valid, 128).unwrap();
    for header in [
        CacheHeader {
            name: String::new(),
            value: "x".to_owned(),
        },
        CacheHeader {
            name: "Bad".to_owned(),
            value: "x".to_owned(),
        },
        CacheHeader {
            name: "connection".to_owned(),
            value: "close".to_owned(),
        },
        CacheHeader {
            name: "x-open-compute-secret".to_owned(),
            value: "x".to_owned(),
        },
        CacheHeader {
            name: "content-type".to_owned(),
            value: "bad\rvalue".to_owned(),
        },
    ] {
        assert_eq!(
            validate_headers(&[header], 128).unwrap_err().code(),
            ErrorCode::CacheProtocolError
        );
    }
    assert_eq!(
        validate_headers(&valid, 1).unwrap_err().code(),
        ErrorCode::CacheLimitExceeded
    );
    validate_request_headers(
        &BTreeMap::from([("accept".to_owned(), "text/plain".to_owned())]),
        128,
    )
    .unwrap();

    validate_vary(&["accept".to_owned(), "accept-language".to_owned()]).unwrap();
    for vary in [
        vec!["*".to_owned()],
        vec![String::new()],
        vec!["Bad".to_owned()],
        vec!["accept".to_owned(), "accept".to_owned()],
    ] {
        assert_eq!(
            validate_vary(&vary).unwrap_err().code(),
            ErrorCode::CacheProtocolError
        );
    }
    assert_eq!(
        validate_vary(&vec!["x".to_owned(); 33]).unwrap_err().code(),
        ErrorCode::CacheLimitExceeded
    );

    validate_tags(&["news".to_owned(), "product".to_owned()], 2).unwrap();
    for tags in [
        vec![String::new()],
        vec!["NEWS".to_owned()],
        vec!["bad\ntag".to_owned()],
        vec!["news".to_owned(), "news".to_owned()],
    ] {
        assert_eq!(
            validate_tags(&tags, 8).unwrap_err().code(),
            ErrorCode::CacheProtocolError
        );
    }
    assert_eq!(
        validate_tags(&["a".to_owned(), "b".to_owned()], 1)
            .unwrap_err()
            .code(),
        ErrorCode::CacheLimitExceeded
    );

    let fingerprint = vary_fingerprint(
        &["accept".to_owned()],
        &BTreeMap::from([("accept".to_owned(), "text/plain".to_owned())]),
    );
    assert_ne!(
        fingerprint,
        vary_fingerprint(&["accept".to_owned()], &BTreeMap::new())
    );
    assert_eq!(
        canonical_url(url::Url::parse("HTTPS://EXAMPLE.COM:443/path#ignored").unwrap()).unwrap(),
        "https://example.com/path"
    );
    assert_eq!(key_invalid().code(), ErrorCode::CacheKeyInvalid);
    assert_eq!(protocol_error().code(), ErrorCode::CacheProtocolError);
    assert_eq!(limit_error().code(), ErrorCode::CacheLimitExceeded);
    assert_eq!(corrupt().code(), ErrorCode::CacheCorrupt);
}

#[test]
fn cache_path_enumeration_ignores_junk_and_rejects_identity_symlinks() {
    let (temp, manager, account, worker) = manager();
    drop(manager.engine(account, worker, 1).unwrap());
    drop(manager);
    let data = temp.path().join("data");
    let paths = CachePaths::open(&data).unwrap();
    assert_eq!(paths.root(), data.join("cache"));
    let worker_dir = paths.ensure_worker_dir(account, worker).unwrap();
    let database = paths.database_path(account, worker);
    std::fs::write(paths.root().join("junk-account"), b"junk").unwrap();
    std::fs::write(worker_dir.join("junk-worker"), b"junk").unwrap();
    assert_eq!(paths.databases().unwrap(), vec![database.clone()]);

    let linked_account = AccountId::generate();
    let linked_account_path = paths.root().join(linked_account.to_string());
    std::os::unix::fs::symlink(temp.path(), &linked_account_path).unwrap();
    assert_eq!(
        paths.databases().unwrap_err().code(),
        ErrorCode::CacheCorrupt
    );
    std::fs::remove_file(&linked_account_path).unwrap();

    let linked_worker = WorkerId::generate();
    let linked_worker_path = paths
        .root()
        .join(account.to_string())
        .join(linked_worker.to_string());
    std::os::unix::fs::symlink(temp.path(), &linked_worker_path).unwrap();
    assert_eq!(
        paths.databases().unwrap_err().code(),
        ErrorCode::CacheCorrupt
    );
}
