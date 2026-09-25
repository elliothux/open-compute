CREATE TABLE migration_v11_extension_guard (
  invalid INTEGER NOT NULL CHECK (invalid = 0)
);
INSERT INTO migration_v11_extension_guard (invalid)
SELECT 1 FROM version_services WHERE target_kind = 'extension' LIMIT 1;
DROP TABLE migration_v11_extension_guard;

ALTER TABLE version_services ADD COLUMN target_policy_revision TEXT
CHECK (
  (target_kind = 'worker' AND target_policy_revision IS NULL)
  OR
  (target_kind = 'extension'
    AND length(target_policy_revision) = 64
    AND target_policy_revision NOT GLOB '*[^0-9a-f]*')
);
