//! One owned probe proves resource enforcement and snapshot mount semantics.

use super::*;
use crate::container_broker::{contract::process_identity, policy};
use std::io::{Read as _, Seek as _, SeekFrom};

const ROOT_MARKER: &str = "open-compute-snapshot-probe";
const VOLATILE_MARKER: &str = "run/open-compute-snapshot-probe";
const RESOURCE_PROBE_COMMAND: &str = "set -eu; if mv /.open-compute-exec /.open-compute-exec-moved; then exit 80; fi; if printf x > /.open-compute-exec/helper; then exit 79; fi; test \"$(cat /sys/fs/cgroup/memory.max)\" = 67108864; test \"$(cat /sys/fs/cgroup/pids.max)\" = 16; read quota period < /sys/fs/cgroup/cpu.max; test \"$quota\" != max; test \"$((quota * 16))\" -eq \"$period\"; test \"$(ulimit -n)\" = 64; if result=$(dd if=/dev/zero of=/quota-probe bs=1048576 count=17 2>&1); then exit 77; fi; case \"$result\" in *'No space left on device'*) ;; *) exit 78;; esac; test \"$(stat -c %s /quota-probe)\" -le 16777216; : > /quota-probe; umask 027; printf %s \"$OPEN_COMPUTE_QUALIFICATION_NONCE\" > /run/open-compute-snapshot-probe; printf %s \"$OPEN_COMPUTE_QUALIFICATION_NONCE\" > /open-compute-snapshot-probe.pending; mv /open-compute-snapshot-probe.pending /open-compute-snapshot-probe; while :; do :; done";

impl Engine {
    pub(super) async fn probe_resources(
        &self,
        config: &BrokerConfig,
    ) -> Result<(bool, bool), PlatformError> {
        let sidecar = config.sidecar_image.as_ref().ok_or_else(invalid)?;
        for image in [&config.qualification_image, sidecar] {
            let (status, value) = self
                .json(Method::GET, &format!("/images/{image}/json"), Value::Null)
                .await?;
            if status != 200 || value.get("Id").and_then(Value::as_str) != Some(image) {
                return Ok((false, false));
            }
        }
        let name = format!("oc-qualification-{}", uuid::Uuid::new_v4());
        let nonce = uuid::Uuid::new_v4().to_string();
        let request = json!({
            "Image":config.qualification_image, "User":"0", "Entrypoint":["/bin/sh"], "Cmd":["-c",RESOURCE_PROBE_COMMAND],
            "Env":[format!("OPEN_COMPUTE_QUALIFICATION_NONCE={nonce}")],
            "HostConfig":{"NetworkMode":"none","NanoCpus":62_500_000,"Memory":67_108_864,"MemorySwap":67_108_864,
                "PidsLimit":16,"Ulimits":[{"Name":"nofile","Soft":64,"Hard":64}],"Tmpfs":policy::run_tmpfs(67_108_864),
                "StorageOpt":{"size":"16777216"},"LogConfig":{"Type":"none"},"SecurityOpt":["no-new-privileges:true"],
                "Mounts":crate::container_broker::helper::mount(&config.exec_helper_volume)}
        });
        let created = self
            .json(
                Method::POST,
                &format!("/containers/create?name={name}"),
                request,
            )
            .await;
        let mut phase = "create";
        let outcome = tokio::time::timeout(Duration::from_secs(180), async {
            let (status, created) = created?;
            if status != 201 {
                return Ok((false, false));
            }
            let id = created
                .get("Id")
                .and_then(Value::as_str)
                .filter(|id| crate::container_broker::contract::opaque(id))
                .ok_or_else(invalid)?;
            phase = "helper";
            self.verify_helper_volume(config).await?;
            self.verify_helper_bytes(id, &config.exec_helper_sha256)
                .await?;
            phase = "start";
            let (status, _) = self
                .json(
                    Method::POST,
                    &format!("/containers/{id}/start"),
                    Value::Null,
                )
                .await?;
            if status != 204 {
                return Ok((false, false));
            }
            phase = "marker";
            if !self.await_resource_marker(id, &nonce).await? {
                return Ok((false, false));
            }
            phase = "source";
            let before = self
                .snapshot_source(id, &config.qualification_image, &config.exec_helper_volume)
                .await?;
            phase = "capture";
            let (file, _) = self
                .capture_rootfs(id, &config.spool_dir, config.max_image_bytes)
                .await?;
            phase = "source_recheck";
            if self
                .snapshot_source(id, &config.qualification_image, &config.exec_helper_volume)
                .await?
                != before
            {
                return Err(invalid());
            }
            phase = "archive";
            let mut file = file.into_std().await;
            let snapshots = tokio::task::spawn_blocking(move || {
                file.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
                archive_proof(&mut file, &nonce, true)
            })
            .await
            .map_err(|_| invalid())??;
            Ok((true, snapshots))
        })
        .await
        .map_err(|_| invalid())
        .and_then(std::convert::identity);
        if !matches!(&outcome, Ok((true, true))) {
            tracing::warn!(phase, "Container resource qualification failed");
        }
        let (removed, _) = self
            .json(
                Method::DELETE,
                &format!("/containers/{name}?force=true"),
                Value::Null,
            )
            .await?;
        if !matches!(removed, 204 | 404) {
            return Err(invalid());
        }
        let (status, _) = self
            .json(
                Method::GET,
                &format!("/containers/{name}/json"),
                Value::Null,
            )
            .await?;
        if status != 404 {
            return Err(invalid());
        }
        outcome
    }

    async fn await_resource_marker(&self, id: &str, nonce: &str) -> Result<bool, PlatformError> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let response = self
                    .send(
                        Method::GET,
                        &format!("/containers/{id}/archive?path=%2F{ROOT_MARKER}"),
                        Vec::new(),
                        false,
                    )
                    .await?;
                if response.status().as_u16() == 200 {
                    let bytes = bounded_response(response, 16 << 10).await?;
                    open_compute_artifacts::verify_container_archive(
                        bytes.as_slice(),
                        16 << 10,
                        8,
                    )?;
                    let proof = archive_proof(bytes.as_slice(), nonce, false);
                    if !matches!(&proof, Ok(true)) {
                        tracing::warn!(
                            archive_readable = proof.is_ok(),
                            "Container qualification marker did not match"
                        );
                    }
                    return proof;
                }
                match response.status().as_u16() {
                    404 => {}
                    500 => {
                        let bytes = bounded_response(response, 4096).await?;
                        if !archive_pivot_enospc(&bytes) {
                            tracing::warn!(
                                status = 500,
                                response_bytes = bytes.len(),
                                "Container qualification archive request rejected"
                            );
                            return Err(invalid());
                        }
                    }
                    status => {
                        tracing::warn!(status, "Container qualification archive request rejected");
                        return Err(invalid());
                    }
                }
                let (status, value) = self
                    .json(Method::GET, &format!("/containers/{id}/json"), Value::Null)
                    .await?;
                if status != 200
                    || value.pointer("/State/Running").and_then(Value::as_bool) != Some(true)
                {
                    tracing::warn!(status, "Container qualification process is not running");
                    return Ok(false);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| invalid())?
    }

    async fn snapshot_source(
        &self,
        id: &str,
        image: &str,
        helper_volume: &str,
    ) -> Result<(String, String), PlatformError> {
        let (status, value) = self
            .json(Method::GET, &format!("/containers/{id}/json"), Value::Null)
            .await?;
        let identity = process_identity(&value)?;
        if status != 200
            || identity.0 != id
            || value.get("Image").and_then(Value::as_str) != Some(image)
            || value.pointer("/State/Running").and_then(Value::as_bool) != Some(true)
            || value.pointer("/State/Paused").and_then(Value::as_bool) != Some(false)
            || value.pointer("/HostConfig/Tmpfs") != Some(&policy::run_tmpfs(67_108_864))
        {
            return Err(invalid());
        }
        crate::container_broker::helper::verify_mounts(&value, helper_volume)?;
        Ok(identity)
    }
}

// The owned resource probe deliberately saturates its writable-layer quota before
// publishing its marker. Docker must create this directory before it can stat the marker.
fn archive_pivot_enospc(bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ArchiveError {
        message: String,
    }
    let Ok(error) = serde_json::from_slice::<ArchiveError>(bytes) else {
        return false;
    };
    let Some(path) = error
        .message
        .strip_prefix("Error setting up pivot dir: mkdir ")
        .and_then(|message| message.strip_suffix(": no space left on device"))
    else {
        return false;
    };
    path.starts_with('/')
        && !path.bytes().any(|byte| byte.is_ascii_control())
        && path
            .split('/')
            .skip(1)
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
        && path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_prefix(".pivot_root"))
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
}

fn archive_proof(
    reader: impl std::io::Read,
    nonce: &str,
    rootfs: bool,
) -> Result<bool, PlatformError> {
    let mut archive = tar::Archive::new(reader);
    let mut root = false;
    for entry in archive.entries().map_err(|_| invalid())? {
        let mut entry = entry.map_err(|_| invalid())?;
        let name = entry
            .path()
            .map_err(|_| invalid())?
            .components()
            .filter(|part| *part != std::path::Component::CurDir)
            .collect::<std::path::PathBuf>();
        if name == std::path::Path::new(VOLATILE_MARKER)
            || name
                == std::path::Path::new(
                    crate::container_broker::helper::PATH.trim_start_matches('/'),
                )
        {
            return Ok(false);
        }
        if name == std::path::Path::new(ROOT_MARKER) {
            let mode = entry.header().mode().map_err(|_| invalid())?;
            if root
                || !entry.header().entry_type().is_file()
                || mode != 0o640
                || entry.size() != nonce.len() as u64
            {
                tracing::warn!(
                    duplicate = root,
                    regular = entry.header().entry_type().is_file(),
                    marker_mode = mode,
                    marker_bytes = entry.size(),
                    "Container qualification marker metadata rejected"
                );
                return Ok(false);
            }
            let mut content = Vec::new();
            entry.read_to_end(&mut content).map_err(|_| invalid())?;
            root = content == nonce.as_bytes();
        } else if !rootfs {
            return Ok(false);
        }
    }
    Ok(root)
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
