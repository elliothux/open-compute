//! Explicit live S3-provider qualification using the production client and preflights.

use open_compute_artifacts::{
    BackendError, ObjectBackend, ObjectKey, R2_MIN_MULTIPART_PART_BYTES, preflight_object_storage,
    preflight_r2, resolve_s3_credentials,
};
use open_compute_core::{InstanceId, PlatformError, S3Config, StartupId};
use serde_json::json;
use std::env;
use url::Url;
use uuid::Uuid;

const CASE: &str = "production-preflight";
const ACK: &str = "s3-provider-qualification";

fn preflight_failure(stage: &'static str, error: &PlatformError) -> &'static str {
    eprintln!("{stage}: {error} ({:?})", error.code());
    stage
}

fn required(name: &'static str) -> Result<String, &'static str> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(name)
}

fn selected_case() -> Result<Option<String>, &'static str> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args == ["--list"] {
        return Ok(None);
    }
    if args.len() == 2 && args[0] == "--case" && args[1] == CASE {
        return Ok(Some(args[1].clone()));
    }
    Err("arguments")
}

fn config() -> Result<S3Config, &'static str> {
    if required("OPEN_COMPUTE_TEST_R2_S3_MUTATION_ACK")? != ACK {
        return Err("mutation-acknowledgement");
    }
    let endpoint = required("OPEN_COMPUTE_TEST_R2_S3_ENDPOINT")?;
    let parsed = Url::parse(&endpoint).map_err(|_| "endpoint")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("endpoint");
    }
    let run = Uuid::now_v7().simple().to_string();
    Ok(S3Config {
        endpoint,
        region: required("OPEN_COMPUTE_TEST_R2_S3_REGION")?,
        bucket: required("OPEN_COMPUTE_TEST_R2_S3_BUCKET")?,
        access_key_id_env: Some("OPEN_COMPUTE_TEST_R2_S3_ACCESS_KEY_ID".to_owned()),
        secret_access_key_env: Some("OPEN_COMPUTE_TEST_R2_S3_SECRET_ACCESS_KEY".to_owned()),
        prefix: format!("qualification/{run}/system/"),
        r2_prefix: format!("tenant/r2-qualification/{run}/"),
        ..S3Config::default()
    })
}

async fn cleanup(backend: &ObjectBackend) -> Result<(), &'static str> {
    for prefix in [backend.prefix(), backend.r2_prefix()] {
        let marker =
            ObjectKey::new(format!("{prefix}authority/v1.json")).map_err(|_| "cleanup-key")?;
        match backend.delete(&marker).await {
            Ok(()) | Err(BackendError::NotFound) => {}
            Err(_) => return Err("cleanup-delete"),
        }
        let remaining = backend
            .list(prefix, 1, None)
            .await
            .map_err(|_| "cleanup-list")?;
        if !remaining.objects.is_empty() || remaining.next_cursor.is_some() {
            return Err("cleanup-incomplete");
        }
    }
    Ok(())
}

async fn qualify() -> Result<serde_json::Value, &'static str> {
    let config = config()?;
    let credentials = resolve_s3_credentials(&config).map_err(|_| "credentials")?;
    let backend = ObjectBackend::connect_s3(&config, &credentials, 2 * R2_MIN_MULTIPART_PART_BYTES)
        .map_err(|_| "client")?;
    let instance = InstanceId::generate();
    let general = preflight_object_storage(&backend, instance, StartupId::generate())
        .await
        .map_err(|error| preflight_failure("object-preflight", &error));
    let result = match general {
        Ok(general) => preflight_r2(&backend, instance, StartupId::generate())
            .await
            .map(|r2| {
                json!({
                    "objectPreflight": {
                        "puts": general.puts(),
                        "heads": general.heads(),
                        "gets": general.gets(),
                        "deletes": general.deletes()
                    },
                    "r2Preflight": {
                        "objects": r2.objects,
                        "multiDelete": r2.multi_delete
                    }
                })
            })
            .map_err(|error| preflight_failure("r2-preflight", &error)),
        Err(stage) => Err(stage),
    };
    let cleaned = cleanup(&backend).await;
    match (result, cleaned) {
        (Ok(evidence), Ok(())) => Ok(evidence),
        (Err(stage), _) | (Ok(_), Err(stage)) => Err(stage),
    }
}

#[tokio::main]
async fn main() {
    match selected_case() {
        Ok(None) => println!("{}", json!({"schemaVersion": 1, "cases": [CASE]})),
        Ok(Some(case)) => match qualify().await {
            Ok(evidence) => println!(
                "{}",
                json!({
                    "schemaVersion": 1,
                    "status": "passed",
                    "cases": [{"id": case, "status": "passed", "evidence": evidence}]
                })
            ),
            Err(stage) => {
                println!(
                    "{}",
                    json!({
                        "schemaVersion": 1,
                        "status": "failed",
                        "cases": [{"id": case, "status": "failed", "stage": stage}]
                    })
                );
                std::process::exit(1);
            }
        },
        Err(stage) => {
            println!(
                "{}",
                json!({"schemaVersion": 1, "status": "failed", "stage": stage, "cases": []})
            );
            std::process::exit(2);
        }
    }
}
