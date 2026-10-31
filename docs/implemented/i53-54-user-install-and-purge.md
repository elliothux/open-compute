# I53–54 user install and explicit purge

Issues #53 and #54 replace the operator lifecycle with one Day1 model:

- a non-root installer owns `$HOME/.local/bin/ocd` and its receipt by default; root alone defaults to `/usr/local`, and explicit environment destinations remain authoritative;
- `ocd setup --yes` creates host-standard user config/data roots and a login-scoped systemd user unit or LaunchAgent; only `setup --system` selects `/etc/open-compute`, `/var/lib/open-compute`, and a boot-level service;
- config discovery is explicit path, project `compute.toml`, default user config, then system config, and never falls past a present invalid candidate;
- registry schema 2 records the exact owning executable, configuration checksum, data path, and non-secret object-authority location. Earlier development registry records are rejected rather than upgraded;
- `instance unregister` is the sole non-data-removing service-registration command;
- `uninstall` acts only on registrations owned by its receipt executable, stops and unregisters them, prints retained paths, and preserves data by default;
- `purge` and `uninstall --purge` resolve and print the entire destructive plan before mutation, require exact path-bound confirmation, retain S3, and fail closed on changed config, symlinks, unsafe roots, ambiguous entries, live sockets, or overlaps. A disjoint Local object root is deleted only when its marker, platform identity, and stored authority fingerprint prove ownership; otherwise it is retained and reported.
- purge removes service definitions first but keeps registry recovery authority until local data roots have been deleted, so a filesystem failure can be retried without rediscovering ownership from a missing config;

The implementation intentionally has no old registry-schema reader, `instance remove` alias, alternate uninstall path, or data backfill. Existing pre-schema-2 local development registry data must be removed and instances set up again.

Focused evidence includes the installer/release-tool suite and service lifecycle unit coverage. Final repository acceptance and the scoped Cloudflare compatibility review are recorded in the completing task rather than treated as permanent compatibility claims.
