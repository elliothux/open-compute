//! Active interceptor startup and CA protocol qualification.

use super::*;
use crate::container_broker::{contract::BrokerLease, policy, relay};
use sha2::{Digest as _, Sha256};

impl Engine {
    pub(super) async fn probe_sidecar(&self, config: &BrokerConfig) -> Result<bool, PlatformError> {
        let image = config.sidecar_image.as_deref().ok_or_else(invalid)?;
        let (status, info) = self
            .json(Method::GET, &format!("/images/{image}/json"), Value::Null)
            .await?;
        if status != 200
            || info.pointer("/Config/Volumes").is_some_and(|value| {
                !value.is_null() && value.as_object().is_none_or(|fields| !fields.is_empty())
            })
        {
            return Ok(false);
        }
        let (status, bridge) = self
            .json(Method::GET, "/networks/bridge", Value::Null)
            .await?;
        let subnet = bridge
            .pointer("/IPAM/Config/0/Subnet")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if status != 200 {
            return Ok(false);
        }
        let port = 39_001_u16;
        let key = hex::encode(Sha256::digest(uuid::Uuid::new_v4().as_bytes()));
        let lease = BrokerLease {
            directory_snapshots: Vec::new(),
            scheduling_policy: open_compute_storage::containers::ContainerSchedulingPolicy::Default,
            lease_key: key.clone(),
            owner_key: key.clone(),
            start_identity: key.clone(),
            manifest_sha256: key.clone(),
            startup: crate::container_broker::BrokerStartOptions {
                enable_internet: false,
                entrypoint: None,
                environment_variables: Vec::new(),
                labels: Vec::new(),
            },
            image: image.into(),
            diff_ids: Vec::new(),
            provider_contract_sha256: key,
            resources: config.resources,
            application_id: open_compute_core::ResourceId::generate(),
            max_instances: 1,
        };
        let name = format!("{}-proxy", lease.lease_key);
        let request = json!({"Image":image,"Cmd":["--http-egress-port",port.to_string(),"--http-ingress-address","0.0.0.0:39001","--docker-gateway-cidr",subnet,"--dns-enabled","--tls-intercept","--control-auth-file",crate::container_broker::credentials::AUTH_PATH],
            "ExposedPorts":{"39001/tcp":{}},"HostConfig":{"NetworkMode":"bridge","PublishAllPorts":true,"Dns":["1.1.1.1","8.8.8.8"],"ExtraHosts":["host.docker.internal:host-gateway"],"CapAdd":["NET_ADMIN"]}});
        let body = policy::prepare_create(
            &serde_json::to_vec(&request).map_err(|_| invalid())?,
            &name,
            &lease,
            image,
            &[],
            &config.exec_helper_volume,
        )?;
        let created = self
            .send(
                Method::POST,
                &format!("/containers/create?name={name}"),
                body,
                false,
            )
            .await?;
        if created.status().as_u16() != 201 {
            return Ok(false);
        }
        drop(created);
        let mut phase = "helper";
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            // This container has no mounts: these bytes prove the immutable image source.
            self.verify_helper_bytes(&name, &config.exec_helper_sha256)
                .await?;
            phase = "credentials";
            let credentials = crate::container_broker::BrokerControlCredentials::generate()?;
            crate::container_broker::credentials::install(self, &name, &credentials).await?;
            phase = "start";
            let (status, _) = self
                .json(
                    Method::POST,
                    &format!("/containers/{name}/start"),
                    Value::Null,
                )
                .await?;
            if status != 204 {
                return Ok(false);
            }
            phase = "inspect";
            let (status, inspect) = self
                .json(
                    Method::GET,
                    &format!("/containers/{name}/json"),
                    Value::Null,
                )
                .await?;
            if status != 200
                || inspect.pointer("/State/Running").and_then(Value::as_bool) != Some(true)
            {
                return Ok(false);
            }
            policy::verify_inspect(
                &inspect,
                &name,
                &lease,
                image,
                None,
                &config.exec_helper_volume,
            )?;
            let port = relay::published_port(&inspect)?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|_| invalid())?;
            let address = std::net::SocketAddr::new(config.relay_host, port);
            let base = format!("http://{address}");
            phase = "profile";
            let profile = loop {
                if let Ok(response) = client
                    .get(format!("{base}/open-compute/profile"))
                    .header(
                        crate::container_broker::credentials::CONTROL_HEADER,
                        &credentials.control_token,
                    )
                    .send()
                    .await
                {
                    if response.status().as_u16() == 200 {
                        break response;
                    }
                    if response.status().as_u16() == 401 {
                        return Ok(false);
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            if profile
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                != Some("application/json")
            {
                return Ok(false);
            }
            let profile: Value = serde_json::from_slice(&bounded_response(profile, 1_024).await?)
                .map_err(|_| invalid())?;
            if profile != json!({"protocol":"open-compute-container-sidecar","version":2}) {
                return Ok(false);
            }
            phase = "control_authentication";
            let wrong = crate::container_broker::BrokerControlCredentials::generate()?;
            // Independent denials share the startup deadline.
            let mut checks = Vec::new();
            for (method, path) in [
                (Method::GET, "/open-compute/profile"),
                (Method::GET, "/ca"),
                (Method::PUT, "/egress"),
                (Method::CONNECT, "/"),
                (Method::POST, "/open-compute/probe-callback"),
                (Method::CONNECT, "/open-compute/callback-channel"),
            ] {
                for token in [None, Some(wrong.control_token.as_str())] {
                    let mut request = client.request(method.clone(), format!("{base}{path}"));
                    if let Some(token) = token {
                        request = request
                            .header(crate::container_broker::credentials::CONTROL_HEADER, token);
                    }
                    checks.push(async move {
                        request
                            .send()
                            .await
                            .map(|response| response.status().as_u16() == 401)
                            .map_err(|_| invalid())
                    });
                }
            }
            if futures::future::try_join_all(checks)
                .await?
                .contains(&false)
            {
                return Ok(false);
            }
            phase = "certificate";
            let response = client
                .get(format!("{base}/ca"))
                .header(
                    crate::container_broker::credentials::CONTROL_HEADER,
                    &credentials.control_token,
                )
                .send()
                .await
                .map_err(|_| invalid())?;
            if response.status().as_u16() != 200 {
                return Ok(false);
            }
            let bytes = bounded_response(response, 65_536).await?;
            let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?.trim();
            if !text.starts_with("-----BEGIN CERTIFICATE-----")
                || !text.ends_with("-----END CERTIFICATE-----")
            {
                return Ok(false);
            }
            let certificate = reqwest::Certificate::from_pem(&bytes).map_err(|_| invalid())?;
            reqwest::Client::builder()
                .no_proxy()
                .add_root_certificate(certificate)
                .build()
                .map_err(|_| invalid())?;
            phase = "callback";
            callback::probe(&client, address, &credentials).await
        })
        .await;
        let timed_out = result.is_err();
        let result = result
            .map_err(|_| invalid())
            .and_then(std::convert::identity);
        if !matches!(&result, Ok(true)) {
            tracing::warn!(
                phase,
                timeout = timed_out,
                "Container sidecar qualification failed"
            );
        }
        let (status, _) = self
            .json(
                Method::DELETE,
                &format!("/containers/{name}?force=true"),
                Value::Null,
            )
            .await?;
        if !matches!(status, 204 | 404) {
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
        result
    }
}
