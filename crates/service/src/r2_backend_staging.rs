//! Bounded staging and checksum work for R2 object PUTs.

use super::*;

impl R2BindingService {
    pub(super) async fn stage_management_put(
        &self,
        resource: ResourceId,
        request_id: &str,
        key: &str,
        max_object_bytes: u64,
        body: Body,
    ) -> Result<StagedPut, PlatformError> {
        use futures::TryStreamExt as _;
        let mut stream = body.into_data_stream();
        let (path, file) = self.staging.create(resource, request_id)?;
        let mut file = tokio::fs::File::from_std(file);
        let guard = StagingFile::new(path);
        let mut length = 0_u64;
        let mut reservation = StagingReservation::new(
            self.staging_bytes.clone(),
            self.config.max_staging_bytes,
            self.metrics.clone(),
        );
        while let Some(chunk) = stream.try_next().await.map_err(|_| protocol_error())? {
            let added = u64::try_from(chunk.len()).map_err(|_| object_too_large())?;
            length = length.checked_add(added).ok_or_else(object_too_large)?;
            if length > max_object_bytes {
                return Err(object_too_large());
            }
            reservation.add(added)?;
            ensure_storage_headroom(&self.storage, added)?;
            file.write_all(&chunk).await.map_err(|_| overloaded())?;
        }
        file.sync_all().await.map_err(|_| overloaded())?;
        drop(file);
        self.hash_staged_put(
            PutHeader {
                key: key.to_owned(),
                options: PutWireOptions::default(),
            },
            length,
            guard,
            reservation,
        )
        .await
    }

    pub(super) async fn stage_put(
        &self,
        resource: ResourceId,
        request_id: &str,
        max_object_bytes: u64,
        body: Body,
    ) -> Result<StagedPut, PlatformError> {
        use futures::TryStreamExt as _;
        let mut stream = body.into_data_stream();
        let mut header_bytes = Vec::with_capacity(4096);
        let mut header_end = None;
        let mut header = None;
        let mut staged = None;
        let mut length = 0_u64;
        let mut reservation = StagingReservation::new(
            self.staging_bytes.clone(),
            self.config.max_staging_bytes,
            self.metrics.clone(),
        );
        while let Some(chunk) = stream.try_next().await.map_err(|_| protocol_error())? {
            let mut remaining = chunk.as_ref();
            while !remaining.is_empty() {
                if header.is_none() {
                    let needed =
                        header_end.map_or(4, |end: usize| end.saturating_sub(header_bytes.len()));
                    let take = needed.min(remaining.len());
                    header_bytes.extend_from_slice(&remaining[..take]);
                    remaining = &remaining[take..];
                    if header_end.is_none() && header_bytes.len() == 4 {
                        let size = u32::from_be_bytes(
                            header_bytes[..4].try_into().map_err(|_| protocol_error())?,
                        );
                        let size = usize::try_from(size).map_err(|_| protocol_error())?;
                        if size > MAX_METADATA_BYTES {
                            return Err(metadata_too_large());
                        }
                        header_end = Some(4_usize.checked_add(size).ok_or_else(protocol_error)?);
                    }
                    if header_end.is_some_and(|end| end == header_bytes.len()) {
                        let parsed: PutHeader = parse_json(&header_bytes[4..])?;
                        parsed.options.validate()?;
                        let (path, file) = self.staging.create(resource, request_id)?;
                        let file = tokio::fs::File::from_std(file);
                        staged = Some((StagingFile::new(path), file));
                        header = Some(parsed);
                    }
                    continue;
                }
                let added = u64::try_from(remaining.len()).map_err(|_| object_too_large())?;
                length = length.checked_add(added).ok_or_else(object_too_large)?;
                if length > max_object_bytes {
                    return Err(object_too_large());
                }
                reservation.add(added)?;
                ensure_storage_headroom(&self.storage, added)?;
                let (_, file) = staged.as_mut().ok_or_else(protocol_error)?;
                file.write_all(remaining).await.map_err(|_| overloaded())?;
                remaining = &[];
            }
        }
        let header = header.ok_or_else(protocol_error)?;
        let (guard, file) = staged.ok_or_else(protocol_error)?;
        file.sync_all().await.map_err(|_| overloaded())?;
        drop(file);
        self.hash_staged_put(header, length, guard, reservation)
            .await
    }

    pub(super) async fn hash_staged_put(
        &self,
        header: PutHeader,
        length: u64,
        guard: StagingFile,
        reservation: StagingReservation,
    ) -> Result<StagedPut, PlatformError> {
        let permit = self
            .checksums
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| overloaded())?;
        // A started blocking task cannot be aborted. It owns both the staging
        // reservation and its CPU permit until hashing (and cleanup) really ends.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let checksums = hash_file(&guard.path, length)?;
            Ok(StagedPut {
                header,
                length,
                checksums,
                guard,
                _reservation: reservation,
            })
        })
        .await
        .map_err(|_| overloaded())?
    }
}
