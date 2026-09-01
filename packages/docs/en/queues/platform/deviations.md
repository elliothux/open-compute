# Deviations

Registered ID: **`OC-QUEUE-001`**.

Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without global FIFO. An unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number.

That is why 63 target members are `supported_with_deviation`.

See [Compatibility](/en/platform/compatibility) and `docs/references/p1-deviations.md`.
