use super::*;

/// Streaming client for the current workerd generation.
#[derive(Clone)]
pub struct WorkerdTransport {
    pub(super) supervisor: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
    pub(super) python_preparation: Arc<Mutex<Option<Arc<python_preparation::PythonPreparation>>>>,
    pub(super) max_request_body: usize,
    pub(super) version_pins: Option<VersionPins>,
    pub(super) service_invocations: Option<crate::service_invocations::ServiceInvocationRegistry>,
    pub(super) workflow_quarantine: Arc<Mutex<Option<open_compute_core::StartupId>>>,
    #[cfg(test)]
    pub(super) test_endpoint: Option<PathBuf>,
    #[cfg(test)]
    pub(crate) test_generation: Option<open_compute_core::StartupId>,
}

impl std::fmt::Debug for WorkerdTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerdTransport").finish_non_exhaustive()
    }
}

impl WorkerdTransport {
    /// Bind transport to the supervisor slot.
    #[must_use]
    pub fn new(supervisor: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>) -> Self {
        Self {
            supervisor,
            python_preparation: Arc::new(Mutex::new(None)),
            max_request_body: MAX_TENANT_BODY_BYTES,
            version_pins: None,
            service_invocations: None,
            workflow_quarantine: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            test_endpoint: None,
            #[cfg(test)]
            test_generation: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_endpoint(socket: &Path) -> Self {
        let mut transport = Self::new(Arc::new(Mutex::new(None)));
        transport.test_endpoint = Some(socket.to_owned());
        transport.test_generation = Some(open_compute_core::StartupId::generate());
        transport
    }

    pub(super) async fn request(
        &self,
        socket: &Path,
        mut request: Request,
    ) -> Result<hyper::Response<hyper::body::Incoming>, Box<dyn std::error::Error + Send + Sync>>
    {
        request
            .headers_mut()
            .entry(header::HOST)
            .or_insert(HeaderValue::from_static("runtime"));
        *request.uri_mut() = request
            .uri()
            .path_and_query()
            .map_or("/", hyper::http::uri::PathAndQuery::as_str)
            .parse()?;
        let stream = tokio::net::UnixStream::connect(socket).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            let _ = connection.with_upgrades().await;
        });
        Ok(sender.send_request(request).await?)
    }

    /// Reduce the request body ceiling for bounded transport fault fixtures.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_test_request_body_limit(mut self, max_request_body: usize) -> Self {
        self.max_request_body = max_request_body.clamp(1, MAX_TENANT_BODY_BYTES);
        self
    }

    /// Attach the conservative execution-lifetime authority used for version deletion.
    #[must_use]
    pub fn with_version_pins(mut self, pins: VersionPins) -> Self {
        self.version_pins = Some(pins);
        self
    }

    /// Attach Service invocation ownership for native WebSocket handoffs.
    #[must_use]
    pub fn with_service_invocations(
        mut self,
        registry: crate::service_invocations::ServiceInvocationRegistry,
    ) -> Self {
        self.service_invocations = Some(registry);
        self
    }

    pub(crate) fn configure_python_preparation(
        &self,
        preparation: python_preparation::PythonPreparation,
    ) -> Result<(), PlatformError> {
        let mut slot = self
            .python_preparation
            .lock()
            .map_err(|_| runtime_unavailable())?;
        if slot.is_some() {
            return Err(runtime_unavailable());
        }
        *slot = Some(Arc::new(preparation));
        Ok(())
    }

    /// Rotate the single supervised runtime generation and wait for readiness.
    pub async fn rotate_generation(&self, timeout: Duration) -> Result<(), PlatformError> {
        let supervisor = self
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(runtime_unavailable)?;
        supervisor.rotate_generation(timeout).await
    }

    /// Dispatch a public request to an already-frozen version target.
    pub async fn dispatch(
        &self,
        target: DispatchTarget,
        request: Request,
    ) -> Result<Response, PlatformError> {
        self.send(target, request, false, false, false).await
    }

    /// Execute one trusted native facet delete after the control-plane fence commits.
    pub async fn delete_durable_object(
        &self,
        authority: &AuthorizedDurableObjectDelete,
    ) -> Result<(), PlatformError> {
        let endpoint = self.endpoint()?;
        let body = serde_json::to_vec(authority).map_err(|_| runtime_unavailable())?;
        let request = hyper::Request::builder()
            .method(Method::POST)
            .uri("http://runtime/internal/do-delete")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .map_err(|_| runtime_unavailable())?;
        let response = tokio::time::timeout(
            RESPONSE_HEADER_TIMEOUT,
            self.request(&endpoint.socket_path, request),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DoDispatchTimeout,
                "Durable Object delete result is unknown",
            )
        })?
        .map_err(|_| runtime_unavailable())?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(PlatformError::new(
                ErrorCode::DoStorageUnavailable,
                "Durable Object native delete did not complete",
            ))
        }
    }

    /// Deliver one scheduler claim through the private private alarm path.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn dispatch_alarm(
        &self,
        job: &ClaimedJob,
        timeout: Duration,
    ) -> Result<AlarmDispatchResult, PlatformError> {
        let result = self
            .alarm_request(
                "/internal/do-alarm",
                &AlarmObjectRequest {
                    namespace_resource_id: job.namespace_resource_id,
                    object_id: job.object_id,
                    object_generation: job.object_generation,
                    row_token: Some(&job.row_token),
                    retry_count: Some(job.retry_count),
                },
                timeout,
            )
            .await?;
        validate_alarm_dispatch_result(result)
    }

    /// Deliver one scheduler claim while the scheduler clock owns the timeout.
    pub(crate) async fn dispatch_alarm_unbounded(
        &self,
        job: &ClaimedJob,
    ) -> Result<AlarmDispatchResult, PlatformError> {
        let result = self
            .alarm_request_unbounded(
                "/internal/do-alarm",
                &AlarmObjectRequest {
                    namespace_resource_id: job.namespace_resource_id,
                    object_id: job.object_id,
                    object_generation: job.object_generation,
                    row_token: Some(&job.row_token),
                    retry_count: Some(job.retry_count),
                },
            )
            .await?;
        validate_alarm_dispatch_result(result)
    }

    /// Probe an Alarm projection while the scheduler clock owns the timeout.
    pub(crate) async fn repair_alarm_unbounded(
        &self,
        namespace_resource_id: open_compute_core::ResourceId,
        object_id: open_compute_core::DurableObjectId,
        object_generation: u64,
    ) -> Result<AlarmRepairResult, PlatformError> {
        let result = self
            .alarm_request_unbounded(
                "/internal/do-alarm-repair",
                &AlarmObjectRequest {
                    namespace_resource_id,
                    object_id,
                    object_generation,
                    row_token: None,
                    retry_count: None,
                },
            )
            .await?;
        validate_alarm_repair_result(result)
    }

    /// Start a rollout replacement through the private native host.
    pub(crate) async fn start_container(
        &self,
        namespace_resource_id: open_compute_core::ResourceId,
        object_id: open_compute_core::DurableObjectId,
        object_generation: u64,
    ) -> Result<(), PlatformError> {
        let response: ContainerStartResult = tokio::time::timeout(
            RESPONSE_HEADER_TIMEOUT,
            self.alarm_request_unbounded(
                "/internal/container-start",
                &ContainerStartRequest {
                    namespace_resource_id,
                    object_id,
                    object_generation,
                },
            ),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DoDispatchTimeout,
                "Container replacement start result is unknown",
            )
        })?
        .map_err(|_| runtime_unavailable())?;
        if response.started {
            Ok(())
        } else {
            Err(runtime_unavailable())
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn alarm_request<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &AlarmObjectRequest<'_>,
        timeout: Duration,
    ) -> Result<T, PlatformError> {
        tokio::time::timeout(timeout, self.alarm_request_unbounded(path, body))
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::DoDispatchTimeout,
                    "Durable Object alarm dispatch result is unknown",
                )
            })?
    }

    async fn alarm_request_unbounded<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, PlatformError> {
        let endpoint = self.endpoint()?;
        let bytes = serde_json::to_vec(body).map_err(|_| alarm_protocol_error())?;
        let request = hyper::Request::builder()
            .method(Method::POST)
            .uri(format!("http://runtime{path}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .map_err(|_| alarm_protocol_error())?;
        let response = self
            .request(&endpoint.socket_path, request)
            .await
            .map_err(|_| runtime_unavailable())?;
        if !response.status().is_success() {
            #[cfg(any(test, feature = "test-support"))]
            {
                let code = response
                    .headers()
                    .get(ERROR_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .filter(|code| {
                        matches!(
                            *code,
                            "DO_VERSION_STALE"
                                | "DO_OBJECT_DELETING"
                                | "DO_INTERNAL_PROTOCOL_ERROR"
                                | "DO_STORAGE_UNAVAILABLE"
                                | "DO_RUNTIME_EXCEPTION"
                                | "CONTAINER_UNAVAILABLE"
                                | "VERSION_INVARIANT_VIOLATION"
                                | "SCHEDULER_INTERNAL_PROTOCOL_ERROR"
                                | "DO_ID_INVALID"
                        )
                    })
                    .unwrap_or("UNRECOGNIZED_CODE");
                tracing::info!(
                    status = response.status().as_u16(),
                    code,
                    "Controlled native DO dispatch rejection"
                );
            }
            return Err(PlatformError::new(
                ErrorCode::DoStorageUnavailable,
                "Durable Object alarm dispatch failed",
            ));
        }
        let bytes = to_bytes(Body::new(response.into_body()), 4096)
            .await
            .map_err(|_| alarm_protocol_error())?;
        serde_json::from_slice(&bytes).map_err(|_| alarm_protocol_error())
    }

    /// Prove that a named module export exists without invoking tenant `fetch()`.
    pub async fn probe_entrypoint(
        &self,
        candidate: ValidationCandidate,
        entrypoint: String,
    ) -> Result<(), PlatformError> {
        self.validate_candidate(candidate, Some(entrypoint), true)
            .await
    }

    async fn validate_candidate(
        &self,
        candidate: ValidationCandidate,
        entrypoint: Option<String>,
        probe: bool,
    ) -> Result<(), PlatformError> {
        let target = DispatchTarget {
            instance_id: candidate.instance_id,
            worker_id: candidate.worker_id,
            version_id: candidate.version_id,
            worker_code_sha256: hex::encode(candidate.worker_code_sha256),
            entrypoint,
            route_generation: 0,
            request_id: RequestId::generate(),
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::empty())
            .map_err(|_| runtime_unavailable())?;
        let response = self.send(target, request, true, false, probe).await?;
        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            StatusCode::NOT_FOUND => Err(PlatformError::new(
                ErrorCode::EntrypointNotFound,
                "named entrypoint was not found",
            )),
            StatusCode::UNPROCESSABLE_ENTITY => Err(PlatformError::new(
                ErrorCode::BundleRuntimeInvalid,
                "real workerd rejected version startup",
            )),
            StatusCode::CONFLICT => Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "real runtime validation reported a non-runnable version",
            )),
            StatusCode::INTERNAL_SERVER_ERROR => Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "real runtime validation failed internally",
            )),
            _ => Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "real runtime validation returned an unexpected status",
            )),
        }
    }

    pub(super) fn endpoint(&self) -> Result<EndpointSnapshot, PlatformError> {
        #[cfg(test)]
        if let Some(socket_path) = &self.test_endpoint {
            return Ok(EndpointSnapshot {
                socket_path: socket_path.clone(),
                startup_id: self.test_generation,
            });
        }
        let supervisor = self
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(runtime_unavailable)?;
        let snapshot = supervisor.snapshot();
        if snapshot.state != SupervisorState::Running {
            return Err(runtime_unavailable());
        }
        let socket_path = snapshot.ingress_socket.ok_or_else(runtime_unavailable)?;
        Ok(EndpointSnapshot {
            socket_path,
            startup_id: snapshot.startup_id,
        })
    }
}

pub(super) fn validate_alarm_dispatch_result(
    result: AlarmDispatchResult,
) -> Result<AlarmDispatchResult, PlatformError> {
    let schedule_valid = result.scheduled_time_ms.is_some_and(|value| value > 0)
        && result.retry_count.is_some_and(|value| value <= 6);
    let shape_valid = match result.outcome {
        AlarmDispatchOutcome::Success | AlarmDispatchOutcome::Stale => {
            result.scheduled_time_ms.is_none()
                && result.retry_count.is_none()
                && result.error_code.is_none()
        }
        AlarmDispatchOutcome::NotDue => schedule_valid && result.error_code.is_none(),
        AlarmDispatchOutcome::Retry => {
            schedule_valid && result.error_code.as_deref() == Some("DO_RUNTIME_EXCEPTION")
        }
        AlarmDispatchOutcome::Exhausted => {
            result.scheduled_time_ms.is_none()
                && result.retry_count.is_none()
                && result.error_code.as_deref() == Some("DO_RUNTIME_EXCEPTION")
        }
    };
    shape_valid
        .then_some(result)
        .ok_or_else(alarm_protocol_error)
}

pub(super) fn validate_queue_dispatch_result(
    result: QueueDispatchResult,
    message_count: usize,
) -> Result<QueueDispatchResult, PlatformError> {
    let bounded = message_count > 0
        && message_count <= 100
        && result.explicit_acks.len() <= message_count
        && result.retry_messages.len() <= message_count;
    let outcome = matches!(
        result.outcome.as_str(),
        "ok" | "exception"
            | "canceled"
            | "killSwitch"
            | "daemonDown"
            | "exceededCpu"
            | "exceededMemory"
            | "loadShed"
            | "responseStreamDisconnected"
            | "scriptNotFound"
            | "internalError"
            | "exceededWallTime"
            | "aborted"
            | "unknown"
    );
    let delay = |value: Option<i64>| value.is_none_or(|value| (0..=86_400).contains(&value));
    let decisions = delay(result.retry_batch.delay_seconds)
        && result
            .retry_messages
            .iter()
            .all(|decision| !decision.msg_id.is_empty() && delay(decision.delay_seconds))
        && result
            .explicit_acks
            .iter()
            .all(|id| !id.is_empty() && id.len() <= 128);
    (bounded && outcome && decisions)
        .then_some(result)
        .ok_or_else(queue_protocol_error)
}

pub(super) fn validate_queue_dispatch_request(
    request: &QueueDispatchRequest,
) -> Result<(), PlatformError> {
    if request.queue_name.is_empty()
        || request.queue_name.len() > 128
        || request.queue_name.chars().any(char::is_control)
        || request.messages.is_empty()
        || request.messages.len() > 100
    {
        return Err(queue_protocol_error());
    }
    let mut identities = HashSet::with_capacity(request.messages.len());
    let mut total = 0_usize;
    for message in &request.messages {
        let id: QueueMessageId = message.id.parse().map_err(|_| queue_protocol_error())?;
        if !identities.insert(id)
            || message.timestamp_ms < 0
            || !(1..=101).contains(&message.attempts)
        {
            return Err(queue_protocol_error());
        }
        let body = base64::engine::general_purpose::STANDARD
            .decode(&message.body_base64)
            .map_err(|_| queue_protocol_error())?;
        if u64::try_from(body.len()).map_err(|_| queue_protocol_error())? > QUEUE_MAX_MESSAGE_BYTES
        {
            return Err(queue_protocol_error());
        }
        match message.content_type {
            QueueContentType::Json => {
                serde_json::from_slice::<serde_json::Value>(&body)
                    .map_err(|_| queue_protocol_error())?;
            }
            QueueContentType::Text => {
                std::str::from_utf8(&body).map_err(|_| queue_protocol_error())?;
            }
            QueueContentType::Bytes | QueueContentType::V8 => {}
        }
        total = total
            .checked_add(message.body_base64.len())
            .ok_or_else(queue_protocol_error)?;
    }
    if total > MAX_QUEUE_CUSTOM_EVENT_REQUEST {
        return Err(queue_protocol_error());
    }
    let metrics = &request.metadata.metrics;
    if metrics.oldest_message_timestamp_ms == Some(0)
        || metrics
            .oldest_message_timestamp_ms
            .is_some_and(|value| value < 0)
    {
        return Err(queue_protocol_error());
    }
    Ok(())
}

pub(super) fn validate_scheduled_dispatch_request(
    request: &ScheduledDispatchRequest,
) -> Result<(), PlatformError> {
    if request.scheduled_time_ms < 0 || request.scheduled_time_ms % 60_000 != 0 {
        return Err(PlatformError::new(
            ErrorCode::CronActivationStale,
            "Cron logical slot is invalid",
        ));
    }
    CronSchedule::parse(&request.cron)?;
    if (!request.scheduled_handler && request.workflow_bindings.is_empty())
        || request.workflow_bindings.len() > 100
        || request
            .workflow_bindings
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || request
            .workflow_bindings
            .iter()
            .any(|name| name.len() > 64 || validate_env_name(name).is_err())
    {
        return Err(PlatformError::new(
            ErrorCode::CronActivationStale,
            "Cron activation target is invalid",
        ));
    }
    Ok(())
}

pub(super) fn validate_scheduled_dispatch_result(
    result: ScheduledDispatchResult,
) -> Result<ScheduledDispatchResult, PlatformError> {
    matches!(
        result.outcome.as_str(),
        "ok" | "exception"
            | "canceled"
            | "killSwitch"
            | "daemonDown"
            | "exceededCpu"
            | "exceededMemory"
            | "loadShed"
            | "responseStreamDisconnected"
            | "scriptNotFound"
            | "internalError"
            | "exceededWallTime"
            | "aborted"
            | "unknown"
    )
    .then_some(result)
    .ok_or_else(custom_event_protocol_error)
}

pub(super) fn validate_alarm_repair_result(
    result: AlarmRepairResult,
) -> Result<AlarmRepairResult, PlatformError> {
    let shape_valid = if result.exists {
        result.scheduled_time_ms.is_some_and(|value| value > 0)
            && result.retry_count.is_some_and(|value| value <= 6)
            && result
                .row_token
                .as_deref()
                .is_some_and(valid_alarm_row_token)
    } else {
        result.scheduled_time_ms.is_none()
            && result.retry_count.is_none()
            && result.row_token.is_none()
    };
    shape_valid
        .then_some(result)
        .ok_or_else(alarm_protocol_error)
}

fn valid_alarm_row_token(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|token| token.get_version() == Some(uuid::Version::Random))
}

impl RuntimeValidator for WorkerdTransport {
    fn validate(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move {
            let preparation = self
                .python_preparation
                .lock()
                .map_err(|_| runtime_unavailable())?
                .clone();
            if let Some(preparation) = preparation {
                preparation.prepare(candidate.clone(), self.clone()).await?;
            }
            self.validate_candidate(candidate, None, false).await
        })
    }

    fn validate_deployment(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<
        Box<dyn Future<Output = Result<open_compute_core::StartupId, PlatformError>> + Send + '_>,
    > {
        Box::pin(async move {
            let before = self.current_generation().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "runtime generation is unavailable before deployment admission",
                )
            })?;
            self.validate_candidate(candidate, None, true).await?;
            if self.current_generation() != Some(before) {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "runtime generation changed during deployment admission",
                ));
            }
            Ok(before)
        })
    }

    fn current_generation(&self) -> Option<open_compute_core::StartupId> {
        #[cfg(test)]
        if self.test_endpoint.is_some() {
            return self.test_generation;
        }

        self.supervisor
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|supervisor| supervisor.snapshot()))
            .filter(|snapshot| snapshot.state == SupervisorState::Running)
            .and_then(|snapshot| snapshot.startup_id)
    }

    fn revoke_worker_loader_prefix(
        &self,
        prefix: String,
        expected_generation: open_compute_core::StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move {
            let result = self
                .revoke_worker_loaders(&[prefix], Some(expected_generation))
                .await;
            if result.is_err() {
                self.recover_worker_loader_revocation(expected_generation)
                    .await?;
            }
            result
        })
    }

    fn recover_worker_loader_revocation(
        &self,
        generation: open_compute_core::StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move {
            if self.current_generation() == Some(generation) {
                self.rotate_generation(Duration::from_secs(30)).await?;
            }
            Ok(())
        })
    }

    fn validate_entrypoint(
        &self,
        candidate: ValidationCandidate,
        entrypoint: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move { self.probe_entrypoint(candidate, entrypoint).await })
    }

    fn validate_durable_object_class(
        &self,
        candidate: ValidationCandidate,
        class_name: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move {
            let target = DispatchTarget {
                instance_id: candidate.instance_id,
                worker_id: candidate.worker_id,
                version_id: candidate.version_id,
                worker_code_sha256: hex::encode(candidate.worker_code_sha256),
                entrypoint: Some(class_name),
                route_generation: 0,
                request_id: RequestId::generate(),
            };
            let request = Request::builder()
                .method(Method::POST)
                .uri("/")
                .body(Body::empty())
                .map_err(|_| runtime_unavailable())?;
            let response = self.send(target, request, true, true, false).await?;
            match response.status() {
                StatusCode::NO_CONTENT => Ok(()),
                StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
                    Err(PlatformError::new(
                        ErrorCode::DoClassNotFound,
                        "Durable Object class was not found",
                    ))
                }
                _ => Err(runtime_unavailable()),
            }
        })
    }

    fn validate_workflow(
        &self,
        target: open_compute_storage::workflows::WorkflowTarget,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move { self.probe_workflow(&target).await })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContainerStartRequest {
    namespace_resource_id: open_compute_core::ResourceId,
    object_id: open_compute_core::DurableObjectId,
    object_generation: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerStartResult {
    started: bool,
}

#[cfg(test)]
#[path = "container_transport_tests.rs"]
mod container_transport_tests;
