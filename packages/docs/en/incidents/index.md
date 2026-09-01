# Incident handbook

When something breaks, follow the symptom. Do not start in the source tree or an internal crate. This chapter is one page per symptom: stop conditions, allowed mutations, rollback, and verification are on each page. Commands match embedded `ocd docs <name>`.

Path examples use `/etc/open-compute/config.toml`. Some embedded runbooks write `platform.toml`; `--config` only needs an absolute path. Fresh-host restore and master-key recovery use a separate `recovery.toml` / `recovery-master.key`, not the daily config.

Unless a section explicitly allows it, do not: overwrite an existing data-dir, force, self-heal SQLite, search `PATH` or download workerd, treat a failed upload as committed, or generate a new master key over an old platform.

Open the matching page by symptom:

- [Current-release restore](/en/incidents/current-release)
- [Fresh-host restore](/en/incidents/fresh-host)
- [Disk pressure](/en/incidents/disk)
- [SQLite corruption](/en/incidents/sqlite)
- [S3 outage](/en/incidents/s3)
- [workerd crash loop](/en/incidents/workerd)
- [Master-key loss](/en/incidents/master-key)
- [Scheduler recovery](/en/incidents/scheduler)
- [Collect a support bundle](/en/incidents/support-bundle)
