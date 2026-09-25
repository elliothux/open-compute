---
title: "Incident handbook"
---

When something breaks, follow the symptom. Do not start in the source tree or an internal crate. This chapter is one page per symptom: stop conditions, allowed mutations, rollback, and verification are on each page. Commands match embedded `ocd docs <name>`.

Path examples use `/var/lib/open-compute/instances/default/compute.toml`. `--config` selects that explicit instance configuration; `--system` selects its OCD scope. For recovery, restore the scoped manifest and config first, then temporarily point that config at the operator-backed-up master key outside the empty instance target.

Unless a section explicitly allows it, do not: overwrite an existing data-dir, force, self-heal SQLite, search `PATH` or download workerd, treat a failed upload as committed, or generate a new master key over an old platform.

Open the matching page by symptom:

- [Current-release restore](/docs/ocd/incidents/current-release/)
- [Fresh-host restore](/docs/ocd/incidents/fresh-host/)
- [Disk pressure](/docs/ocd/incidents/disk/)
- [SQLite corruption](/docs/ocd/incidents/sqlite/)
- [S3 outage](/docs/ocd/incidents/s3/)
- [workerd crash loop](/docs/ocd/incidents/workerd/)
- [Master-key loss](/docs/ocd/incidents/master-key/)
- [Scheduler recovery](/docs/ocd/incidents/scheduler/)
- [Collect a support bundle](/docs/ocd/incidents/support-bundle/)
