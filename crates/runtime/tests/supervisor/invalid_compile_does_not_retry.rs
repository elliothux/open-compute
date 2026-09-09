use super::*;

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler: FnCompiler(|_t, _id| {
                Box::pin(async {
                    Err(open_compute_core::PlatformError::new(
                        ErrorCode::ConfigCompileFailed,
                        "static config compilation failed",
                    ))
                })
                    as Pin<
                        Box<
                            dyn Future<
                                    Output = Result<
                                        CompiledConfig,
                                        open_compute_core::PlatformError,
                                    >,
                                > + Send,
                        >,
                    >
            }),
            config: small_cfg(),
            clock: clock.clone(),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    assert_eq!(snap.reason, ReadinessReason::ConfigInvalid);
    clock.advance(Duration::from_secs(10));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(sup.snapshot().state, SupervisorState::Failed);
    assert_eq!(sup.snapshot().attempt, 1);
    sup.shutdown().await;
}
