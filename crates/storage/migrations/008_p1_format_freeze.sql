-- P1 establishes the explicit offline-upgrade boundary. The schema transition is
-- intentionally metadata-only: the checksummed migration row and user_version
-- are the durable fence that prevents a P0 binary from serving after upgrade.
SELECT 1;
