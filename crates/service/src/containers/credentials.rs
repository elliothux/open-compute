//! Serialize control grants across startup generations and retain unknown rotation outcomes.

use super::*;

impl ContainerService {
    pub(crate) async fn credentials_for_generation(
        &self,
        key: &str,
        generation: &str,
    ) -> Result<crate::container_broker::BrokerControlCredentials, PlatformError> {
        let mut pending = self.credential_rotation.lock().await;
        if pending.contains(key) {
            return Err(unavailable());
        }
        let mut hash = Sha256::new();
        hash.update(b"dev.open-compute.container.control-generation.v1\0");
        hash.update(generation.as_bytes());
        // Set before I/O. Cancellation or an unknown response keeps later rotation fenced.
        pending.insert(key.to_owned());
        let credentials = self
            .provider
            .lease_credentials(key, &hex::encode(hash.finalize()))
            .await?;
        pending.remove(key);
        Ok(credentials)
    }
}
