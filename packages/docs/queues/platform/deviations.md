# Behavior differences

The Queues JavaScript API matches Cloudflare. Durability comes from `scheduler.sqlite` on this node.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | Same: `send` / `sendBatch`, `contentType` (json / text / bytes / v8), `delaySeconds`, `metrics`, consumer `MessageBatch` / `ack` / `retry` |
| Durability | Global replication | Local `scheduler.sqlite` on the node running ocd |
| Delivery | At-least-once | At-least-once |
| Global FIFO | Available | Not provided |
| Unknown native dispatch | — | May retain the lease; duplicate attempt numbers possible |
| Pull consumer | Available | Not provided |
| Binding | wrangler `queues` | Producer `{ type, id, permissions? }`; consumer is the Worker `queue` handler |

See [Compatibility](/platform/compatibility).
