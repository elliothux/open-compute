use super::*;

impl crate::binding_backend::KvBindingExecutor for SqliteKvBindingExecutor {
    fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    fn stream_limits(&self) -> (u32, u32) {
        (self.streams.limit, self.streams_per_namespace)
    }

    /// Execute one already-authorized KV command.
    fn execute(
        &self,
        binding: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        let result = (|| {
            let reservation_bytes = match &command {
                KvCommand::Put { value, .. } => Some(value.len() as u64 + 64 * 1024),
                KvCommand::PutStaged { value, .. } => Some(value.length as u64 + 64 * 1024),
                _ => None,
            };
            let admission = reservation_bytes
                .map(|bytes| self.storage.reserve_mutation(bytes))
                .transpose();
            if let Some(metrics) = &self.metrics
                && reservation_bytes.is_some()
            {
                metrics.observe_admission(
                    OperationClass::Kv,
                    admission.as_ref().err().map(PlatformError::code),
                );
            }
            let _admission = admission?;
            let mutation = matches!(
                &command,
                KvCommand::Put { .. } | KvCommand::PutStaged { .. } | KvCommand::Delete { .. }
            );
            let _connection = self.connections.acquire(self.operation_timeout)?;
            let _connection_metric = self.metrics.as_ref().map(|metrics| {
                KvGaugeGuard::new(
                    metrics,
                    if mutation {
                        KvGauge::WriterConnection
                    } else {
                        KvGauge::ReaderConnection
                    },
                )
            });
            let (handle, now_ms) = self.open_handle(binding)?;
            let _writer = mutation.then(|| {
                handle
                    .writer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            });
            let _reader = (!mutation)
                .then(|| handle.readers.acquire(self.operation_timeout))
                .transpose()?;
            KvNamespaceRepository::new(self.storage.db())
                .record_open(binding.resource.id, now_ms)?;
            let result = self.execute_inner(binding, &handle.engine, now_ms, command);
            handle.touch(self.effective_now_ms());
            result
        })();
        if let (Some(metrics), Err(error)) = (&self.metrics, &result) {
            metrics.observe_product_error(OperationClass::Kv, error.code());
        }
        self.isolate_failure(binding, &result);
        result
    }

    /// Stream a single get through bounded global and per-namespace slots.
    fn stream_get(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        cache_ttl: Option<u64>,
        sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        let result = (|| {
            if cache_ttl.is_some_and(|value| value < KV_MIN_CACHE_TTL_SECONDS) {
                return Err(invalid_options());
            }
            let _connection = self.connections.acquire(self.operation_timeout)?;
            let _global_stream = self.streams.acquire(self.operation_timeout)?;
            let _connection_metric = self
                .metrics
                .as_ref()
                .map(|metrics| KvGaugeGuard::new(metrics, KvGauge::ReaderConnection));
            let _stream_metric = self
                .metrics
                .as_ref()
                .map(|metrics| KvGaugeGuard::new(metrics, KvGauge::ActiveStream));
            let (handle, now_ms) = self.open_handle(binding)?;
            let _resource_stream = handle.streams.acquire(self.operation_timeout)?;
            KvNamespaceRepository::new(self.storage.db())
                .record_open(binding.resource.id, now_ms)?;
            let sink = RefCell::new(sink);
            let result = handle.engine.stream_get(
                key,
                now_ms,
                |entry| (sink.borrow_mut())(KvStreamPart::Entry(entry)),
                |bytes| (sink.borrow_mut())(KvStreamPart::Bytes(bytes.to_vec())),
            );
            handle.touch(self.effective_now_ms());
            result
        })();
        if let (Some(metrics), Err(error)) = (&self.metrics, &result) {
            metrics.observe_product_error(OperationClass::Kv, error.code());
        }
        self.isolate_failure(binding, &result);
        result
    }
}
