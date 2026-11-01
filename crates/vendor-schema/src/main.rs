//! Export the same private DTO definitions used by the vendor HTTP handlers.
// Schema inspection does not construct requests or read their fields.
#![allow(dead_code)]
use std::collections::BTreeMap;

// Sorted registration keeps the fragment byte-stable across modules.
type Schemas = BTreeMap<&'static str, serde_json::Value>;

fn insert<T: schemars::JsonSchema>(schemas: &mut Schemas, name: &'static str) {
    let mut settings = schemars::generate::SchemaSettings::draft2020_12();
    settings.inline_subschemas = true;
    let schema = settings.into_generator().into_root_schema_for::<T>();
    schemas.insert(name, schema.to_value());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut schemas = Schemas::new();
    vendor::export(&mut schemas);
    backups::export(&mut schemas);
    d1_resources::export(&mut schemas);
    durable_objects::export(&mut schemas);
    migrations::export(&mut schemas);
    worker_origins::export(&mut schemas);
    consumer_runtime::export(&mut schemas);
    r2_multipart::export(&mut schemas);
    serde_json::to_writer(std::io::stdout().lock(), &schemas)?;
    Ok(())
}

mod vendor {
    use serde::Serialize;
    use std::collections::BTreeMap;
    include!("../../service/src/cloudflare_v4/vendor_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<Capabilities<'static>>(schemas, "Capabilities");
        super::insert::<SystemStatus>(schemas, "SystemStatus");
        super::insert::<SchedulerStatus>(schemas, "SchedulerStatus");
        super::insert::<CacheStatus>(schemas, "CacheStatus");
        super::insert::<ImageCapacity>(schemas, "ImageCapacity");
        super::insert::<super::upgrade::UpgradeCheckResult>(schemas, "UpgradeCheck");
    }
}

mod backups {
    use serde::{Deserialize, Serialize};
    include!("../../service/src/cloudflare_v4/vendor/backups_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<RestoreRequest>(schemas, "RestoreRequest");
        super::insert::<RestoredResource>(schemas, "RestoredResource");
        super::insert::<Backup>(schemas, "Backup");
    }
}

mod d1_resources {
    use serde::{Deserialize, Serialize};
    include!("../../service/src/cloudflare_v4/vendor/d1_resources_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<RenameRequest>(schemas, "D1RenameRequest");
        super::insert::<RenameResult>(schemas, "D1RenameResult");
        super::insert::<CheckpointResult>(schemas, "D1CheckpointTimes");
    }
}

mod durable_objects {
    use serde::Serialize;
    include!("../../service/src/cloudflare_v4/vendor/durable_objects_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<DurableObjectNamespace>(schemas, "DurableObjectNamespace");
        super::insert::<DurableObjectRecord>(schemas, "DurableObjectRecord");
        super::insert::<DurableObjectNamespacePage>(schemas, "DurableObjectNamespacePage");
        super::insert::<DurableObjectRecordPage>(schemas, "DurableObjectRecordPage");
    }
}

mod migrations {
    use serde::{Deserialize, Serialize};
    include!("../../service/src/cloudflare_v4/vendor/migrations_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<MigrationInput>(schemas, "D1MigrationInput");
        super::insert::<MigrationOutput>(schemas, "D1Migration");
    }
}

mod worker_origins {
    use serde::{Deserialize, Serialize};
    include!("../../service/src/cloudflare_v4/vendor/worker_origins_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<Option<PublicOriginBinding>>(schemas, "PublicOriginBindingResult");
        super::insert::<PublicOriginInput>(schemas, "PublicOriginRequest");
        super::insert::<WorkerEndpoint>(schemas, "WorkerEndpoint");
    }
}

mod consumer_runtime {
    use serde::Serialize;
    include!("../../service/src/cloudflare_v4/queues/consumers_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<ConsumerRuntimeResponse>(schemas, "QueueConsumerRuntime");
    }
}

mod r2_multipart {
    use open_compute_artifacts::{R2HttpMetadata, R2UploadedPart};
    use serde::Deserialize;
    use std::collections::BTreeMap;
    include!("../../service/src/cloudflare_v4/r2/multipart_models.rs");
    pub(super) fn export(schemas: &mut super::Schemas) {
        super::insert::<CreateBody>(schemas, "R2MultipartCreateRequest");
        super::insert::<CompleteBody>(schemas, "R2MultipartCompleteRequest");
        super::insert::<R2UploadedPart>(schemas, "R2MultipartPart");
        super::insert::<open_compute_artifacts::R2ObjectMetadata>(
            schemas,
            "R2MultipartCompletedObject",
        );
        super::insert::<open_compute_storage::r2_objects::R2BucketUsage>(schemas, "R2BucketUsage");
        super::insert::<super::multipart_created::MultipartCreated>(schemas, "R2MultipartCreate");
    }
}

// The shared result type is public in the service library.
#[allow(unreachable_pub)]
mod upgrade {
    use serde::{Deserialize, Serialize};
    include!("../../service/src/upgrade_api_model.rs");
}

mod multipart_created {
    include!("../../service/src/r2_backend_multipart_model.rs");
}
