ALTER TABLE r2_objects ADD COLUMN size_bytes INTEGER
  CHECK(size_bytes IS NULL OR size_bytes >= 0);
