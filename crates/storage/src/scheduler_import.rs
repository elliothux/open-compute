//! One-time, crash-safe import of the released scheduler database into control.

use crate::control_db::ControlDb;
use crate::schema_migrations::{self, DatabaseKind};
use open_compute_core::{ErrorCode, InstanceId, PlatformError};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};
use std::path::Path;
use std::time::Duration;

pub(crate) const TABLES: &[&str] = &[
    "scheduled_jobs",
    "queue_state",
    "queue_messages",
    "queue_enqueue_operations",
    "queue_consumer_state",
    "queue_delivery_batches",
    "queue_dlq_pending",
    "cron_schedules",
    "cron_runs",
    "workflow_events",
    "workflow_event_receipts",
    "workflow_gc_receipts",
    "workflow_instances",
    "workflow_mutation_context",
    "workflow_operation_progress",
    "workflow_step_dependencies",
    "workflow_steps",
    "scheduler_meta",
    "scheduler_identity",
];
const MARKER: &str = "scheduler_import_v3";

pub(crate) fn import(
    control: &ControlDb,
    root: &Path,
    busy_timeout_ms: u64,
    instance_id: InstanceId,
) -> Result<(), PlatformError> {
    import_inner(
        control,
        root,
        busy_timeout_ms,
        instance_id,
        #[cfg(test)]
        None,
    )
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum ImportFault {
    BeforeCommit,
    AfterCommit,
}

fn import_inner(
    control: &ControlDb,
    root: &Path,
    busy_timeout_ms: u64,
    instance_id: InstanceId,
    #[cfg(test)] fault: Option<ImportFault>,
) -> Result<(), PlatformError> {
    let source = root.join("scheduler.sqlite");
    if !source
        .try_exists()
        .map_err(|_| failed("cannot inspect legacy scheduler file"))?
    {
        return Ok(());
    }
    crate::fs::validate_owned_file(&source)?;
    let source = crate::control_db::canonical_database_path(&source)?;
    let complete = control.with_read(|connection| {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM platform_meta WHERE key=?1)",
                [MARKER],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| failed("cannot read scheduler import marker"))
    })?;
    if !complete {
        let mut legacy = Connection::open_with_flags(
            crate::control_db::canonical_database_path(&source)?,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| failed("cannot open legacy scheduler database"))?;
        legacy
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .map_err(|_| failed("cannot configure scheduler import timeout"))?;
        legacy
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")
            .map_err(|_| failed("cannot configure legacy scheduler database"))?;
        // Keep the legacy WAL and SHM files for the retained import copy.
        legacy
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(|_| failed("cannot retain legacy scheduler sidecars"))?;
        schema_migrations::migrate(&mut legacy, DatabaseKind::Scheduler)?;
        let quick: String = legacy
            .pragma_query_value(None, "quick_check", |row| row.get(0))
            .map_err(|_| failed("cannot check legacy scheduler database"))?;
        let owner: Option<String> = legacy
            .query_row(
                "SELECT instance_id FROM scheduler_identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| failed("cannot read legacy scheduler owner"))?;
        if quick != "ok"
            || owner
                .as_deref()
                .is_some_and(|owner| owner != instance_id.as_str())
        {
            return Err(failed(
                "legacy scheduler integrity or instance identity is invalid",
            ));
        }
        drop(legacy);
        control.with_connection_mut(|connection| {
            // ATTACH and DETACH must remain outside the transaction.
            connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .map_err(|_| failed("cannot retain attached scheduler sidecars"))?;
            connection.execute("ATTACH DATABASE ?1 AS legacy_scheduler", [source.to_str().ok_or_else(|| failed("legacy scheduler path is not UTF-8"))?])
                .map_err(|_| failed("cannot attach legacy scheduler database"))?;
            let result = (|| {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| failed("cannot begin scheduler import"))?;
                tx.execute_batch("PRAGMA defer_foreign_keys=ON;")
                    .map_err(|_| failed("cannot defer scheduler import foreign keys"))?;
                for table in TABLES {
                    let populated: bool = tx.query_row(
                        &format!("SELECT EXISTS(SELECT 1 FROM main.{table})"), [], |row| row.get(0),
                    ).map_err(|_| failed("cannot inspect scheduler import destination"))?;
                    if populated && !matches!(*table,"queue_consumer_state"|"cron_schedules") {
                        return Err(failed("control already contains scheduler rows without an import marker; refusing to merge"));
                    }
                }
                // Insert guards reject retained claims; counter triggers would count messages twice.
                let triggers = {
                    let mut statement = tx.prepare("SELECT name,tbl_name,sql FROM main.sqlite_master WHERE type='trigger'")
                        .map_err(|_| failed("cannot inspect scheduler triggers"))?;
                    statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
                        .map_err(|_| failed("cannot read scheduler triggers"))?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| failed("cannot read scheduler triggers"))?
                        .into_iter().filter(|(_, table, _)| TABLES.contains(&table.as_str())).collect::<Vec<_>>()
                };
                for (name, _, _) in &triggers {
                    tx.execute_batch(&format!("DROP TRIGGER main.\"{name}\";"))
                        .map_err(|_| failed("cannot suspend scheduler import triggers"))?;
                }
                for table in TABLES {
                    let columns = {
                        let mut statement = tx.prepare(&format!("PRAGMA main.table_info({table})"))
                            .map_err(|_| failed("cannot read scheduler destination columns"))?;
                        statement.query_map([], |row| row.get::<_, String>(1))
                            .map_err(|_| failed("cannot read scheduler destination columns"))?
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| failed("cannot read scheduler destination columns"))?
                    };
                    let select = columns.iter().map(|column| match (*table, column.as_str()) {
                        ("queue_consumer_state", "state") => "CASE WHEN state='paused' THEN 'paused' ELSE 'active' END".to_owned(),
                        ("cron_runs", "worker_id") => "(SELECT s.worker_id FROM legacy_scheduler.cron_schedules s WHERE s.activation_id=source.activation_id)".to_owned(),
                        _ => format!("source.{column}"),
                    }).collect::<Vec<_>>().join(",");
                    let authority = match *table {
                        "queue_consumer_state" => " WHERE EXISTS(SELECT 1 FROM main.queue_consumers c WHERE c.id=source.consumer_id AND c.version_id=source.version_id AND c.consumer_generation=source.consumer_generation)",
                        "cron_schedules" => " WHERE EXISTS(SELECT 1 FROM main.cron_activations c WHERE c.id=source.activation_id AND c.version_id=source.version_id)",
                        _ => "",
                    };
                    tx.execute(&format!("INSERT OR REPLACE INTO main.{table} SELECT {select} FROM legacy_scheduler.{table} source{authority}"), [])
                        .map_err(|_| failed("cannot copy legacy scheduler rows"))?;
                }
                // Recreate missing live targets from control authority within the import transaction.
                let workers = {
                    let mut statement=tx.prepare("SELECT DISTINCT worker_id FROM queue_consumers")
                        .map_err(|_| failed("cannot read live Queue targets"))?;
                    statement.query_map([],|row|row.get::<_,open_compute_core::WorkerId>(0))
                        .map_err(|_| failed("cannot read live Queue targets"))?.collect::<Result<Vec<_>,_>>()
                        .map_err(|_| failed("cannot read live Queue targets"))?
                };
                for worker in workers {
                    crate::queue_consumers::insert_scheduler_targets_tx(&tx,worker)
                        .map_err(|_| failed("cannot restore live Queue targets"))?;
                }
                tx.execute_batch("UPDATE queue_consumer_state SET state=(SELECT state FROM queue_consumers WHERE id=consumer_id);
                    INSERT OR IGNORE INTO cron_schedules
                    SELECT a.id,a.worker_id,a.version_id,w.route_generation,a.expression,a.expression_sha256,a.parser_version,
                      (a.updated_at_ms/60000+1)*60000,a.updated_at_ms
                    FROM cron_activations a JOIN workers w ON w.id=a.worker_id;
                    INSERT OR IGNORE INTO version_referrers SELECT version_id,'queue_batch',id,created_at_ms FROM queue_delivery_batches WHERE version_id IN(SELECT id FROM worker_versions);
                    INSERT OR IGNORE INTO version_referrers SELECT version_id,'cron_run',id,created_at_ms FROM cron_runs WHERE state IN('ready','claimed') AND version_id IN(SELECT id FROM worker_versions);")
                    .map_err(|_| failed("cannot retain imported execution targets"))?;
                // Preserve AUTOINCREMENT even when the largest message has already been deleted.
                tx.execute("DELETE FROM main.sqlite_sequence WHERE name='queue_messages'", [])
                    .map_err(|_| failed("cannot reset scheduler message sequence"))?;
                tx.execute("INSERT INTO main.sqlite_sequence SELECT * FROM legacy_scheduler.sqlite_sequence WHERE name='queue_messages'", [])
                    .map_err(|_| failed("cannot copy scheduler message sequence"))?;
                for (_, _, sql) in &triggers {
                    tx.execute_batch(sql).map_err(|_| failed("cannot restore scheduler triggers"))?;
                }
                // Recovery can discard exhausted messages, so counter and version-pin triggers must be active.
                crate::scheduler::reset_claims_tx(&tx, open_compute_core::wall_time_ms(), 3)?;
                tx.execute("INSERT INTO platform_meta(key,value,updated_at_ms) VALUES(?1,?2,?3)", params![MARKER, b"complete".as_slice(), open_compute_core::wall_time_ms()])
                    .map_err(|_| failed("cannot record scheduler import completion"))?;
                #[cfg(test)]
                if fault == Some(ImportFault::BeforeCommit) {
                    return Err(failed("injected failure before scheduler import commit"));
                }
                tx.commit().map_err(|_| failed("cannot commit scheduler import"))
            })();
            let detached = connection.execute_batch("DETACH DATABASE legacy_scheduler;")
                .map_err(|_| failed("cannot detach legacy scheduler database"));
            let configured = connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, false)
                .map_err(|_| failed("cannot restore control checkpoint configuration"));
            result?;
            detached?;
            configured.map(|_| ())
        })?;
    }
    #[cfg(test)]
    if fault == Some(ImportFault::AfterCommit) {
        return Err(failed("injected failure after scheduler import commit"));
    }
    for name in [
        "control.sqlite",
        "control.sqlite-wal",
        "scheduler.sqlite",
        "scheduler.sqlite-wal",
        "scheduler.sqlite-shm",
    ] {
        let path = root.join(name);
        if path
            .try_exists()
            .map_err(|_| failed("cannot inspect scheduler import files"))?
        {
            std::fs::File::open(path)
                .and_then(|file| file.sync_all())
                .map_err(|_| failed("cannot sync scheduler import files"))?;
        }
    }
    crate::fs::fsync_dir(root)?;
    // Rename the main file last so an interrupted rename can finish using the marker.
    for suffix in ["-wal", "-shm", ""] {
        let from = root.join(format!("scheduler.sqlite{suffix}"));
        let to = root.join(format!("scheduler.sqlite.imported{suffix}"));
        if from
            .try_exists()
            .map_err(|_| failed("cannot inspect scheduler import source"))?
        {
            crate::fs::validate_owned_file(&from)?;
            if to
                .try_exists()
                .map_err(|_| failed("cannot inspect scheduler import backup"))?
            {
                return Err(failed(
                    "scheduler import backup already exists; refusing to overwrite it",
                ));
            }
            std::fs::rename(from, to)
                .map_err(|_| failed("cannot retain imported scheduler database"))?;
            crate::fs::fsync_dir(root)?;
        }
    }
    Ok(())
}

fn failed(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::MigrationFailed, message)
}

#[cfg(test)]
#[path = "scheduler_import_tests.rs"]
mod tests;
