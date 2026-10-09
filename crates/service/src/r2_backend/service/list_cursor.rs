use super::*;

impl R2BindingService {
    pub(super) async fn head_list_objects(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        objects: &[R2ObjectRecord],
        timeout: Duration,
    ) -> Result<Vec<R2ObjectMetadata>, PlatformError> {
        use futures::{StreamExt as _, stream};
        let fanout = usize::try_from(self.config.max_metadata_head_concurrency)
            .unwrap_or(1)
            .max(1);
        let owned = objects.to_vec();
        let results = stream::iter(owned.into_iter().enumerate().map(
            |(index, object)| async move {
                let key = UserObjectKey::parse(&object.object_key)?;
                let metadata = self
                    .authoritative_head(binding, locator, &key, timeout)
                    .await?
                    .ok_or_else(metadata_invalid)?;
                Ok::<_, PlatformError>((index, metadata))
            },
        ))
        .buffer_unordered(fanout)
        .collect::<Vec<_>>()
        .await;
        let mut ordered: Vec<Option<R2ObjectMetadata>> = (0..objects.len()).map(|_| None).collect();
        for result in results {
            let (index, value) = result?;
            ordered[index] = Some(value);
        }
        ordered
            .into_iter()
            .map(|value| value.ok_or_else(protocol_error))
            .collect()
    }

    pub(super) fn encode_cursor(
        &self,
        binding: &AuthorizedBinding,
        input: &ListRequest,
        include_mask: u8,
        after_key: Option<String>,
    ) -> Result<String, PlatformError> {
        let now = unix_ms()?;
        let payload = CursorPayload {
            v: 1,
            resource_id: binding.resource.id,
            generation: binding.resource.spec_generation,
            prefix_sha256: digest_text(&input.prefix),
            delimiter_sha256: digest_text(input.delimiter.as_deref().unwrap_or("")),
            include_mask,
            start_after_sha256: digest_text(input.start_after.as_deref().unwrap_or("")),
            after_key,
            expires_at_ms: now.saturating_add(self.config.cursor_ttl_ms),
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| cursor_invalid())?;
        let signature = self.storage.crypto().sign_r2_cursor(&bytes);
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Ok(format!(
            "{}.{}",
            base64.encode(bytes),
            base64.encode(signature)
        ))
    }

    pub(super) fn decode_cursor(
        &self,
        binding: &AuthorizedBinding,
        input: &ListRequest,
        include_mask: u8,
        cursor: &str,
    ) -> Result<Option<String>, PlatformError> {
        let (payload, signature) = cursor.split_once('.').ok_or_else(cursor_invalid)?;
        if signature.contains('.') {
            return Err(cursor_invalid());
        }
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let payload = base64.decode(payload).map_err(|_| cursor_invalid())?;
        let signature = base64.decode(signature).map_err(|_| cursor_invalid())?;
        if !self.storage.crypto().verify_r2_cursor(&payload, &signature) {
            return Err(cursor_invalid());
        }
        let decoded: CursorPayload =
            serde_json::from_slice(&payload).map_err(|_| cursor_invalid())?;
        if decoded.v != 1
            || decoded.resource_id != binding.resource.id
            || decoded.generation != binding.resource.spec_generation
            || decoded.prefix_sha256 != digest_text(&input.prefix)
            || decoded.delimiter_sha256 != digest_text(input.delimiter.as_deref().unwrap_or(""))
            || decoded.include_mask != include_mask
            || decoded.start_after_sha256 != digest_text(input.start_after.as_deref().unwrap_or(""))
            || decoded.expires_at_ms < unix_ms()?
        {
            return Err(cursor_invalid());
        }
        Ok(decoded.after_key)
    }
}
