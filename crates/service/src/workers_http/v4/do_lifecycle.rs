//! Fixed-Wrangler Durable Object export/migration lifecycle adapter.

use super::errors::{invalid, unsupported};
use super::model::{WorkerUploadExport, WorkerUploadMetadata};
use crate::workers_http::WorkerApiState;
use open_compute_core::{AccountId, ErrorCode, PlatformError, WorkerId};
use open_compute_storage::{
    DurableObjectClassRename, DurableObjectMigrationPlan, DurableObjectRepository,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct PreparedDoMigration {
    plan: DurableObjectMigrationPlan,
    already_committed: bool,
}

impl PreparedDoMigration {
    pub(super) fn tag(&self) -> &str {
        &self.plan.new_tag
    }

    pub(super) fn complete(
        &self,
        api: &WorkerApiState,
        worker_id: WorkerId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if self.already_committed {
            return Ok(());
        }
        DurableObjectRepository::new(&api.storage)
            .complete_worker_migration(worker_id, &self.plan, now_ms)
    }

    pub(super) fn rollback(
        &self,
        api: &WorkerApiState,
        worker_id: WorkerId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if self.already_committed {
            return Ok(());
        }
        DurableObjectRepository::new(&api.storage).rollback_worker_migration(
            worker_id,
            &self.plan.new_tag,
            now_ms,
        )
    }
}

pub(super) fn prepare(
    api: &WorkerApiState,
    account_id: AccountId,
    worker_id: WorkerId,
    metadata: &WorkerUploadMetadata,
    now_ms: i64,
) -> Result<Option<PreparedDoMigration>, PlatformError> {
    let repository = DurableObjectRepository::new(&api.storage);
    let current = repository.current_worker_migration_tag(worker_id)?;
    let Some(mut plan) = migration_plan(metadata, current.clone())? else {
        return Ok(None);
    };
    if current.as_deref() == Some(plan.new_tag.as_str()) {
        return Ok(Some(PreparedDoMigration {
            plan,
            already_committed: true,
        }));
    }
    if current.as_ref() != plan.old_tag.as_ref() {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "Durable Object migration tag does not match current Script authority",
        ));
    }
    repository.prepare_worker_migration(account_id, worker_id, &plan, now_ms)?;
    plan.new_sqlite_classes.sort();
    Ok(Some(PreparedDoMigration {
        plan,
        already_committed: false,
    }))
}

pub(super) fn declares_live_class(metadata: &WorkerUploadMetadata, class_name: &str) -> bool {
    if metadata.migrations.as_ref().is_some_and(|migrations| {
        migrations.steps.iter().any(|step| {
            step.new_sqlite_classes
                .iter()
                .any(|name| name == class_name)
                || step
                    .renamed_classes
                    .iter()
                    .any(|rename| rename.to == class_name)
        })
    }) {
        return true;
    }
    metadata
        .exports
        .as_ref()
        .and_then(|exports| exports.get(class_name))
        .is_some_and(|export| match export {
            WorkerUploadExport::DurableObject { state, storage, .. } => {
                state.as_deref().unwrap_or("created") == "created"
                    && storage.as_deref() == Some("sqlite")
            }
            WorkerUploadExport::Worker { .. } => false,
        })
}

fn migration_plan(
    metadata: &WorkerUploadMetadata,
    current_tag: Option<String>,
) -> Result<Option<DurableObjectMigrationPlan>, PlatformError> {
    let durable_exports = metadata
        .exports
        .as_ref()
        .map(|exports| {
            exports
                .iter()
                .filter(|(_, export)| matches!(export, WorkerUploadExport::DurableObject { .. }))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    if metadata.migrations.is_some() && !durable_exports.is_empty() {
        return Err(invalid(
            "Durable Object exports and migrations are mutually exclusive",
        ));
    }
    if let Some(migrations) = &metadata.migrations {
        if migrations.steps.is_empty() {
            return Err(invalid("Durable Object migration steps are empty"));
        }
        let mut plan = DurableObjectMigrationPlan {
            declarative: false,
            old_tag: migrations.old_tag.clone(),
            new_tag: migrations.new_tag.clone(),
            new_sqlite_classes: Vec::new(),
            renamed_classes: Vec::new(),
            deleted_classes: Vec::new(),
        };
        for step in &migrations.steps {
            if !step.new_classes.is_empty() || !step.transferred_classes.is_empty() {
                return Err(unsupported(
                    "only SQLite Durable Object migrations are supported",
                ));
            }
            plan.new_sqlite_classes
                .extend(step.new_sqlite_classes.iter().cloned());
            plan.renamed_classes
                .extend(
                    step.renamed_classes
                        .iter()
                        .map(|rename| DurableObjectClassRename {
                            from: rename.from.clone(),
                            to: rename.to.clone(),
                        }),
                );
            plan.deleted_classes
                .extend(step.deleted_classes.iter().cloned());
        }
        return Ok(Some(plan));
    }
    if durable_exports.is_empty() {
        return Ok(None);
    }
    declarative_export_plan(&durable_exports, current_tag).map(Some)
}

fn declarative_export_plan(
    exports: &BTreeMap<&String, &WorkerUploadExport>,
    current_tag: Option<String>,
) -> Result<DurableObjectMigrationPlan, PlatformError> {
    let mut live = BTreeSet::new();
    let mut rename_targets = BTreeSet::new();
    let mut renamed_classes = Vec::new();
    let mut deleted_classes = Vec::new();
    let mut tag_material = Vec::new();
    for (name, export) in exports {
        let WorkerUploadExport::DurableObject {
            state,
            storage,
            renamed_to,
            container,
            transferred_to,
            transfer_from,
        } = export
        else {
            continue;
        };
        tag_material.extend_from_slice(name.as_bytes());
        tag_material.push(0);
        tag_material.extend_from_slice(state.as_deref().unwrap_or("created").as_bytes());
        tag_material.push(0);
        tag_material.extend_from_slice(storage.as_deref().unwrap_or_default().as_bytes());
        tag_material.push(0);
        tag_material.extend_from_slice(renamed_to.as_deref().unwrap_or_default().as_bytes());
        tag_material.push(0xff);
        if container.is_some() || transferred_to.is_some() || transfer_from.is_some() {
            return Err(unsupported(
                "Durable Object containers and cross-Script transfer are unsupported",
            ));
        }
        match state.as_deref().unwrap_or("created") {
            "created" if storage.as_deref() == Some("sqlite") && renamed_to.is_none() => {
                live.insert((*name).clone());
            }
            "deleted" if storage.is_none() && renamed_to.is_none() => {
                deleted_classes.push((*name).clone());
            }
            "renamed" if storage.is_none() => {
                let target = renamed_to
                    .as_ref()
                    .ok_or_else(|| invalid("Durable Object rename target is missing"))?;
                rename_targets.insert(target.clone());
                renamed_classes.push(DurableObjectClassRename {
                    from: (*name).clone(),
                    to: target.clone(),
                });
            }
            _ => {
                return Err(unsupported(
                    "Durable Object export lifecycle shape is unsupported",
                ));
            }
        }
    }
    if !rename_targets.is_subset(&live) {
        return Err(invalid(
            "Durable Object rename targets must also be live SQLite exports",
        ));
    }
    let new_sqlite_classes = live
        .difference(&rename_targets)
        .cloned()
        .collect::<Vec<_>>();
    let digest = Sha256::digest(&tag_material);
    Ok(DurableObjectMigrationPlan {
        declarative: true,
        old_tag: current_tag,
        new_tag: format!("exports-{}", &hex::encode(digest)[..32]),
        new_sqlite_classes,
        renamed_classes,
        deleted_classes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(value: serde_json::Value) -> WorkerUploadMetadata {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn fixed_wrangler_migrations_and_exports_map_to_one_sqlite_plan() {
        let migrations = metadata(serde_json::json!({
            "main_module": "index.js",
            "compatibility_date": "2026-08-30",
            "migrations": {
                "old_tag": "v1",
                "new_tag": "v2",
                "steps": [{
                    "new_sqlite_classes": ["Created"],
                    "renamed_classes": [{"from": "Old", "to": "Renamed"}],
                    "deleted_classes": ["Deleted"]
                }]
            }
        }));
        let plan = migration_plan(&migrations, Some("v1".to_owned()))
            .unwrap()
            .unwrap();
        assert_eq!(plan.new_tag, "v2");
        assert_eq!(plan.new_sqlite_classes, ["Created"]);
        assert_eq!(plan.renamed_classes[0].to, "Renamed");

        let exports = metadata(serde_json::json!({
            "main_module": "index.js",
            "compatibility_date": "2026-08-30",
            "exports": {
                "default": {"type": "worker", "cache": {"enabled": true}},
                "NewName": {"type": "durable-object", "storage": "sqlite"},
                "OldName": {"type": "durable-object", "state": "renamed", "renamed_to": "NewName"},
                "Gone": {"type": "durable-object", "state": "deleted"}
            }
        }));
        let plan = migration_plan(&exports, Some("prior".to_owned()))
            .unwrap()
            .unwrap();
        assert_eq!(plan.old_tag.as_deref(), Some("prior"));
        assert!(plan.new_sqlite_classes.is_empty());
        assert_eq!(plan.renamed_classes[0].from, "OldName");
        assert_eq!(plan.deleted_classes, ["Gone"]);
        assert!(declares_live_class(&exports, "NewName"));
    }

    #[test]
    fn non_sqlite_and_transfer_lifecycle_fail_closed() {
        for value in [
            serde_json::json!({
                "main_module": "index.js",
                "compatibility_date": "2026-08-30",
                "migrations": {"new_tag": "v1", "steps": [{"new_classes": ["Legacy"]}]}
            }),
            serde_json::json!({
                "main_module": "index.js",
                "compatibility_date": "2026-08-30",
                "exports": {"Moved": {"type": "durable-object", "state": "transferred", "transferred_to": "other"}}
            }),
        ] {
            assert!(migration_plan(&metadata(value), None).is_err());
        }
    }
}
