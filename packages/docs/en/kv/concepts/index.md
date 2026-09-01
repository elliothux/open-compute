# Concepts

Each KV namespace is its own SQLite database (`data/kv/<account>/<namespace>/`). Keys sort as UTF-8 bytes with no Unicode normalization. `put` is an atomic replacement in one transaction; a failed write leaves the previous value. Expired keys are immediately absent on read and list.

`cacheTtl` is accepted, but there is no colo cache here: a read is the current SQLite value.

## Same as Cloudflare

Keys are at most 512 bytes, and cannot be empty, `.`, or `..`. Values are at most 25 MiB. Serialized metadata is at most 1024 bytes. Minimum `expirationTtl` is 60 seconds. Bulk `get` is at most 100 keys. `list` defaults to / caps at 1000. Types and error shapes match the [KV API](https://developers.cloudflare.com/kv/api/).

## Intentional differences

**`OC-KV-001`**: single-node SQLite authority. No Cloudflare global replication or propagation timing. There is no eventual-consistency window to measure, because there is no second replica. No jurisdiction. No REST bulk write/delete product.

Next: [Guides](/en/kv/guides/).
