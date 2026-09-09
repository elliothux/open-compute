use super::*;

#[tokio::test]
async fn listener_plan_and_task_join_errors_are_stable() {
    let mut server = open_compute_core::ServerConfig {
        public_bind: "127.0.0.1:8080".to_owned(),
        admin_bind: Some("127.0.0.1:8080".to_owned()),
        ..open_compute_core::ServerConfig::default()
    };
    assert_eq!(listener_plan(&server).unwrap().1, None);
    server.admin_bind = Some("127.0.0.1:8081".to_owned());
    assert_eq!(listener_plan(&server).unwrap().1.unwrap().port(), 8081);
    server.public_bind = "not-an-address".to_owned();
    assert!(listener_plan(&server).is_err());

    assert_eq!(join_listener(Ok(Ok(()))).code(), ErrorCode::ConfigInvalid);
    assert_eq!(
        join_listener(Ok(Err(open_compute_core::PlatformError::new(
            ErrorCode::PathInvalid,
            "listener",
        ))))
        .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        join_runtime_source(Ok(Ok(()))).code(),
        ErrorCode::RuntimeUnavailable
    );
    assert_eq!(
        join_runtime_source(Ok(Err(open_compute_core::PlatformError::new(
            ErrorCode::Internal,
            "runtime source",
        ))))
        .code(),
        ErrorCode::Internal
    );
    assert_eq!(
        join_scheduler(Ok(Ok(()))).code(),
        ErrorCode::SchedulerUnavailable
    );
    assert_eq!(
        join_scheduler(Ok(Err(open_compute_core::PlatformError::new(
            ErrorCode::SchedulerCorrupt,
            "scheduler",
        ))))
        .code(),
        ErrorCode::SchedulerCorrupt
    );

    let listener_panic = tokio::spawn(async { panic!("listener test panic") })
        .await
        .unwrap_err();
    assert_eq!(
        join_listener(Err(listener_panic)).code(),
        ErrorCode::ConfigInvalid
    );
    let scheduler_panic = tokio::spawn(async { panic!("scheduler test panic") })
        .await
        .unwrap_err();
    assert_eq!(
        join_scheduler(Err(scheduler_panic)).code(),
        ErrorCode::SchedulerUnavailable
    );
    let source_panic = tokio::spawn(async { panic!("source test panic") })
        .await
        .unwrap_err();
    assert_eq!(
        join_runtime_source(Err(source_panic)).code(),
        ErrorCode::RuntimeUnavailable
    );
}
