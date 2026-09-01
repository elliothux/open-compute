# Concepts

A Queue has an identity in the control catalog. Message rows live in `scheduler.sqlite`. A producer binding writes `send` into that authority. A push consumer claims a batch and dispatches it to the Worker's `queue` handler.

Each Queue has at most one active push consumer at a time. Delivery is at-least-once: a crash before ack retries. There is no global FIFO — retry, concurrency, and crashes can reorder.

## Unknown dispatch

If the native dispatch outcome is unknown (process / workerd gone before confirmation), the lease is retained and the tenant `max_retries` budget is **not** consumed. Recovery may deliver again with the **same attempt number** (`OC-QUEUE-001`). This is not exactly-once.

## Same as Cloudflare

[Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) for send / batch / delay / content types / ack / retry / metrics. Per-message first-call-wins.

## Intentional differences

**`OC-QUEUE-001`**: single-node durability, at-least-once, no global FIFO, unknown dispatch keeps the lease. No pull consumer, no Cloudflare global throughput plan.
