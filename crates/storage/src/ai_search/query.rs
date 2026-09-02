//! Catalog queries, configuration updates, and full-index reindex planning.

use super::*;

impl AiSearchStore {
    /// Query the selected FTS5 authority with a pre-escaped MATCH expression,
    /// returning active chunks in stable BM25 order.
    pub fn keyword_chunks(
        &self,
        fts_query: &str,
        trigram: bool,
        limit: u32,
    ) -> Result<Vec<AiSearchChunkRecord>, PlatformError> {
        let generation = self.inspect()?.active_index_generation;
        self.keyword_chunks_at(generation, fts_query, trigram, limit)
    }

    /// Query one explicitly pinned index generation for a consistent search snapshot.
    pub fn keyword_chunks_at(
        &self,
        index_generation: u64,
        fts_query: &str,
        trigram: bool,
        limit: u32,
    ) -> Result<Vec<AiSearchChunkRecord>, PlatformError> {
        if fts_query.is_empty() || fts_query.len() > 8_192 || limit == 0 || limit > 100_000 {
            return Err(limit_error());
        }
        let sql = if trigram {
            "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                    c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
               FROM chunks_fts_trigram f JOIN chunks c ON c.id=f.chunk_id
               JOIN items i ON i.id=c.item_id
              WHERE chunks_fts_trigram MATCH ?1
                AND c.index_generation=?2
                AND c.item_generation=i.active_generation
              ORDER BY bm25(chunks_fts_trigram), c.id LIMIT ?3"
        } else {
            "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                    c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
               FROM chunks_fts_porter f JOIN chunks c ON c.id=f.chunk_id
               JOIN items i ON i.id=c.item_id
              WHERE chunks_fts_porter MATCH ?1
                AND c.index_generation=?2
                AND c.item_generation=i.active_generation
              ORDER BY bm25(chunks_fts_porter), c.id LIMIT ?3"
        };
        let connection = self.lock()?;
        let mut statement = connection.prepare(sql).map_err(sql_error)?;
        let dimensions = self.active_dimensions;
        let vector_enabled = self.active_vector_enabled;
        let rows = statement
            .query_map(
                params![fts_query, to_i64(index_generation)?, i64::from(limit)],
                |row| decode_chunk(row, dimensions, vector_enabled),
            )
            .map_err(sql_error)?;
        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row.map_err(sql_error)?);
        }
        Ok(chunks)
    }

    /// Stream ranked FTS rows from one active generation without materializing
    /// the whole candidate set. Returning `false` stops the scan early.
    pub fn scan_keyword_chunks_at(
        &self,
        index_generation: u64,
        fts_query: &str,
        trigram: bool,
        mut visit: impl FnMut(AiSearchChunkRecord) -> Result<bool, PlatformError>,
    ) -> Result<(), PlatformError> {
        if fts_query.is_empty() || fts_query.len() > 8_192 {
            return Err(limit_error());
        }
        let sql = if trigram {
            "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                    c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
               FROM chunks_fts_trigram f JOIN chunks c ON c.id=f.chunk_id
               JOIN items i ON i.id=c.item_id
              WHERE chunks_fts_trigram MATCH ?1 AND c.index_generation=?2
                AND c.item_generation=i.active_generation
              ORDER BY bm25(chunks_fts_trigram), c.id"
        } else {
            "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                    c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
               FROM chunks_fts_porter f JOIN chunks c ON c.id=f.chunk_id
               JOIN items i ON i.id=c.item_id
              WHERE chunks_fts_porter MATCH ?1 AND c.index_generation=?2
                AND c.item_generation=i.active_generation
              ORDER BY bm25(chunks_fts_porter), c.id"
        };
        let connection = self.lock()?;
        let mut statement = connection.prepare(sql).map_err(sql_error)?;
        let mut rows = statement
            .query(params![fts_query, to_i64(index_generation)?])
            .map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let chunk = decode_chunk(row, self.active_dimensions, self.active_vector_enabled)
                .map_err(sql_error)?;
            if !visit(chunk)? {
                break;
            }
        }
        Ok(())
    }

    /// Read one active chunk and its bounded adjacent context within the same item.
    pub fn active_chunk_context(
        &self,
        item_id: &str,
        ordinal: u32,
        radius: u8,
    ) -> Result<Vec<AiSearchChunkRecord>, PlatformError> {
        let generation = self.inspect()?.active_index_generation;
        let anchor: Option<String> = self
            .lock()?
            .query_row(
                "SELECT id FROM chunks WHERE item_id=?1 AND ordinal=?2
                  AND index_generation=?3 ORDER BY item_generation DESC LIMIT 1",
                params![item_id, i64::from(ordinal), to_i64(generation)?],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        let Some(anchor) = anchor else {
            return Ok(Vec::new());
        };
        self.active_chunk_context_at(generation, &anchor, radius)
    }

    /// Read adjacent chunks around one anchor from the same explicit index and item generation.
    pub fn active_chunk_context_at(
        &self,
        index_generation: u64,
        anchor_chunk_id: &str,
        radius: u8,
    ) -> Result<Vec<AiSearchChunkRecord>, PlatformError> {
        validate_identity(anchor_chunk_id)?;
        if radius > 3 {
            return Err(limit_error());
        }
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                        c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
                   FROM chunks anchor JOIN chunks c
                     ON c.item_id=anchor.item_id
                    AND c.item_generation=anchor.item_generation
                    AND c.index_generation=anchor.index_generation
                   JOIN items i ON i.id=c.item_id
                  WHERE anchor.id=?1 AND anchor.index_generation=?2
                    AND anchor.item_generation=i.active_generation
                    AND c.ordinal BETWEEN
                      MAX(0, anchor.ordinal - ?3) AND anchor.ordinal + ?3
                  ORDER BY c.ordinal",
            )
            .map_err(sql_error)?;
        let dimensions = self.active_dimensions;
        let vector_enabled = self.active_vector_enabled;
        let rows = statement
            .query_map(
                params![
                    anchor_chunk_id,
                    to_i64(index_generation)?,
                    i64::from(radius)
                ],
                |row| decode_chunk(row, dimensions, vector_enabled),
            )
            .map_err(sql_error)?;
        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row.map_err(sql_error)?);
        }
        Ok(chunks)
    }

    /// Return the active generation and item status for focused inspection.
    pub fn item_state(
        &self,
        item_id: &str,
    ) -> Result<Option<(String, Option<u64>)>, PlatformError> {
        let value: Option<(String, Option<i64>)> = self
            .lock()?
            .query_row(
                "SELECT status, active_generation FROM items WHERE id=?1",
                [item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        value
            .map(|(state, generation)| Ok((state, generation.map(to_u64).transpose()?)))
            .transpose()
    }

    /// Read one item with its active, or otherwise desired, immutable object identity.
    pub fn get_item(&self, item_id: &str) -> Result<Option<AiSearchItemRecord>, PlatformError> {
        validate_identity(item_id)?;
        self.lock()?
            .query_row(
                "SELECT i.id, i.key, i.status, i.active_generation, i.desired_generation,
                        i.metadata_json, i.created_at_ms, i.updated_at_ms,
                        g.object_key, g.object_sha256, g.object_size, g.content_type,
                        (SELECT COUNT(*) FROM chunks c
                          WHERE c.item_id=i.id AND c.item_generation=i.active_generation
                            AND c.index_generation=(SELECT active_index_generation
                              FROM instance_meta WHERE singleton=1))
                   FROM items i JOIN item_generations g ON g.item_id=i.id
                    AND g.generation=COALESCE(i.active_generation, i.desired_generation)
                  WHERE i.id=?1",
                [item_id],
                decode_item,
            )
            .optional()
            .map_err(sql_error)
    }

    /// Resolve one built-in source item by its exact source key.
    pub fn get_item_by_key(&self, key: &str) -> Result<Option<AiSearchItemRecord>, PlatformError> {
        if key.is_empty() || key.len() > 1_024 || key.chars().any(char::is_control) {
            return Err(limit_error());
        }
        self.lock()?
            .query_row(
                "SELECT i.id, i.key, i.status, i.active_generation, i.desired_generation,
                        i.metadata_json, i.created_at_ms, i.updated_at_ms,
                        g.object_key, g.object_sha256, g.object_size, g.content_type,
                        (SELECT COUNT(*) FROM chunks c
                          WHERE c.item_id=i.id AND c.item_generation=i.active_generation
                            AND c.index_generation=(SELECT active_index_generation
                              FROM instance_meta WHERE singleton=1))
                   FROM items i JOIN item_generations g ON g.item_id=i.id
                    AND g.generation=COALESCE(i.active_generation, i.desired_generation)
                  WHERE i.source='builtin' AND i.key=?1",
                [key],
                decode_item,
            )
            .optional()
            .map_err(sql_error)
    }

    /// Read one item joined to its exact desired generation source object.
    pub fn get_desired_item(
        &self,
        item_id: &str,
    ) -> Result<Option<AiSearchItemRecord>, PlatformError> {
        validate_identity(item_id)?;
        self.lock()?
            .query_row(
                "SELECT i.id, i.key, i.status, i.active_generation, i.desired_generation,
                        i.metadata_json, i.created_at_ms, i.updated_at_ms,
                        g.object_key, g.object_sha256, g.object_size, g.content_type,
                        (SELECT COUNT(*) FROM chunks c
                          WHERE c.item_id=i.id AND c.item_generation=i.active_generation
                            AND c.index_generation=(SELECT active_index_generation
                              FROM instance_meta WHERE singleton=1))
                   FROM items i JOIN item_generations g ON g.item_id=i.id
                    AND g.generation=i.desired_generation WHERE i.id=?1",
                [item_id],
                decode_item,
            )
            .optional()
            .map_err(sql_error)
    }

    /// List item catalog rows in stable update/id order.
    pub fn list_items(
        &self,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<AiSearchItemRecord>, u64), PlatformError> {
        if limit == 0 || limit > 100 {
            return Err(limit_error());
        }
        let connection = self.lock()?;
        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .map_err(sql_error)?;
        let mut statement = connection
            .prepare(
                "SELECT i.id, i.key, i.status, i.active_generation, i.desired_generation,
                        i.metadata_json, i.created_at_ms, i.updated_at_ms,
                        g.object_key, g.object_sha256, g.object_size, g.content_type,
                        (SELECT COUNT(*) FROM chunks c
                          WHERE c.item_id=i.id AND c.item_generation=i.active_generation
                            AND c.index_generation=(SELECT active_index_generation
                              FROM instance_meta WHERE singleton=1))
                   FROM items i JOIN item_generations g ON g.item_id=i.id
                    AND g.generation=COALESCE(i.active_generation, i.desired_generation)
                  ORDER BY i.updated_at_ms DESC, i.id LIMIT ?1 OFFSET ?2",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![i64::from(limit), to_i64(offset)?], decode_item)
            .map_err(sql_error)?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(sql_error)?);
        }
        Ok((items, to_u64(total)?))
    }

    /// Read one indexing job without exposing its claim token.
    pub fn get_job(&self, job_id: &str) -> Result<Option<AiSearchJobRecord>, PlatformError> {
        validate_identity(job_id)?;
        self.lock()?
            .query_row(
                "SELECT id, source, description, state, created_at_ms, started_at_ms,
                        ended_at_ms, updated_at_ms FROM index_jobs WHERE id=?1",
                [job_id],
                decode_job,
            )
            .optional()
            .map_err(sql_error)
    }

    /// List indexing jobs in stable newest-first order.
    pub fn list_jobs(
        &self,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<AiSearchJobRecord>, u64), PlatformError> {
        if limit == 0 || limit > 100 {
            return Err(limit_error());
        }
        let connection = self.lock()?;
        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM index_jobs", [], |row| row.get(0))
            .map_err(sql_error)?;
        let mut statement = connection
            .prepare(
                "SELECT id, source, description, state, created_at_ms, started_at_ms,
                        ended_at_ms, updated_at_ms FROM index_jobs
                  ORDER BY created_at_ms DESC, id LIMIT ?1 OFFSET ?2",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![i64::from(limit), to_i64(offset)?], decode_job)
            .map_err(sql_error)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(sql_error)?);
        }
        Ok((jobs, to_u64(total)?))
    }

    /// List active chunks for one item, or for the entire active index generation.
    pub fn active_chunks(
        &self,
        item_id: Option<&str>,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<AiSearchChunkRecord>, u64), PlatformError> {
        let generation = self.inspect()?.active_index_generation;
        self.active_chunks_at(generation, item_id, offset, limit)
    }

    /// List chunks from one explicitly pinned index generation.
    pub fn active_chunks_at(
        &self,
        index_generation: u64,
        item_id: Option<&str>,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<AiSearchChunkRecord>, u64), PlatformError> {
        if limit == 0 || limit > 100_000 {
            return Err(limit_error());
        }
        if let Some(item_id) = item_id {
            validate_identity(item_id)?;
        }
        let connection = self.lock()?;
        let total: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM chunks c JOIN items i ON i.id=c.item_id
                  WHERE c.index_generation=?2
                    AND c.item_generation=i.active_generation
                    AND (?1 IS NULL OR i.id=?1)",
                params![item_id, to_i64(index_generation)?],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let mut statement = connection
            .prepare(
                "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                        c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
                   FROM chunks c JOIN items i ON i.id=c.item_id
                  WHERE c.index_generation=?2
                    AND c.item_generation=i.active_generation
                    AND (?1 IS NULL OR i.id=?1)
                  ORDER BY i.id, c.ordinal LIMIT ?3 OFFSET ?4",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(
                params![
                    item_id,
                    to_i64(index_generation)?,
                    i64::from(limit),
                    to_i64(offset)?
                ],
                |row| decode_chunk(row, self.active_dimensions, self.active_vector_enabled),
            )
            .map_err(sql_error)?;
        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row.map_err(sql_error)?);
        }
        Ok((chunks, to_u64(total)?))
    }

    /// Stream all active chunks in one pinned index generation through a
    /// bounded caller-owned reducer.
    pub fn scan_active_chunks_at(
        &self,
        index_generation: u64,
        mut visit: impl FnMut(AiSearchChunkRecord) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT c.id, c.item_id, c.ordinal, c.start_byte, c.end_byte, c.text,
                        c.embedding_f32le, c.metadata_json, i.key, i.created_at_ms
                   FROM chunks c JOIN items i ON i.id=c.item_id
                  WHERE c.index_generation=?1 AND c.item_generation=i.active_generation
                  ORDER BY c.id",
            )
            .map_err(sql_error)?;
        let mut rows = statement
            .query([to_i64(index_generation)?])
            .map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            visit(
                decode_chunk(row, self.active_dimensions, self.active_vector_enabled)
                    .map_err(sql_error)?,
            )?;
        }
        Ok(())
    }

    /// Verify that a multi-stage search still observes the exact active fence
    /// captured before retrieval began.
    pub fn active_fence_matches(
        &self,
        index_generation: u64,
        active_epoch: u64,
    ) -> Result<bool, PlatformError> {
        self.lock()?
            .query_row(
                "SELECT active_index_generation=?1 AND active_epoch=?2
                   FROM instance_meta WHERE singleton=1",
                params![to_i64(index_generation)?, to_i64(active_epoch)?],
                |row| row.get(0),
            )
            .map_err(sql_error)
    }

    /// List sanitized item logs after an optional cursor.
    pub fn item_logs(
        &self,
        item_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<AiSearchLogRecord>, PlatformError> {
        validate_identity(item_id)?;
        if limit == 0 || limit > 100 {
            return Err(limit_error());
        }
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, message_code, 0, created_at_ms FROM item_logs
                  WHERE item_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3",
            )
            .map_err(sql_error)?;
        decode_logs(
            statement
                .query_map(
                    params![item_id, to_i64(after)?, i64::from(limit)],
                    decode_log,
                )
                .map_err(sql_error)?,
        )
    }

    /// List sanitized job logs with offset pagination.
    pub fn job_logs(
        &self,
        job_id: &str,
        offset: u64,
        limit: u32,
    ) -> Result<Vec<AiSearchLogRecord>, PlatformError> {
        validate_identity(job_id)?;
        if limit == 0 || limit > 100 {
            return Err(limit_error());
        }
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, message_code, message_type, created_at_ms FROM job_logs
                  WHERE job_id=?1 ORDER BY sequence LIMIT ?2 OFFSET ?3",
            )
            .map_err(sql_error)?;
        decode_logs(
            statement
                .query_map(
                    params![job_id, i64::from(limit), to_i64(offset)?],
                    decode_log,
                )
                .map_err(sql_error)?,
        )
    }
}
