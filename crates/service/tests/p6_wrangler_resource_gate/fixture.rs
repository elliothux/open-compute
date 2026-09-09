use super::*;

pub(super) struct Fixture {
    pub(super) process: platform_process::Process,
    pub(super) _mock: MockS3,
    pub(super) _evidence: Evidence,
    pub(super) _embedding: search::EmbeddingFixture,
    pub(super) project: PathBuf,
    pub(super) public_addr: SocketAddr,
    pub(super) admin_addr: SocketAddr,
    pub(super) public_account: String,
    pub(super) internal_account: String,
    pub(super) data: PathBuf,
    pub(super) config: PathBuf,
    pub(super) log: PathBuf,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let repo = repo_root();
        let workerd = PathBuf::from(
            std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
                .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime"),
        );
        assert!(workerd.is_absolute());
        assert!(workerd.is_file());
        verify_runtime_binary(
            &repo.join("packages/runtime/workerd.lock.json"),
            &workerd,
            Duration::from_secs(10),
            &Redactor::new(),
        )
        .await
        .expect("formal pinned stock runtime");

        let runs = repo.join(".temp/p6-wrangler-resource-run");
        fs::create_dir_all(&runs).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("resources-")
            .tempdir_in(runs)
            .unwrap();
        let evidence = Evidence::new(temp);
        let root = evidence.path().to_owned();
        let data = root.join("data");
        let storage = PlatformStorage::bootstrap(&storage_config(&data), &SystemClock).unwrap();
        let internal_account = storage.identity().default_account_id.to_string();
        seed_workflow(&storage);
        drop(storage);

        let mock = MockS3::spawn("open-compute").await;
        let embedding = search::EmbeddingFixture::spawn().await;
        let (public_addr, admin_addr) = platform_process::distinct_addresses();
        let config =
            platform_process::config(&root, &data, &mock.endpoint, public_addr, admin_addr);
        append_resource_config(&config, &root, &embedding.base_url, admin_addr);

        let log = root.join("ocd.stderr.log");
        let mut process = platform_process::spawn(&config, &log);
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http();
        wait_ready(&client, admin_addr, &mut process, &log).await;
        let public_account = discover_public_account(&client, admin_addr).await;

        let project = root.join("wrangler-project");
        fs::create_dir(&project).unwrap();
        fs::create_dir(project.join("xdg")).unwrap();
        fs::write(
            project.join("index.ts"),
            "export default { fetch() { return new Response('ok'); } };",
        )
        .unwrap();
        write_config(&project, &public_account, None, None);
        Self {
            process,
            _mock: mock,
            _evidence: evidence,
            _embedding: embedding,
            project,
            public_addr,
            admin_addr,
            public_account,
            internal_account,
            data,
            config,
            log,
        }
    }
}
