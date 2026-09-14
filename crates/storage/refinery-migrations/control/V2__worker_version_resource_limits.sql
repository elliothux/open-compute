-- Standard resource limits materialized with every immutable Version (W2).
-- The default backfills pre-W2 development rows with the Standard profile defaults.
ALTER TABLE worker_versions ADD COLUMN resource_limits_json BLOB NOT NULL
DEFAULT X'7B226370754D73223A33303030302C227375625265717565737473223A31303030307D';
