use super::*;

pub(super) async fn run() {
    let mut fixture = Fixture::new().await;
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };

    assert_success(&command.run(&["--version"]).await);
    exercise_tail(
        &command,
        fixture.admin_addr,
        fixture.public_addr,
        &fixture.public_account,
        &fixture.internal_account,
    )
    .await;
    exercise_live_tail(
        fixture.admin_addr,
        fixture.public_addr,
        &fixture.public_account,
        &fixture.internal_account,
    )
    .await;
    exercise_p12_project_workflow(&fixture).await;
    exercise_kv(&command, &fixture.project).await;
    exercise_d1(&command, &fixture.project).await;
    exercise_r2(&command, &fixture.project).await;
    exercise_queues(&command).await;
    exercise_workflows(&command).await;
    search::exercise_vectorize(&command, &fixture.project).await;
    search::exercise_ai_search(&command).await;

    let log = fs::read(&fixture.log).unwrap_or_default();
    assert_clean_output(&log);
    assert!(
        fixture.process.0.try_wait().unwrap().is_none(),
        "ocd exited while fixed Wrangler was using the admin v4 listener: {}",
        String::from_utf8_lossy(&log),
    );
    fixture.process.stop().await;
    assert!(
        tokio::net::TcpStream::connect(fixture.public_addr)
            .await
            .is_err(),
        "normal shutdown left the public listener reachable",
    );
    assert!(
        tokio::net::TcpStream::connect(fixture.admin_addr)
            .await
            .is_err(),
        "normal shutdown left the admin listener reachable",
    );

    fixture.process.restart(&fixture.config, &fixture.log);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    wait_ready(
        &client,
        fixture.admin_addr,
        &mut fixture.process,
        &fixture.log,
    )
    .await;
    assert_eq!(
        tail_count(&client, fixture.admin_addr, &fixture.public_account).await,
        0,
        "ephemeral tail sessions must not survive an ocd restart",
    );
    wait_persisted_tail_log(&client, fixture.admin_addr, &fixture.public_account).await;
    fixture.process.stop().await;
    assert_observability_audit(&fixture.data);
    assert_clean_output(&fs::read(&fixture.log).unwrap_or_default());
}
