use super::*;

/// P0.2 version orchestrator over typed P0.1 capabilities.
pub struct VersionController<'a> {
    pub(super) storage: &'a PlatformStorage,
    pub(super) artifacts: ArtifactStore,
    pub(super) validator: Arc<dyn RuntimeValidator>,
    pub(super) bundle_limits: BundleLimits,
    pub(super) max_queue_consumer_concurrency: u32,
    pub(super) product_promoter: Option<Arc<dyn ProductPromotionCoordinator>>,
    pub(super) durable_object_migration: Option<DurableObjectMigrationPlan>,
}

impl std::fmt::Debug for VersionController<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionController")
            .field("artifacts", &self.artifacts)
            .field("bundle_limits", &self.bundle_limits)
            .finish_non_exhaustive()
    }
}

impl<'a> VersionController<'a> {
    /// Bind storage, immutable artifacts, and a real runtime validator.
    #[must_use]
    pub fn new(
        storage: &'a PlatformStorage,
        artifacts: ArtifactStore,
        validator: Arc<dyn RuntimeValidator>,
        bundle_limits: BundleLimits,
    ) -> Self {
        Self {
            storage,
            artifacts,
            validator,
            bundle_limits,
            max_queue_consumer_concurrency: DEFAULT_MAX_QUEUE_CONSUMER_CONCURRENCY,
            product_promoter: None,
            durable_object_migration: None,
        }
    }

    /// Apply the validated operator-local Queue consumer concurrency ceiling.
    #[must_use]
    pub fn with_queue_consumer_limit(mut self, maximum: u32) -> Self {
        self.max_queue_consumer_concurrency = maximum.max(1);
        self
    }

    /// Attach the single-process Queue/Cron cross-database promotion owner.
    #[must_use]
    pub fn with_product_promoter(mut self, promoter: Arc<dyn ProductPromotionCoordinator>) -> Self {
        self.product_promoter = Some(promoter);
        self
    }

    /// Attach the prepared Durable Object plan published by this Version's ready transition.
    #[must_use]
    pub fn with_durable_object_migration(mut self, plan: DurableObjectMigrationPlan) -> Self {
        self.durable_object_migration = Some(plan);
        self
    }

    /// Execute upload, immutable DB transaction, runtime validation, and optional promotion.
    pub async fn create_version(
        &self,
        request: CreateVersionRequest,
    ) -> Result<CreateVersionOutcome, PlatformError> {
        self.create_version_with_id(request, None).await
    }

    /// Finalize a resumable upload using the version identity persisted before validation.
    pub async fn finalize_upload(
        &self,
        request: CreateVersionRequest,
        version_id: VersionId,
    ) -> Result<CreateVersionOutcome, PlatformError> {
        self.create_version_with_id(request, Some(version_id)).await
    }

    async fn create_version_with_id(
        &self,
        request: CreateVersionRequest,
        version_id: Option<VersionId>,
    ) -> Result<CreateVersionOutcome, PlatformError> {
        validate_idempotency_key(&request.idempotency_key)?;
        let content = PreparedContent::prepare(&request.content, self.bundle_limits)?;
        let (canonical_vars, stored_vars) = canonicalize_vars(request.vars.clone())?;
        validate_secret_set(&request.secrets, &canonical_vars)?;
        validate_binding_set(&request.bindings, &canonical_vars, &request.secrets)?;
        validate_service_set(
            &request.services,
            &canonical_vars,
            &request.secrets,
            &request.bindings,
        )?;
        if let Some(bundle) = content.bundle() {
            validate_injection_module_collisions(bundle.manifest())?;
        }
        validate_asset_content(&request, &content, &canonical_vars)?;
        validate_product_counts(&request)?;
        let repo = WorkerRepository::new(self.storage.db());
        // Authentication/account scoping happens before reserving a key, so a
        // nonexistent target cannot strand a running idempotency row.
        repo.get_worker(request.account_id, request.worker_id)?;
        let fingerprint_input = request_fingerprint(
            &request,
            &content,
            &canonical_vars,
            version_id,
            self.durable_object_migration.as_ref(),
        )?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&fingerprint_input);
        let reservation = repo.reserve_idempotency(
            request.account_id,
            "version.create",
            &request.idempotency_key,
            self.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
        )?;
        let recover_running = matches!(reservation, IdempotencyReservation::Running);
        match reservation {
            IdempotencyReservation::Complete(response) => {
                return Ok(CreateVersionOutcome::Replay(response));
            }
            IdempotencyReservation::Running if version_id.is_none() => {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "the same idempotent operation is still running",
                ));
            }
            IdempotencyReservation::Failed(response) => {
                let failed: FailedResponse =
                    serde_json::from_slice(&response).map_err(|_| invariant())?;
                return Err(PlatformError::new(
                    ErrorCode::from_stable_str(&failed.code).unwrap_or(ErrorCode::Internal),
                    "idempotent version operation previously failed",
                ));
            }
            IdempotencyReservation::Running | IdempotencyReservation::Reserved => {}
        }

        let fixed_version_id = version_id.unwrap_or_else(VersionId::generate);
        let migration_preflight = self
            .durable_object_migration
            .as_ref()
            .map(|plan| {
                DurableObjectRepository::new(self.storage).validate_worker_migration_version(
                    request.worker_id,
                    fixed_version_id,
                    plan,
                )
            })
            .transpose();
        let operation = if let Err(error) = migration_preflight {
            Err(error)
        } else if recover_running {
            self.resume_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_version_id,
            )
            .await
        } else {
            self.create_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_version_id,
            )
            .await
        };
        match operation {
            Ok(result) => {
                let response = serde_json::to_vec(&serde_json::json!({
                    "version": result.version.to_api_json(),
                    "deployment": result.deployment,
                }))
                .map_err(|_| invariant())?;
                repo.complete_idempotency_with_version_ref(
                    request.account_id,
                    "version.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                    result.version.id,
                    &idempotency_ref_id(
                        request.account_id,
                        "version.create",
                        &request.idempotency_key,
                    ),
                    request.now_ms,
                )?;
                Ok(CreateVersionOutcome::Applied(result))
            }
            Err(error) => {
                let response = serde_json::to_vec(&FailedResponse {
                    code: error.code().as_str().to_owned(),
                })
                .map_err(|_| invariant())?;
                repo.fail_idempotency(
                    request.account_id,
                    "version.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                )?;
                Err(error)
            }
        }
    }

    async fn create_reserved(
        &self,
        request: &CreateVersionRequest,
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        version_id: VersionId,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let compatibility_flags = validate_compatibility(&request.runtime_features)?;
        let _admission = self.storage.reserve_mutation(content.admission_bytes()?)?;
        let (stored_secrets, secret_descriptors) = self.encrypt_secrets(
            request.account_id,
            request.worker_id,
            version_id,
            &request.secrets,
        )?;
        let PreparedBindings {
            descriptors: binding_descriptors,
            rows: stored_bindings,
            artifact_rows: stored_artifact_bindings,
            queue_descriptors: queue_binding_descriptors,
            queue_rows: stored_queue_bindings,
            workflow_descriptors: workflow_binding_descriptors,
            workflow_rows: stored_workflow_bindings,
            durable_object_classes,
            service_descriptors,
            service_rows,
        } = self.prepare_bindings(request, version_id)?;
        let queue_consumers = self.prepare_queue_consumers(request)?;
        let cron = prepare_cron_config(request, &workflow_binding_descriptors)?;
        let (cache_policy, mut cache_rows, builtin_descriptors, builtin_rows) =
            prepare_runtime_features(&request.runtime_features)?;
        let mut builtin_names = HashSet::new();
        for descriptor in &builtin_descriptors {
            if !builtin_names.insert(descriptor.name.as_str())
                || canonical_vars.contains_key(&descriptor.name)
                || request.secrets.contains_key(&descriptor.name)
                || request.bindings.contains_key(&descriptor.name)
                || request.services.contains_key(&descriptor.name)
                || content
                    .assets()
                    .and_then(|assets| assets.routing.binding.as_deref())
                    == Some(descriptor.name.as_str())
            {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "runtime binding names must be unique",
                ));
            }
            let expected_type = match descriptor.kind {
                BuiltinBindingDescriptorKindV1::WasmModule => Some(crate::ModuleType::Wasm),
                BuiltinBindingDescriptorKindV1::TextBlob => Some(crate::ModuleType::Text),
                BuiltinBindingDescriptorKindV1::DataBlob => Some(crate::ModuleType::Data),
                _ => None,
            };
            if let Some(expected_type) = expected_type {
                let module_name = descriptor.tag.as_deref().ok_or_else(invariant)?;
                if !content.bundle().is_some_and(|bundle| {
                    bundle.manifest().modules.iter().any(|module| {
                        module.name == module_name && module.module_type == expected_type
                    })
                }) {
                    return Err(PlatformError::new(
                        ErrorCode::BundleInvalid,
                        "service-worker module binding does not match a canonical bundle part",
                    ));
                }
            }
        }
        let descriptor = WorkerCodeDescriptorV1::new(
            request.account_id,
            request.worker_id,
            version_id,
            request.now_ms,
            request.runtime_features.compatibility_date.clone(),
            compatibility_flags.clone(),
            content
                .bundle()
                .map(|bundle| (bundle.sha256(), bundle.manifest())),
            content
                .assets()
                .map(|assets| (&assets.manifest, &assets.routing)),
            canonical_vars,
            secret_descriptors,
            binding_descriptors,
            queue_binding_descriptors,
            workflow_binding_descriptors,
            service_descriptors,
            cache_policy,
            builtin_descriptors,
            u32::try_from(LOADER_SCHEMA_VERSION).map_err(|_| invariant())?,
        )?;
        if content.kind() == VersionContentKind::AssetsOnly {
            cache_rows.clear();
        }
        let descriptor_hash = descriptor.sha256()?;
        let artifact_reservation = self.artifacts.reserve_version_artifact().await;
        let bundle_identity = if let Some(bundle) = content.bundle() {
            let size = bundle.size()?;
            let artifact = bundle.store(&self.artifacts).await?;
            if artifact.sha256_bytes() != &bundle.sha256() || artifact.size() != size {
                return Err(PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "ArtifactStore returned a different immutable artifact",
                ));
            }
            Some((bundle.sha256(), size, bundle.manifest().main_module.clone()))
        } else {
            None
        };
        let prepared_assets = self.prepare_assets(content.assets()).await?;
        let version = repo.insert_staging_version(
            &NewVersion {
                id: version_id,
                account_id: request.account_id,
                worker_id: request.worker_id,
                content_kind: content.kind(),
                artifact_sha256: bundle_identity.as_ref().map(|value| value.0),
                artifact_size: bundle_identity.as_ref().map(|value| value.1),
                artifact_schema_version: bundle_identity
                    .as_ref()
                    .map(|_| WORKER_BUNDLE_SCHEMA_VERSION),
                main_module: bundle_identity.as_ref().map(|value| value.2.clone()),
                worker_code_sha256: descriptor_hash,
                compatibility_date: request.runtime_features.compatibility_date.clone(),
                compatibility_flags,
                vars: stored_vars,
                secrets: stored_secrets,
                request_id: request.request_id,
                now_ms: request.now_ms,
            },
            &open_compute_storage::NewVersionProducts {
                annotations: Some(&request.runtime_features.annotations),
                assets: prepared_assets.as_ref().map(|value| &value.0),
                asset_object_refs: prepared_assets
                    .as_ref()
                    .map_or(&[], |value| value.1.as_slice()),
                bindings: &stored_bindings,
                artifact_bindings: &stored_artifact_bindings,
                queue_bindings: &stored_queue_bindings,
                workflow_bindings: &stored_workflow_bindings,
                services: &service_rows,
                cache_policies: &cache_rows,
                builtin_bindings: &builtin_rows,
                queue_consumers: &queue_consumers,
                cron: (content.kind() == VersionContentKind::Worker).then_some(&cron),
            },
            self.storage.hardening().max_versions_per_worker,
        )?;
        drop(artifact_reservation);
        let requires_product_promoter =
            !queue_consumers.is_empty() || !cron.declarations.is_empty();
        let queue_entrypoints: Vec<Option<String>> = queue_consumers
            .iter()
            .map(|consumer| consumer.entrypoint.clone())
            .collect();
        let cache_entrypoints = request
            .runtime_features
            .cache
            .entrypoints
            .iter()
            .filter(|(_, policy)| policy.enabled)
            .map(|(name, _)| Some(name.clone()));
        let queue_entrypoints = queue_entrypoints
            .into_iter()
            .chain(cache_entrypoints)
            .collect();
        self.finish_reserved(
            request,
            version,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn resume_reserved(
        &self,
        request: &CreateVersionRequest,
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        version_id: VersionId,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let version =
            match repo.get_worker_version(request.account_id, request.worker_id, version_id) {
                Ok(version) => version,
                Err(error) if error.code() == ErrorCode::VersionNotFound => {
                    return self
                        .create_reserved(request, content, canonical_vars, stored_vars, version_id)
                        .await;
                }
                Err(error) => return Err(error),
            };
        if version.content_kind != content.kind() || version.deleted_at_ms.is_some() {
            return Err(invariant());
        }
        let mut durable_object_classes = Vec::new();
        for binding in BindingRepository::new(self.storage.db()).version_bindings(version_id)? {
            if binding.kind == BindingKind::DoNamespace {
                let namespace = DurableObjectRepository::new(self.storage)
                    .get_namespace(request.account_id, binding.resource_id)?;
                durable_object_classes.push(namespace.class_name);
            }
        }
        durable_object_classes.sort();
        durable_object_classes.dedup();
        let queue_declarations =
            QueueConsumerRepository::new(self.storage.db()).version_declarations(version_id)?;
        let cron_declarations = open_compute_storage::CronRepository::new(self.storage.db())
            .version_config(version_id)?
            .declarations;
        let requires_product_promoter =
            !queue_declarations.is_empty() || !cron_declarations.is_empty();
        let mut queue_entrypoints = queue_declarations
            .into_iter()
            .map(|consumer| consumer.entrypoint)
            .collect::<Vec<_>>();
        let (cache_policies, _) =
            open_compute_storage::version_runtime_features(self.storage.db(), version_id)?;
        queue_entrypoints.extend(
            cache_policies
                .into_iter()
                .filter(|policy| policy.enabled && policy.entrypoint.is_some())
                .map(|policy| policy.entrypoint),
        );
        self.finish_reserved(
            request,
            version,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn finish_reserved(
        &self,
        request: &CreateVersionRequest,
        mut version: VersionRecord,
        durable_object_classes: Vec<String>,
        queue_entrypoints: Vec<Option<String>>,
        requires_product_promoter: bool,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        if version.state == VersionState::Rejected {
            let code = version
                .rejection_code
                .as_deref()
                .and_then(ErrorCode::from_stable_str)
                .unwrap_or(ErrorCode::BundleRuntimeInvalid);
            return Err(PlatformError::new(
                code,
                "version validation previously failed",
            ));
        }
        if version.state == VersionState::Staging {
            repo.begin_validation(version.id)?;
            version.state = VersionState::Validating;
        }
        let candidate = ValidationCandidate {
            account_id: request.account_id,
            worker_id: request.worker_id,
            version_id: version.id,
            worker_code_sha256: version.worker_code_sha256,
        };
        let validation = if version.state == VersionState::Validating
            && version.content_kind == VersionContentKind::Worker
        {
            self.validator.validate(candidate.clone()).await
        } else {
            Ok(())
        };
        if let Err(err) = validation {
            let code = stable_validation_code(&err);
            repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
            return Err(PlatformError::new(
                code,
                "real workerd validation rejected the version",
            ));
        }
        for class_name in durable_object_classes {
            if version.state != VersionState::Validating {
                break;
            }
            if let Err(error) = self
                .validator
                .validate_durable_object_class(candidate.clone(), class_name)
                .await
            {
                let code = if error.code() == ErrorCode::DoClassNotFound {
                    ErrorCode::DoClassNotFound
                } else {
                    stable_validation_code(&error)
                };
                repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a Durable Object class",
                ));
            }
        }
        for entrypoint in &queue_entrypoints {
            if version.state == VersionState::Validating
                && let Some(entrypoint) = entrypoint
                && let Err(error) = self
                    .validator
                    .validate_entrypoint(candidate.clone(), entrypoint.clone())
                    .await
            {
                let code = stable_validation_code(&error);
                repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a named entrypoint",
                ));
            }
        }
        if version.state == VersionState::Validating {
            if let Some(plan) = &self.durable_object_migration {
                repo.mark_ready_with_durable_object_migration(
                    version.id,
                    request.worker_id,
                    plan,
                    request.now_ms,
                )?;
            } else {
                repo.mark_ready(version.id, request.now_ms)?;
            }
            version.state = VersionState::Ready;
            version.ready_at_ms = Some(request.now_ms);
        }
        let deployment = if let Some(source) = request.deployment_source {
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            if worker.active_version_id == Some(version.id) {
                let deployment_id = worker.active_deployment_id.ok_or_else(invariant)?;
                return Ok(CreateVersionResult {
                    version,
                    deployment: Some(repo.get_worker_deployment(
                        request.account_id,
                        request.worker_id,
                        deployment_id,
                    )?),
                });
            }
            for route in repo.list_worker_routes(request.account_id, request.worker_id)? {
                if let Some(entrypoint) = route.entrypoint {
                    self.validator
                        .validate_entrypoint(candidate.clone(), entrypoint)
                        .await?;
                }
            }
            if let Some(promoter) = &self.product_promoter {
                promoter
                    .promote(ProductPromotionRequest {
                        account_id: request.account_id,
                        worker_id: request.worker_id,
                        version_id: version.id,
                        source,
                        annotations: BTreeMap::new(),
                        request_id: request.request_id,
                        now_ms: request.now_ms,
                    })
                    .await?;
            } else if requires_product_promoter {
                return Err(PlatformError::new(
                    ErrorCode::QueueConsumerProjectionPending,
                    "Queue/Cron promotion coordinator is unavailable",
                ));
            } else {
                repo.create_deployment_checked(
                    request.account_id,
                    request.worker_id,
                    version.id,
                    None,
                    Some(worker.route_generation),
                    source,
                    &BTreeMap::new(),
                    request.request_id,
                    request.now_ms,
                )?;
            }
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            let deployment_id = worker.active_deployment_id.ok_or_else(invariant)?;
            Some(repo.get_worker_deployment(
                request.account_id,
                request.worker_id,
                deployment_id,
            )?)
        } else {
            None
        };
        let result = CreateVersionResult {
            version,
            deployment,
        };
        Ok(result)
    }

    async fn prepare_assets(
        &self,
        assets: Option<&VersionAssets>,
    ) -> Result<Option<(NewVersionAssets, Vec<NewVersionObjectRef>)>, PlatformError> {
        let Some(assets) = assets else {
            return Ok(None);
        };
        assets.manifest.validate()?;
        assets.routing.validate()?;
        let manifest_bytes = assets.manifest.canonical_bytes()?;
        let manifest_digest: [u8; 32] = Sha256::digest(&manifest_bytes).into();
        let manifest_ref = self
            .artifacts
            .put_verified(
                stream::once(async {
                    Ok::<Bytes, std::io::Error>(Bytes::from(manifest_bytes.clone()))
                }),
                &hex::encode(manifest_digest),
                manifest_bytes.len() as u64,
            )
            .await
            .map_err(|error| map_asset_store_error(&error))?;
        if manifest_ref.sha256_bytes() != &manifest_digest {
            return Err(PlatformError::new(
                ErrorCode::AssetIntegrityError,
                "asset manifest identity changed during upload",
            ));
        }
        let mut refs = vec![NewVersionObjectRef {
            kind: VersionObjectKind::AssetManifest,
            sha256: manifest_digest,
            size: manifest_bytes.len() as u64,
        }];
        let mut seen = BTreeMap::<[u8; 32], u64>::new();
        for entry in &assets.manifest.entries {
            let object = entry.artifact_ref()?;
            if let Some(size) = seen.insert(*object.sha256_bytes(), object.size()) {
                if size != object.size() {
                    return Err(PlatformError::new(
                        ErrorCode::AssetManifestInvalid,
                        "one asset digest declares conflicting lengths",
                    ));
                }
                continue;
            }
            self.artifacts
                .download_verified(&object, &mut std::io::sink())
                .await
                .map_err(|error| map_asset_store_error(&error))?;
            refs.push(NewVersionObjectRef {
                kind: VersionObjectKind::AssetBlob,
                sha256: *object.sha256_bytes(),
                size: object.size(),
            });
        }
        Ok(Some((
            NewVersionAssets {
                manifest_sha256: manifest_digest,
                manifest_json: manifest_bytes,
                routing_config_json: assets.routing.canonical_bytes()?,
                binding_name: assets.routing.binding.clone(),
                logical_file_count: u32::try_from(assets.manifest.entries.len())
                    .map_err(|_| invariant())?,
                logical_total_bytes: assets.manifest.total_bytes()?,
            },
            refs,
        )))
    }

    fn encrypt_secrets(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        secrets: &BTreeMap<String, SecretString>,
    ) -> Result<(BTreeMap<String, StoredVersionSecret>, Vec<SecretDescriptor>), PlatformError> {
        let mut stored = BTreeMap::new();
        let mut descriptors = Vec::with_capacity(secrets.len());
        for (name, value) in secrets {
            let revision_id = Uuid::now_v7().to_string();
            let plaintext = SecretBytes::new(value.expose().as_bytes().to_vec());
            let envelope = self.storage.crypto().encrypt(
                &plaintext,
                account_id,
                worker_id,
                version_id,
                name,
                &revision_id,
            )?;
            descriptors.push(SecretDescriptor {
                name: name.clone(),
                revision_id: revision_id.clone(),
                ciphertext_sha256: ciphertext_sha256(&envelope.nonce, &envelope.ciphertext),
            });
            stored.insert(
                name.clone(),
                StoredVersionSecret {
                    name: name.clone(),
                    revision_id,
                    envelope,
                },
            );
        }
        Ok((stored, descriptors))
    }
}
