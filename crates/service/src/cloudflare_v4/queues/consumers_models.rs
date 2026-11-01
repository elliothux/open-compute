#[derive(Serialize, schemars::JsonSchema)]
struct ConsumerRuntimeResponse {
    projection_exists: bool,
    backlog_messages: u64,
    backlog_bytes: u64,
    ready_messages: u64,
    claimed_batches: u64,
    claimed_messages: u64,
    dlq_pending: u64,
}
