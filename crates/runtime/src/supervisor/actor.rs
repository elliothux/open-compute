use super::*;

pub(super) struct Actor {
    pub(super) runtime: VerifiedRuntime,
    pub(super) compiler: Arc<dyn ConfigCompiler>,
    pub(super) config: RuntimeConfig,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) jitter: Arc<dyn JitterRng>,
    pub(super) redactor: Redactor,
    pub(super) cmd_rx: mpsc::UnboundedReceiver<Command>,
    pub(super) watch_tx: watch::Sender<SupervisorSnapshot>,
    pub(super) snap: SupervisorSnapshot,
    pub(super) child: Option<LiveRuntime>,
    pub(super) in_flight: Option<InFlight>,
    pub(super) budget: RestartBudget,
    pub(super) consecutive_failures: u32,
    pub(super) shutting_down: bool,
    pub(super) owners: OwnerRegistry,
    pub(super) pending_shutdown_acks: Vec<oneshot::Sender<()>>,
    pub(super) diagnostics: Arc<std::sync::Mutex<Option<ProcessDiagnostics>>>,
    pub(super) last_report: Option<OwnerCompletion>,
    pub(super) lease_path: Option<PathBuf>,
    pub(super) lease_active: bool,
    pub(super) recovery_failed: bool,
    pub(super) external_services: Arc<[ExternalServiceAddress]>,
    pub(super) directory_services: Arc<[DirectoryServicePath]>,
    pub(super) generation_auths: Arc<[GenerationAuthRegistry]>,
}

impl Actor {
    pub(super) async fn run(mut self) {
        if let Some(path) = &self.lease_path {
            let digest = self.runtime.binary_sha256().to_owned();
            let path = path.clone();
            match tokio::task::spawn_blocking(move || recover_orphans(&path, &digest)).await {
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => {
                    self.recovery_failed = true;
                    self.permanent_fail(ErrorCode::RuntimeInvalid);
                }
            }
        }
        loop {
            if let Some(mut flight) = self.in_flight.take() {
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        self.in_flight = Some(flight);
                        let Some(cmd) = cmd else { break; };
                        self.handle(cmd).await;
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    result = &mut flight.task => {
                        if let Ok(outcome) = result {
                            self.on_attempt_outcome(outcome).await;
                        }
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        self.in_flight = Some(flight);
                        self.on_tick().await;
                    }
                }
            } else {
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break; };
                        self.handle(cmd).await;
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        self.on_tick().await;
                    }
                }
            }
        }
        self.force_stop().await;
        for ack in self.pending_shutdown_acks.drain(..) {
            let _ = ack.send(());
        }
        self.ack_pending_shutdowns();
    }

    fn shutdown_is_terminal(&self) -> bool {
        self.shutting_down
            && matches!(
                self.snap.state,
                SupervisorState::Stopped | SupervisorState::Failed
            )
    }

    fn fail_closed_after_teardown(&mut self) {
        self.clear_generation_auths();
        self.recovery_failed = true;
        self.permanent_fail(ErrorCode::RuntimeInvalid);
    }

    fn clear_generation_auths(&self) {
        for auth in self.generation_auths.iter() {
            auth.clear();
        }
    }

    async fn on_tick(&mut self) {
        self.poll_running().await;
        if self.snap.state == SupervisorState::BackingOff
            && let Some(at) = self.snap.next_retry_at
            && self.clock.now() >= at
            && !self.shutting_down
            && !self.recovery_failed
            && self.in_flight.is_none()
        {
            self.begin_attempt();
        }
    }

    fn ack_pending_shutdowns(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            if let Command::Shutdown { ack: Some(ack) } = cmd {
                let _ = ack.send(());
            }
        }
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Start => {
                if self.recovery_failed {
                    return;
                }
                if matches!(
                    self.snap.state,
                    SupervisorState::Stopped | SupervisorState::Failed
                ) && !self.shutting_down
                    && self.in_flight.is_none()
                {
                    self.budget = RestartBudget::new();
                    self.consecutive_failures = 0;
                    self.begin_attempt();
                }
            }
            Command::ReportUnhealthy => {
                if self.snap.state == SupervisorState::Running {
                    match self.teardown_child().await {
                        Ok(report) => {
                            self.fail_or_backoff(
                                ErrorCode::RuntimeExitedInFlight,
                                true,
                                report.as_ref(),
                            )
                            .await;
                        }
                        Err(_) => self.fail_closed_after_teardown(),
                    }
                }
            }
            Command::BeginDrain => {
                self.shutting_down = true;
                if self.cancel_attempt().await.is_err() {
                    self.fail_closed_after_teardown();
                } else {
                    self.graceful_stop().await;
                }
            }
            Command::Shutdown { ack } => {
                self.shutting_down = true;
                if let Some(ack) = ack {
                    self.pending_shutdown_acks.push(ack);
                }
                if self.cancel_attempt().await.is_err() {
                    self.fail_closed_after_teardown();
                } else {
                    self.graceful_stop().await;
                }
            }
        }
    }

    fn begin_attempt(&mut self) {
        self.last_report = None;
        self.transition(
            SupervisorState::Starting,
            ReadinessReason::RuntimeStarting,
            None,
            None,
        );
        self.snap.attempt = self.snap.attempt.saturating_add(1);
        self.publish();

        let token = match generate_internal_token() {
            Ok(t) => t,
            Err(err) => {
                self.permanent_fail(err.code());
                return;
            }
        };
        let mut redactor = self.redactor.clone();
        redactor.register_secret_string(&token);
        let startup_id = StartupId::generate();
        self.snap.startup_id = Some(startup_id);
        #[cfg(any(test, feature = "test-support"))]
        {
            self.snap.token_fingerprint = Some(token_fingerprint(&token));
        }
        self.publish();

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let compiler = self.compiler.clone();
        let runtime = self.runtime.clone();
        let startup = Duration::from_millis(self.config.startup_timeout_ms);
        let task = tokio::spawn(run_attempt(
            AttemptArgs {
                compiler,
                runtime,
                token,
                redactor,
                startup_id,
                startup,
                owners: self.owners.clone(),
                external_services: self.external_services.clone(),
                directory_services: self.directory_services.clone(),
                lease_path: self.lease_path.clone(),
            },
            cancel_rx,
        ));
        self.in_flight = Some(InFlight {
            task,
            cancel: Some(cancel_tx),
        });
    }

    async fn cancel_attempt(&mut self) -> Result<(), PlatformError> {
        let Some(mut flight) = self.in_flight.take() else {
            return Ok(());
        };
        if let Some(cancel) = flight.cancel.take() {
            let _ = cancel.send(());
        }
        match flight.task.await {
            Ok(AttemptOutcome::Ready(live)) => {
                self.child = Some(*live);
                self.teardown_child().await?;
            }
            Ok(AttemptOutcome::Failed(fail)) => {
                self.clear_generation_auths();
                if let Some(report) = fail.completion {
                    self.record_completion(report);
                }
                if let Some(pid) = fail.pid {
                    assert_reaped(Some(pid))?;
                }
            }
            Ok(AttemptOutcome::Cancelled) | Err(_) => {
                self.clear_generation_auths();
            }
        }
        Ok(())
    }

    async fn on_attempt_outcome(&mut self, outcome: AttemptOutcome) {
        match outcome {
            AttemptOutcome::Ready(live) => {
                if self.shutting_down {
                    self.child = Some(*live);
                    if self.teardown_child().await.is_err() {
                        self.fail_closed_after_teardown();
                    }
                    return;
                }
                self.consecutive_failures = 0;
                self.snap.config_digest = live.config_digest.clone();
                self.snap.listen_port = Some(live.port);
                let pid = live.pid();
                let pgid = live.pgid();
                self.child = Some(*live);
                if self.persist_lease(pid, pgid).is_err() {
                    let _ = self.teardown_child().await;
                    self.fail_closed_after_teardown();
                    return;
                }
                self.transition(
                    SupervisorState::Running,
                    ReadinessReason::Ready,
                    Some(pid),
                    Some(pgid),
                );
            }
            AttemptOutcome::Failed(fail) => {
                self.clear_generation_auths();
                let report = fail.completion.clone();
                if let Some(report) = fail.completion {
                    self.record_completion(report);
                }
                if let Some(pid) = fail.pid
                    && assert_reaped(Some(pid)).is_err()
                {
                    self.fail_closed_after_teardown();
                    return;
                }
                self.snap.pid = None;
                self.snap.pgid = None;
                self.snap.listen_port = None;
                if self.shutting_down {
                    self.transition(
                        SupervisorState::Stopped,
                        ReadinessReason::Draining,
                        None,
                        None,
                    );
                    return;
                }
                let retryable = !is_permanent(fail.error.code());
                self.fail_or_backoff(fail.error.code(), retryable, report.as_ref())
                    .await;
            }
            AttemptOutcome::Cancelled => {
                self.clear_generation_auths();
                if self.shutting_down {
                    self.transition(
                        SupervisorState::Stopped,
                        ReadinessReason::Draining,
                        None,
                        None,
                    );
                }
            }
        }
    }

    async fn poll_running(&mut self) {
        if self.snap.state != SupervisorState::Running {
            return;
        }
        let mut exited = false;
        let mut unhealthy = false;
        if let Some(live) = self.child.as_mut() {
            if !live.handle.leader_alive() {
                exited = true;
            } else {
                let mut buf = [0u8; 1024];
                let read = if let Ok(control) = live.ensure_control() {
                    tokio::time::timeout(Duration::from_millis(1), control.read(&mut buf)).await
                } else {
                    unhealthy = true;
                    Ok(Ok(0))
                };
                if !unhealthy {
                    match read {
                        Ok(Ok(0)) => {
                            if !live.parser.accepted() {
                                unhealthy = true;
                            }
                        }
                        Ok(Ok(n)) => {
                            if live.parser.push(&buf[..n]).is_err() {
                                unhealthy = true;
                            }
                        }
                        Ok(Err(_)) => unhealthy = true,
                        Err(_) => {}
                    }
                }
            }
        }
        if exited {
            self.on_child_exit().await;
        } else if unhealthy {
            self.control_unhealthy().await;
        }
    }

    async fn control_unhealthy(&mut self) {
        if self.shutting_down {
            return;
        }
        match self.teardown_child().await {
            Ok(report) => {
                self.fail_or_backoff(ErrorCode::RuntimeExitedInFlight, true, report.as_ref())
                    .await;
            }
            Err(_) => self.fail_closed_after_teardown(),
        }
    }

    async fn on_child_exit(&mut self) {
        let Ok(report) = self.teardown_child().await else {
            self.fail_closed_after_teardown();
            return;
        };
        if self.shutting_down {
            self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            );
            return;
        }
        self.fail_or_backoff(ErrorCode::RuntimeExitedInFlight, true, report.as_ref())
            .await;
    }

    async fn fail_or_backoff(
        &mut self,
        code: ErrorCode,
        consume_budget: bool,
        report: Option<&OwnerCompletion>,
    ) {
        let now = self.clock.now();
        let window = Duration::from_millis(self.config.restart_window_ms);
        if consume_budget {
            self.budget.record(now, window);
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let (exit_code, signal) = report.map_or((None, None), |r| (r.exit_code(), r.signal()));
        self.snap.last_exit = Some(SanitizedExit {
            code: exit_code,
            signal,
            retryable: consume_budget && !is_permanent(code),
            code_name: code.as_str().to_owned(),
        });
        if is_permanent(code) {
            self.permanent_fail(code);
            return;
        }
        if consume_budget && self.budget.exceeded(self.config.restart_budget) {
            self.permanent_fail(ErrorCode::RuntimeInvalid);
            self.snap.reason = ReadinessReason::RuntimeInvalid;
            self.publish();
            return;
        }
        let delay = backoff_delay(
            &self.config,
            self.consecutive_failures,
            self.jitter.as_ref(),
        );
        let next = now + delay;
        self.snap.next_retry_at = Some(next);
        self.transition(
            SupervisorState::BackingOff,
            ReadinessReason::RuntimeRestartBackoff,
            None,
            None,
        );
    }

    fn permanent_fail(&mut self, code: ErrorCode) {
        let reason = match code {
            ErrorCode::ConfigCompileFailed | ErrorCode::ConfigInvalid => {
                ReadinessReason::ConfigInvalid
            }
            _ => ReadinessReason::RuntimeInvalid,
        };
        self.snap.next_retry_at = None;
        self.snap.pid = None;
        self.snap.pgid = None;
        self.transition(SupervisorState::Failed, reason, None, None);
    }

    async fn graceful_stop(&mut self) {
        if self.snap.state == SupervisorState::Stopped {
            return;
        }
        self.transition(
            SupervisorState::Draining,
            ReadinessReason::Draining,
            self.snap.pid,
            self.snap.pgid,
        );
        tokio::time::sleep(Duration::from_millis(self.config.drain_timeout_ms)).await;
        self.transition(
            SupervisorState::Stopping,
            ReadinessReason::Draining,
            self.snap.pid,
            self.snap.pgid,
        );
        match self.teardown_child().await {
            Ok(_) => self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            ),
            Err(_) => self.fail_closed_after_teardown(),
        }
    }

    fn persist_lease(&mut self, pid: i32, pgid: i32) -> Result<(), PlatformError> {
        let Some(path) = &self.lease_path else {
            return Ok(());
        };
        let Some(lease) = capture_lease(pid, pgid, self.runtime.binary_sha256()) else {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to capture runtime child lease identity",
            ));
        };
        write_lease(path, &lease)?;
        self.lease_active = true;
        Ok(())
    }

    async fn teardown_child(&mut self) -> Result<Option<OwnerCompletion>, PlatformError> {
        self.clear_generation_auths();
        let report = if let Some(live) = self.child.take() {
            let pid = live.pid();
            let pgid = live.pgid();
            let report = live
                .shutdown(
                    Duration::from_millis(self.config.shutdown_grace_ms),
                    Duration::from_millis(self.config.kill_timeout_ms),
                )
                .await;
            self.record_completion(report.clone());
            if pid != pgid {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime child is not its process group leader",
                ));
            }
            wait_reaped(pid, Duration::from_secs(2))?;
            Some(report)
        } else {
            None
        };
        if report.is_none() && self.lease_active {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "active child lease cannot be cleared without a reap proof",
            ));
        }
        if self.lease_active {
            let path = self.lease_path.as_ref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "active child lease path is missing",
                )
            })?;
            clear_lease(path)?;
            self.lease_active = false;
        }
        self.snap.pid = None;
        self.snap.pgid = None;
        self.snap.listen_port = None;
        Ok(report)
    }

    fn record_completion(&mut self, report: OwnerCompletion) {
        let diag = ProcessDiagnostics {
            stdout_tail: report.stdout.as_lossy_str(),
            stderr_tail: report.stderr.as_lossy_str(),
            exit_code: report.exit_code(),
            signal: report.signal(),
            reader_failed: report.reader_failed,
        };
        *self
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(diag);
        self.last_report = Some(report);
    }

    async fn force_stop(&mut self) {
        if self.recovery_failed {
            return;
        }
        if self.cancel_attempt().await.is_err() || self.teardown_child().await.is_err() {
            self.fail_closed_after_teardown();
            return;
        }
        if self.snap.state != SupervisorState::Stopped {
            self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            );
        }
    }

    fn transition(
        &mut self,
        state: SupervisorState,
        reason: ReadinessReason,
        pid: Option<i32>,
        pgid: Option<i32>,
    ) {
        self.snap.state = state;
        self.snap.reason = reason;
        self.snap.last_transition_at = self.clock.now();
        self.snap.pid = pid;
        self.snap.pgid = pgid;
        if !matches!(
            state,
            SupervisorState::Starting
                | SupervisorState::Running
                | SupervisorState::Draining
                | SupervisorState::Stopping
        ) {
            self.snap.pid = None;
            self.snap.pgid = None;
        }
        self.publish();
    }

    fn publish(&self) {
        let _ = self.watch_tx.send(self.snap.clone());
    }
}

struct AttemptArgs {
    compiler: Arc<dyn ConfigCompiler>,
    runtime: VerifiedRuntime,
    token: SecretString,
    redactor: Redactor,
    startup_id: StartupId,
    startup: Duration,
    owners: OwnerRegistry,
    external_services: Arc<[ExternalServiceAddress]>,
    directory_services: Arc<[DirectoryServicePath]>,
    lease_path: Option<PathBuf>,
}

async fn run_attempt(args: AttemptArgs, mut cancel: oneshot::Receiver<()>) -> AttemptOutcome {
    let AttemptArgs {
        compiler,
        runtime,
        token,
        redactor,
        startup_id,
        startup,
        owners,
        external_services,
        directory_services,
        lease_path,
    } = args;
    let compiled = tokio::select! {
        biased;
        _ = &mut cancel => return AttemptOutcome::Cancelled,
        compiled = compiler.compile(token.clone(), startup_id) => compiled,
    };
    let compiled = match compiled {
        Ok(c) => c,
        Err(err) => {
            return AttemptOutcome::Failed(SpawnFailure {
                error: err,
                pid: None,
                pgid: None,
                completion: None,
            });
        }
    };

    let runtime_spawn = runtime.clone();
    let token_spawn = token.clone();
    let redactor_spawn = redactor.clone();
    let owners_spawn = owners.clone();
    let spawn_lease_path = lease_path.clone();
    let spawn_task = tokio::task::spawn_blocking(move || {
        spawn_child(&SpawnRequest {
            runtime: &runtime_spawn,
            compiled: &compiled,
            token: &token_spawn,
            redactor: &redactor_spawn,
            owners: &owners_spawn,
            external_services: &external_services,
            directory_services: &directory_services,
            lease_path: spawn_lease_path.as_deref(),
        })
    });
    tokio::pin!(spawn_task);
    let mut cancel_pending = false;
    let spawned = loop {
        tokio::select! {
            biased;
            _ = &mut cancel, if !cancel_pending => {
                cancel_pending = true;
            }
            spawned = &mut spawn_task => break spawned,
        }
    };
    let mut live = match spawned {
        Ok(Ok(live)) => live,
        Ok(Err(fail)) => return AttemptOutcome::Failed(fail),
        Err(_) => {
            return AttemptOutcome::Failed(SpawnFailure {
                error: PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime spawn task ended without a result",
                ),
                pid: None,
                pgid: None,
                completion: None,
            });
        }
    };
    if cancel_pending {
        let pid = live.pid();
        live.shutdown(Duration::from_millis(0), Duration::from_secs(2))
            .await;
        clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
        return AttemptOutcome::Cancelled;
    }

    tokio::select! {
        biased;
        _ = &mut cancel => {
            let pid = live.pid();
            live.shutdown(Duration::from_millis(0), Duration::from_secs(2)).await;
            clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
            AttemptOutcome::Cancelled
        }
        ready = wait_ready(&mut live, &token, startup) => {
            match ready {
                Ok(port) => {
                    live.port = port;
                    AttemptOutcome::Ready(Box::new(live))
                }
                Err(error) => {
                    let pid = live.pid();
                    let pgid = live.pgid();
                    let completion = live.shutdown(Duration::from_millis(0), Duration::from_secs(2)).await;
                    clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
                    AttemptOutcome::Failed(SpawnFailure {
                        error,
                        pid: Some(pid),
                        pgid: Some(pgid),
                        completion: Some(completion),
                    })
                }
            }
        }
    }
}

fn clear_attempt_lease_if_reaped(lease_path: Option<&Path>, pid: i32) {
    if wait_reaped(pid, Duration::from_secs(2)).is_ok()
        && let Some(path) = lease_path
    {
        let _ = clear_lease(path);
    }
}

fn is_permanent(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::RuntimeInvalid
            | ErrorCode::ConfigCompileFailed
            | ErrorCode::ConfigInvalid
            | ErrorCode::PathInvalid
            | ErrorCode::CacheEntryCorrupt
            | ErrorCode::LimitInvalid
            | ErrorCode::SchemaTooNew
            | ErrorCode::MasterKeyMismatch
    )
}
