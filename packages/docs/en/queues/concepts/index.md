# Concepts

A Queue has an identity in the control catalog. Message rows live in `scheduler.sqlite`. A producer binding writes `send` into that authority. A push consumer claims a batch and dispatches it to the Worker's `queue` handler.

Each Queue has at most one active push consumer at a time. Delivery is at-least-once: a crash before ack retries. Global FIFO is not provided. Retry, concurrency, and crashes can reorder.

## Unknown dispatch

If the native dispatch outcome is unknown (process / workerd gone before confirmation), the lease is retained and the tenant `max_retries` budget is **not** consumed. Recovery may deliver again with the **same attempt number**. This is not exactly-once.

[Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) for send / batch / delay / content types / ack / retry / metrics match Cloudflare. Per-message first-call-wins.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | Same: send / batch / delay / content types / ack / retry / metrics |
| Durability | Global replication | Local `scheduler.sqlite` on the node running ocd |
| Delivery | At-least-once | At-least-once; unknown dispatch keeps the lease |
| Global FIFO | Available | Not provided |
| Pull consumer | Available | Not provided |
