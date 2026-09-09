use super::*;

pub(super) fn seal_source(
    file: &mut File,
    key: &ObjectKey,
    source: ObjectSource,
    metadata: ObjectMetadata,
    customer_key: Option<&CustomerKey>,
) -> Result<EnvelopeHeader, BackendError> {
    match source {
        ObjectSource::Bytes(bytes) => {
            let length = bytes.len() as u64;
            seal_reader(
                file,
                key,
                std::io::Cursor::new(bytes),
                length,
                metadata,
                customer_key,
            )
        }
        ObjectSource::File {
            file: mut source_file,
            length,
        } => {
            let metadata_on_disk = source_file
                .metadata()
                .map_err(|_| BackendError::Unavailable)?;
            if !metadata_on_disk.file_type().is_file()
                || metadata_on_disk.len() != length
                || metadata_on_disk.nlink() != 1
                || std::os::unix::fs::MetadataExt::mode(&metadata_on_disk) & 0o077 != 0
                || std::os::unix::fs::MetadataExt::uid(&metadata_on_disk)
                    != rustix::process::getuid().as_raw()
            {
                return Err(BackendError::Corrupt);
            }
            source_file
                .seek(SeekFrom::Start(0))
                .map_err(|_| BackendError::Unavailable)?;
            seal_reader(file, key, source_file, length, metadata, customer_key)
        }
    }
}

pub(super) fn seal_reader(
    file: &mut File,
    key: &ObjectKey,
    mut source: impl Read,
    expected_size: u64,
    mut metadata: ObjectMetadata,
    customer_key: Option<&CustomerKey>,
) -> Result<EnvelopeHeader, BackendError> {
    file.write_all(&vec![0_u8; HEADER_BYTES])
        .map_err(|_| BackendError::Unavailable)?;
    let mut plaintext_sha256 = Sha256::new();
    let mut plaintext_md5 = Md5::new();
    let mut total = 0_u64;
    let mut stored = 0_u64;
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let encryption = customer_key
        .as_ref()
        .map(|key_value| encryption_header(key, key_value))
        .transpose()?;
    let cipher = customer_key
        .as_ref()
        .map(|value| XChaCha20Poly1305::new(Key::from_slice(value.bytes())));
    let nonce_base = encryption
        .as_ref()
        .map(|value| decode_nonce(&value.nonce))
        .transpose()?;
    let mut chunk_index = 0_u64;
    loop {
        let read = read_chunk(&mut source, &mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(BackendError::Capacity)?;
        plaintext_sha256.update(&buffer[..read]);
        plaintext_md5.update(&buffer[..read]);
        if let (Some(cipher), Some(nonce), Some(encryption)) =
            (&cipher, nonce_base, encryption.as_ref())
        {
            let nonce = chunk_nonce(nonce, chunk_index);
            let ciphertext = cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &buffer[..read],
                        aad: &chunk_aad_from_digest(
                            &hex::encode(Sha256::digest(key.as_str().as_bytes())),
                            &encryption.object_version,
                            expected_size,
                            chunk_index,
                        ),
                    },
                )
                .map_err(|_| BackendError::Corrupt)?;
            file.write_all(&ciphertext)
                .map_err(|_| BackendError::Unavailable)?;
            stored = stored
                .checked_add(ciphertext.len() as u64)
                .ok_or(BackendError::Capacity)?;
        } else {
            file.write_all(&buffer[..read])
                .map_err(|_| BackendError::Unavailable)?;
            stored = stored
                .checked_add(read as u64)
                .ok_or(BackendError::Capacity)?;
        }
        chunk_index = chunk_index.checked_add(1).ok_or(BackendError::Capacity)?;
    }
    if total != expected_size {
        return Err(BackendError::Corrupt);
    }
    metadata.size = total;
    metadata.last_modified_ms = open_compute_core::wall_time_ms();
    let payload_sha256 = hex::encode(plaintext_sha256.finalize());
    metadata.etag = hex::encode(plaintext_md5.finalize());
    metadata.ssec_key_md5 = encryption.as_ref().map(|value| value.ssec_key_md5.clone());
    let header = EnvelopeHeader {
        schema_version: FORMAT_SCHEMA,
        key_sha256: hex::encode(Sha256::digest(key.as_str().as_bytes())),
        size: total,
        stored_size: stored,
        etag: metadata.etag.clone(),
        last_modified_ms: metadata.last_modified_ms,
        payload_sha256,
        metadata,
        encryption,
    };
    write_header(file, &header)?;
    Ok(header)
}

pub(super) fn write_header(file: &mut File, header: &EnvelopeHeader) -> Result<(), BackendError> {
    let canonical = serde_json::to_vec(header).map_err(|_| BackendError::Corrupt)?;
    let record = HeaderRecord {
        header: header.clone(),
        header_sha256: hex::encode(Sha256::digest(&canonical)),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| BackendError::Corrupt)?;
    if bytes.len().saturating_add(12) > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| BackendError::Unavailable)?;
    file.write_all(MAGIC)
        .and_then(|()| file.write_all(&(bytes.len() as u32).to_be_bytes()))
        .and_then(|()| file.write_all(&bytes))
        .map_err(|_| BackendError::Unavailable)
}

pub(super) fn read_header_from_file(
    file: &mut File,
    key: &ObjectKey,
) -> Result<EnvelopeHeader, BackendError> {
    let metadata = file.metadata().map_err(|_| BackendError::Unavailable)?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 {
        return Err(BackendError::Corrupt);
    }
    let mut prefix = [0_u8; 12];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut prefix))
        .map_err(|_| BackendError::Corrupt)?;
    if &prefix[..8] != MAGIC {
        return Err(BackendError::Corrupt);
    }
    let length = u32::from_be_bytes(
        prefix[8..12]
            .try_into()
            .map_err(|_| BackendError::Corrupt)?,
    ) as usize;
    if length == 0 || length.saturating_add(12) > HEADER_BYTES {
        return Err(BackendError::Corrupt);
    }
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(|_| BackendError::Corrupt)?;
    let record: HeaderRecord = serde_json::from_slice(&bytes).map_err(|_| BackendError::Corrupt)?;
    let canonical = serde_json::to_vec(&record.header).map_err(|_| BackendError::Corrupt)?;
    if record.header_sha256 != hex::encode(Sha256::digest(canonical))
        || record.header.schema_version != FORMAT_SCHEMA
        || record.header.key_sha256 != hex::encode(Sha256::digest(key.as_str().as_bytes()))
        || record.header.etag != record.header.metadata.etag
        || record.header.size != record.header.metadata.size
        || record.header.last_modified_ms != record.header.metadata.last_modified_ms
        || metadata.len() != HEADER_BYTES as u64 + record.header.stored_size
        || !valid_envelope_header(&record.header)
    {
        return Err(BackendError::Corrupt);
    }
    Ok(record.header)
}

pub(super) fn valid_envelope_header(header: &EnvelopeHeader) -> bool {
    let valid_sha256 = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if !valid_sha256(&header.key_sha256)
        || !valid_sha256(&header.payload_sha256)
        || !valid_etag(&header.etag)
        || header.last_modified_ms < 0
        || header.metadata.ssec_key_md5
            != header
                .encryption
                .as_ref()
                .map(|encryption| encryption.ssec_key_md5.clone())
    {
        return false;
    }
    match &header.encryption {
        None => header.stored_size == header.size,
        Some(encryption) => {
            let chunks = header.size.div_ceil(CHUNK_BYTES as u64);
            let expected = header
                .size
                .checked_add(chunks.saturating_mul(AEAD_TAG_BYTES as u64));
            expected == Some(header.stored_size)
                && encryption.algorithm == "xchacha20poly1305-chunked-v1"
                && encryption.chunk_size == CHUNK_BYTES as u32
                && canonical_uuid_v7(&encryption.object_version)
                && decode_nonce(&encryption.nonce).is_ok()
                && base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(&encryption.verifier)
                    .is_ok_and(|bytes| bytes.len() == AEAD_TAG_BYTES)
                && valid_lower_hex(&encryption.ssec_key_md5, 32)
        }
    }
}

pub(super) fn read_header(
    parent: &OwnedFd,
    name: &str,
    key: &ObjectKey,
) -> Result<EnvelopeHeader, BackendError> {
    let fd = open_regular(parent, name)?;
    read_header_from_file(&mut File::from(fd), key)
}

pub(super) fn read_optional_header(
    parent: &OwnedFd,
    name: &str,
    key: &ObjectKey,
) -> Result<Option<EnvelopeHeader>, BackendError> {
    match read_header(parent, name, key) {
        Ok(header) => Ok(Some(header)),
        Err(BackendError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn encryption_header(
    key: &ObjectKey,
    customer_key: &CustomerKey,
) -> Result<EncryptionHeader, BackendError> {
    let mut nonce = [0_u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let object_version = uuid::Uuid::now_v7().hyphenated().to_string();
    let cipher = XChaCha20Poly1305::new(Key::from_slice(customer_key.bytes()));
    let verifier_nonce = chunk_nonce(nonce, u64::MAX);
    let verifier = cipher
        .encrypt(
            XNonce::from_slice(&verifier_nonce),
            Payload {
                msg: &[],
                aad: &verifier_aad(key, &object_version),
            },
        )
        .map_err(|_| BackendError::Corrupt)?;
    Ok(EncryptionHeader {
        algorithm: "xchacha20poly1305-chunked-v1".to_owned(),
        chunk_size: CHUNK_BYTES as u32,
        object_version,
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
        verifier: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier),
        ssec_key_md5: hex::encode(Md5::digest(customer_key.bytes())),
    })
}

pub(super) fn verify_customer(
    header: &EnvelopeHeader,
    customer_key: Option<&CustomerKey>,
) -> Result<(), BackendError> {
    match (&header.encryption, customer_key) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(BackendError::CustomerKeyInvalid),
        (Some(encryption), Some(customer)) => {
            if encryption.algorithm != "xchacha20poly1305-chunked-v1"
                || encryption.chunk_size != CHUNK_BYTES as u32
                || !canonical_uuid_v7(&encryption.object_version)
            {
                return Err(BackendError::Corrupt);
            }
            let nonce = decode_nonce(&encryption.nonce)?;
            let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&encryption.verifier)
                .map_err(|_| BackendError::Corrupt)?;
            let cipher = XChaCha20Poly1305::new(Key::from_slice(customer.bytes()));
            let actual = cipher
                .encrypt(
                    XNonce::from_slice(&chunk_nonce(nonce, u64::MAX)),
                    Payload {
                        msg: &[],
                        aad: &verifier_aad_from_digest(
                            &header.key_sha256,
                            &encryption.object_version,
                        ),
                    },
                )
                .map_err(|_| BackendError::CustomerKeyInvalid)?;
            if actual != verifier {
                return Err(BackendError::CustomerKeyInvalid);
            }
            Ok(())
        }
    }
}

pub(super) fn stream_payload(
    file: File,
    header: EnvelopeHeader,
    customer_key: Option<CustomerKey>,
    range: Option<ObjectRange>,
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
) -> Result<(), BackendError> {
    let mut reader = match range {
        Some(range) => PayloadReader::range(file, header, customer_key, range)?,
        None => PayloadReader::full(file, header, customer_key)?,
    };
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| BackendError::Corrupt)?;
        if read == 0 {
            break;
        }
        sender
            .blocking_send(Ok(Bytes::copy_from_slice(&buffer[..read])))
            .map_err(|_| BackendError::Unavailable)?;
    }
    Ok(())
}

pub(super) struct PayloadReader {
    file: File,
    header: EnvelopeHeader,
    customer_key: Option<CustomerKey>,
    next_chunk: u64,
    current: Vec<u8>,
    current_offset: usize,
    remaining: u64,
    verify_full: bool,
    hasher: Sha256,
}

impl PayloadReader {
    pub(super) fn full(
        mut file: File,
        header: EnvelopeHeader,
        customer_key: Option<CustomerKey>,
    ) -> Result<Self, BackendError> {
        file.seek(SeekFrom::Start(HEADER_BYTES as u64))
            .map_err(|_| BackendError::Corrupt)?;
        Ok(Self {
            remaining: header.size,
            file,
            header,
            customer_key,
            next_chunk: 0,
            current: Vec::new(),
            current_offset: 0,
            verify_full: true,
            hasher: Sha256::new(),
        })
    }

    fn range(
        mut file: File,
        header: EnvelopeHeader,
        customer_key: Option<CustomerKey>,
        range: ObjectRange,
    ) -> Result<Self, BackendError> {
        let first_chunk = range.start / CHUNK_BYTES as u64;
        let within = (range.start % CHUNK_BYTES as u64) as usize;
        if header.encryption.is_none() {
            file.seek(SeekFrom::Start(HEADER_BYTES as u64))
                .map_err(|_| BackendError::Corrupt)?;
            let mut remaining = header.size;
            let mut buffer = vec![0_u8; CHUNK_BYTES];
            let mut digest = Sha256::new();
            while remaining != 0 {
                let requested = buffer.len().min(remaining as usize);
                let count = file
                    .read(&mut buffer[..requested])
                    .map_err(|_| BackendError::Corrupt)?;
                if count == 0 {
                    return Err(BackendError::Corrupt);
                }
                digest.update(&buffer[..count]);
                remaining -= count as u64;
            }
            if hex::encode(digest.finalize()) != header.payload_sha256 {
                return Err(BackendError::Corrupt);
            }
        }
        let offset = if header.encryption.is_some() {
            HEADER_BYTES as u64 + first_chunk * (CHUNK_BYTES as u64 + AEAD_TAG_BYTES as u64)
        } else {
            HEADER_BYTES as u64 + range.start
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| BackendError::Corrupt)?;
        let mut reader = Self {
            remaining: range.end - range.start + 1,
            file,
            header,
            customer_key,
            next_chunk: first_chunk,
            current: Vec::new(),
            current_offset: 0,
            verify_full: false,
            hasher: Sha256::new(),
        };
        if reader.header.encryption.is_some() {
            reader.load_chunk().map_err(|_| BackendError::Corrupt)?;
            reader.current_offset = within;
        }
        Ok(reader)
    }

    fn load_chunk(&mut self) -> std::io::Result<()> {
        if self.header.encryption.is_none() {
            return Ok(());
        }
        let chunk_start = self.next_chunk.saturating_mul(CHUNK_BYTES as u64);
        if chunk_start >= self.header.size {
            self.current.clear();
            self.current_offset = 0;
            return Ok(());
        }
        let plain_len = (self.header.size - chunk_start).min(CHUNK_BYTES as u64) as usize;
        let mut ciphertext = vec![0_u8; plain_len + AEAD_TAG_BYTES];
        self.file.read_exact(&mut ciphertext)?;
        let encryption = self
            .header
            .encryption
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing local object encryption metadata"))?;
        let nonce = decode_nonce(&encryption.nonce)
            .map_err(|_| std::io::Error::other("invalid local object nonce"))?;
        let customer = self
            .customer_key
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing local object customer key"))?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(customer.bytes()));
        self.current = cipher
            .decrypt(
                XNonce::from_slice(&chunk_nonce(nonce, self.next_chunk)),
                Payload {
                    msg: &ciphertext,
                    aad: &chunk_aad_from_digest(
                        &self.header.key_sha256,
                        &encryption.object_version,
                        self.header.size,
                        self.next_chunk,
                    ),
                },
            )
            .map_err(|_| std::io::Error::other("local object authentication failed"))?;
        self.current_offset = 0;
        self.next_chunk += 1;
        Ok(())
    }
}

impl Read for PayloadReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            if self.verify_full {
                let actual = std::mem::replace(&mut self.hasher, Sha256::new()).finalize();
                if hex::encode(actual) != self.header.payload_sha256 {
                    return Err(std::io::Error::other("local object checksum failed"));
                }
                self.verify_full = false;
            }
            return Ok(0);
        }
        if self.header.encryption.is_none() {
            let count = output.len().min(self.remaining as usize);
            let read = self.file.read(&mut output[..count])?;
            if read == 0 {
                return Err(std::io::Error::other("local object is truncated"));
            }
            self.remaining -= read as u64;
            if self.verify_full {
                self.hasher.update(&output[..read]);
            }
            return Ok(read);
        }
        if self.current_offset == self.current.len() {
            self.load_chunk()?;
        }
        let available = self.current.len().saturating_sub(self.current_offset);
        let count = output.len().min(available).min(self.remaining as usize);
        if count == 0 {
            return Err(std::io::Error::other("local object is truncated"));
        }
        output[..count]
            .copy_from_slice(&self.current[self.current_offset..self.current_offset + count]);
        self.current_offset += count;
        self.remaining -= count as u64;
        if self.verify_full {
            self.hasher.update(&output[..count]);
        }
        Ok(count)
    }
}

pub(super) struct MultipartConcatReader {
    pub(super) readers: VecDeque<PayloadReader>,
}

impl Read for MultipartConcatReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let Some(reader) = self.readers.front_mut() else {
                return Ok(0);
            };
            let count = reader.read(output)?;
            if count != 0 {
                return Ok(count);
            }
            self.readers.pop_front();
        }
    }
}
