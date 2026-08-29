use super::*;
use crate::assets::{AssetHeaderOperation, AssetHeaderRule, AssetRedirectRule, RunWorkerFirst};

fn entry(path: &str, digest: u8, content_type: &str) -> AssetEntryV1 {
    AssetEntryV1 {
        path: path.to_owned(),
        sha256: format!("{digest:02x}").repeat(32),
        size: u64::from(digest),
        content_type: content_type.to_owned(),
    }
}

fn manifest() -> AssetManifestV1 {
    AssetManifestV1 {
        schema_version: 1,
        entries: vec![
            entry("/404.html", 1, "text/html; charset=utf-8"),
            entry("/file.html", 2, "text/html; charset=utf-8"),
            entry("/folder/index.html", 3, "text/html; charset=utf-8"),
            entry("/index.html", 4, "text/html; charset=utf-8"),
            entry("/static/app.js", 5, "text/javascript; charset=utf-8"),
        ],
    }
}

fn routing() -> AssetRoutingConfigV1 {
    AssetRoutingConfigV1 {
        schema_version: 1,
        binding: Some("ASSETS".to_owned()),
        run_worker_first: RunWorkerFirst::All(false),
        html_handling: HtmlHandling::AutoTrailingSlash,
        not_found_handling: NotFoundHandling::None,
        headers: Vec::new(),
        redirects: Vec::new(),
    }
}

fn request<'a>(path: &'a str) -> AssetRequest<'a> {
    AssetRequest {
        method: "GET",
        path,
        query: None,
        host: "example.com",
        sec_fetch_mode: None,
        if_none_match: None,
        has_authorization: false,
        has_range: false,
    }
}

#[test]
fn html_modes_match_the_documented_canonical_matrix() {
    let manifest = manifest();
    let mut config = routing();
    let plan = plan_asset_response(&manifest, &config, request("/file")).unwrap();
    assert_eq!(plan.status, 200);
    assert_eq!(plan.entry.unwrap().path, "/file.html");
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/file.html"))
            .unwrap()
            .headers["location"],
        "/file"
    );
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/folder"))
            .unwrap()
            .headers["location"],
        "/folder/"
    );
    config.html_handling = HtmlHandling::ForceTrailingSlash;
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/file"))
            .unwrap()
            .headers["location"],
        "/file/"
    );
    config.html_handling = HtmlHandling::DropTrailingSlash;
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/folder"))
            .unwrap()
            .entry
            .unwrap()
            .path,
        "/folder/index.html"
    );
    config.html_handling = HtmlHandling::None;
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/file"))
            .unwrap()
            .status,
        404
    );
}

#[test]
fn etag_head_spa_and_custom_rules_are_deterministic() {
    let manifest = manifest();
    let mut config = routing();
    config.not_found_handling = NotFoundHandling::SinglePageApplication;
    config.redirects.push(AssetRedirectRule {
        from: "/old/:name".to_owned(),
        to: "/new/:name".to_owned(),
        status: 308,
    });
    config.headers.push(AssetHeaderRule {
        pattern: "/static/*".to_owned(),
        operations: vec![AssetHeaderOperation {
            name: "cache-control".to_owned(),
            value: Some("public, immutable".to_owned()),
        }],
    });
    let mut redirect = request("/old/page");
    redirect.query = Some("a=1");
    assert_eq!(
        plan_asset_response(&manifest, &config, redirect)
            .unwrap()
            .headers["location"],
        "/new/page?a=1"
    );
    let static_plan = plan_asset_response(&manifest, &config, request("/static/app.js")).unwrap();
    assert_eq!(static_plan.headers["cache-control"], "public, immutable");
    let etag = static_plan.headers["etag"].clone();
    let mut conditional = request("/static/app.js");
    conditional.if_none_match = Some(&etag);
    let not_modified = plan_asset_response(&manifest, &config, conditional).unwrap();
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.entry.is_none());
    let mut head = request("/missing");
    head.method = "HEAD";
    let spa = plan_asset_response(&manifest, &config, head).unwrap();
    assert!(spa.head);
    assert_eq!(spa.entry.unwrap().path, "/index.html");

    let mut unsupported = request("/");
    unsupported.method = "POST";
    assert_eq!(
        plan_asset_response(&manifest, &config, unsupported)
            .unwrap()
            .status,
        405
    );

    config.redirects = vec![AssetRedirectRule {
        from: "/rewrite/*".to_owned(),
        to: "/static/:splat?a=1".to_owned(),
        status: 200,
    }];
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/rewrite/app.js"))
            .unwrap()
            .entry
            .unwrap()
            .path,
        "/static/app.js"
    );
}

#[test]
fn encoded_paths_range_404_and_header_precedence_are_explicit() {
    let manifest = AssetManifestV1 {
        schema_version: 1,
        entries: vec![
            entry("/%255Bname%255D.html", 1, "text/html; charset=utf-8"),
            entry("/404.html", 2, "text/html; charset=utf-8"),
            entry("/caf%C3%A9.html", 3, "text/html; charset=utf-8"),
            entry("/docs/404.html", 4, "text/html; charset=utf-8"),
            entry("/index.html", 5, "text/html; charset=utf-8"),
        ],
    };
    let mut config = routing();
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/caf%C3%A9"))
            .unwrap()
            .entry
            .unwrap()
            .path,
        "/caf%C3%A9.html"
    );
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/%255Bname%255D"))
            .unwrap()
            .entry
            .unwrap()
            .path,
        "/%255Bname%255D.html"
    );
    assert_eq!(
        plan_asset_response(&manifest, &config, request("/%5Bname%5D"))
            .unwrap()
            .status,
        404
    );

    config.not_found_handling = NotFoundHandling::Page404;
    let nested = plan_asset_response(&manifest, &config, request("/docs/missing/deep")).unwrap();
    assert_eq!(nested.status, 404);
    assert_eq!(nested.entry.unwrap().path, "/docs/404.html");

    config.not_found_handling = NotFoundHandling::None;
    config.headers = vec![
        AssetHeaderRule {
            pattern: "https://example.com/*".to_owned(),
            operations: vec![AssetHeaderOperation {
                name: "x-site".to_owned(),
                value: Some("example.com".to_owned()),
            }],
        },
        AssetHeaderRule {
            pattern: "/*".to_owned(),
            operations: vec![
                AssetHeaderOperation {
                    name: "x-site".to_owned(),
                    value: Some("second".to_owned()),
                },
                AssetHeaderOperation {
                    name: "etag".to_owned(),
                    value: None,
                },
            ],
        },
    ];
    let mut range = request("/");
    range.has_range = true;
    let ranged = plan_asset_response(&manifest, &config, range).unwrap();
    assert_eq!(ranged.status, 200);
    assert_eq!(ranged.headers["content-length"], "5");
    assert!(!ranged.headers.contains_key("accept-ranges"));
    assert!(!ranged.headers.contains_key("cache-control"));
    assert!(!ranged.headers.contains_key("etag"));
    assert_eq!(ranged.headers["x-site"], "example.com, second");

    let mut authorized = request("/");
    authorized.has_authorization = true;
    assert!(
        !plan_asset_response(&manifest, &config, authorized)
            .unwrap()
            .headers
            .contains_key("cache-control")
    );

    config.not_found_handling = NotFoundHandling::Page404;
    let without_page = AssetManifestV1 {
        schema_version: 1,
        entries: vec![entry("/index.html", 1, "text/html")],
    };
    assert_eq!(
        plan_asset_response(&without_page, &config, request("/missing"))
            .unwrap()
            .status,
        404
    );

    for mode in [
        HtmlHandling::ForceTrailingSlash,
        HtmlHandling::DropTrailingSlash,
    ] {
        config.html_handling = mode;
        config.not_found_handling = NotFoundHandling::None;
        let plan = plan_asset_response(&manifest, &config, request("/absent")).unwrap();
        assert_eq!(plan.status, 404);
    }
}

#[test]
fn rule_matching_handles_unicode_backtracking_and_empty_placeholders() {
    assert_eq!(
        split_path("/asset?key=value"),
        ("/asset", Some("key=value"))
    );
    assert_eq!(split_path("/asset"), ("/asset", None));
    assert!(match_rule("/*x", "example.com", "/é").is_none());
    assert!(match_rule("/:name/end", "example.com", "//end").is_none());
    assert!(match_rule("/literal", "example.com", "/different").is_none());
}
