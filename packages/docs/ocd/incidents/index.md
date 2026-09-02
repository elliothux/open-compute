# Incident handbook

When something breaks, follow the symptom. Do not start in the source tree or an internal crate. This chapter is one page per symptom: stop conditions, allowed mutations, rollback, and verification are on each page. Commands match embedded `ocd docs <name>`.

Path examples use `/etc/open-compute/config.toml`. Some embedded runbooks write `platform.toml`; `--config` only needs an absolute path. Fresh-host restore and master-key recovery use a separate `recovery.toml` / `recovery-master.key`, not the daily config.

Unless a section explicitly allows it, do not: overwrite an existing data-dir, force, self-heal SQLite, search `PATH` or download workerd, treat a failed upload as committed, or generate a new master key over an old platform.

Open the matching page by symptom:

- [Current-release restore](/ocd/incidents/current-release)
- [Fresh-host restore](/ocd/incidents/fresh-host)
- [Disk pressure](/ocd/incidents/disk)
- [SQLite corruption](/ocd/incidents/sqlite)
- [S3 outage](/ocd/incidents/s3)
- [workerd crash loop](/ocd/incidents/workerd)
- [Master-key loss](/ocd/incidents/master-key)
- [Scheduler recovery](/ocd/incidents/scheduler)
- [Collect a support bundle](/ocd/incidents/support-bundle)
