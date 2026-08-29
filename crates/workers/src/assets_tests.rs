use super::*;

fn entry(path: &str, digest: char, size: u64) -> AssetEntryV1 {
    AssetEntryV1 {
        path: path.to_owned(),
        sha256: digest.to_string().repeat(64),
        size,
        content_type: "text/plain; charset=utf-8".to_owned(),
    }
}

#[test]
fn canonical_manifest_requires_sorted_unique_safe_paths() {
    let manifest = AssetManifestV1 {
        schema_version: 1,
        entries: vec![entry("/a.txt", 'a', 1), entry("/z.txt", 'b', 2)],
    };
    assert_eq!(manifest.total_bytes().unwrap(), 3);
    assert_eq!(manifest.sha256().unwrap().len(), 32);
    let mut reversed = manifest.clone();
    reversed.entries.reverse();
    assert_eq!(
        reversed.validate().unwrap_err().code(),
        ErrorCode::AssetManifestInvalid
    );
    validate_asset_path("/%E4%BD%A0%E5%A5%BD/%25%20%5Bname%5D.js").unwrap();
    for path in [
        "//a",
        "/",
        "/a/",
        "/a/../b",
        "/a/%2E%2E/b",
        "/a/%2f/b",
        "/[name].js",
        "/你好",
        "/a\\b",
        "/a\nb",
    ] {
        assert_eq!(
            validate_asset_path(path).unwrap_err().code(),
            ErrorCode::AssetPathInvalid
        );
    }
}

#[test]
fn manifest_enforces_logical_quotas_before_deduplication() {
    for manifest in [
        AssetManifestV1 {
            schema_version: 0,
            entries: vec![entry("/a", 'a', 1)],
        },
        AssetManifestV1 {
            schema_version: 1,
            entries: Vec::new(),
        },
    ] {
        assert_eq!(
            manifest.validate().unwrap_err().code(),
            ErrorCode::AssetManifestInvalid
        );
    }
    let mut invalid_content_type = entry("/invalid.txt", 'a', 1);
    invalid_content_type.content_type.clear();
    assert_eq!(
        AssetManifestV1 {
            schema_version: 1,
            entries: vec![invalid_content_type],
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::AssetManifestInvalid
    );
    let manifest = AssetManifestV1 {
        schema_version: 1,
        entries: vec![entry("/large.bin", 'a', MAX_ASSET_FILE_BYTES + 1)],
    };
    assert_eq!(
        manifest.validate().unwrap_err().code(),
        ErrorCode::AssetLimitExceeded
    );
    let entries = (0..MAX_ASSET_FILES + 1)
        .map(|index| entry(&format!("/{index:05}.txt"), 'a', 0))
        .collect();
    assert_eq!(
        AssetManifestV1 {
            schema_version: 1,
            entries
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::AssetLimitExceeded
    );
    let entries = (0..=MAX_ASSET_TOTAL_BYTES / MAX_ASSET_FILE_BYTES)
        .map(|index| entry(&format!("/{index:05}.bin"), 'a', MAX_ASSET_FILE_BYTES))
        .collect();
    assert_eq!(
        AssetManifestV1 {
            schema_version: 1,
            entries,
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::AssetLimitExceeded
    );
    assert_eq!(RunWorkerFirst::default(), RunWorkerFirst::All(false));
}

#[test]
fn routing_rejects_ambiguous_or_unsafe_rules() {
    let valid = AssetRoutingConfigV1 {
        schema_version: 1,
        binding: Some("STATIC".to_owned()),
        run_worker_first: RunWorkerFirst::Rules(vec![
            "/api/*".to_owned(),
            "!/api/docs/*".to_owned(),
        ]),
        html_handling: HtmlHandling::AutoTrailingSlash,
        not_found_handling: NotFoundHandling::Page404,
        headers: vec![AssetHeaderRule {
            pattern: "/*.html".to_owned(),
            operations: vec![AssetHeaderOperation {
                name: "cache-control".to_owned(),
                value: Some("no-cache".to_owned()),
            }],
        }],
        redirects: vec![AssetRedirectRule {
            from: "/old".to_owned(),
            to: "/new".to_owned(),
            status: 308,
        }],
    };
    valid.validate().unwrap();
    let mut invalid = valid.clone();
    invalid.redirects[0].status = 206;
    assert_eq!(
        invalid.validate().unwrap_err().code(),
        ErrorCode::AssetConfigUnsupported
    );

    let invalid_configs = [
        AssetRoutingConfigV1 {
            schema_version: 0,
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            binding: Some("A".repeat(65)),
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            run_worker_first: RunWorkerFirst::Rules(Vec::new()),
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            run_worker_first: RunWorkerFirst::Rules(vec!["relative".to_owned()]),
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            headers: vec![AssetHeaderRule {
                pattern: "/path".to_owned(),
                operations: Vec::new(),
            }],
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            headers: vec![AssetHeaderRule {
                pattern: "/path".to_owned(),
                operations: vec![AssetHeaderOperation {
                    name: "Uppercase".to_owned(),
                    value: None,
                }],
            }],
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            headers: vec![AssetHeaderRule {
                pattern: "/*/*".to_owned(),
                operations: vec![AssetHeaderOperation {
                    name: "x-test".to_owned(),
                    value: None,
                }],
            }],
            ..valid.clone()
        },
        AssetRoutingConfigV1 {
            redirects: vec![AssetRedirectRule {
                from: "/:".to_owned(),
                to: "/destination".to_owned(),
                status: 302,
            }],
            ..valid
        },
    ];
    for invalid in invalid_configs {
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            ErrorCode::AssetConfigUnsupported
        );
    }

    for path in ["/a//b", "/%FF", "/bad.txt"] {
        let result = if path == "/bad.txt" {
            AssetManifestV1 {
                schema_version: 1,
                entries: vec![AssetEntryV1 {
                    path: path.to_owned(),
                    sha256: "G".repeat(64),
                    size: 1,
                    content_type: "text/plain".to_owned(),
                }],
            }
            .validate()
        } else {
            validate_asset_path(path)
        };
        assert!(result.is_err());
    }
}
