//! Private AI Search operation dispatch.

use super::*;

impl AiSearchBindingService {
    pub(super) async fn execute_call(
        &self,
        authority: Authority,
        call: JsonCall,
    ) -> Result<Response, PlatformError> {
        let result = self.execute_value(&authority, call).await?;
        json_response(&json!({"schemaVersion": 1, "result": result}))
    }

    pub(super) async fn execute_value(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let write = matches!(
            call.operation.as_str(),
            "namespace.create"
                | "namespace.openComputeCreateManual"
                | "namespace.delete"
                | "instance.update"
                | "instance.move"
                | "instance.purgeCache"
                | "items.delete"
                | "items.openComputeUpsert"
                | "item.sync"
                | "jobs.create"
                | "job.cancel"
        );
        require_permission(authority, write)?;
        let metric_operation = metric_operation(&call.operation);
        let result = match call.operation.as_str() {
            "namespace.list" => self.namespace_list(authority, call)?,
            "namespace.create" => self.namespace_create(authority, call)?,
            "namespace.openComputeCreateManual" => {
                self.namespace_open_compute_create_manual(authority, call)?
            }
            "namespace.delete" => self.namespace_delete(authority, call).await?,
            "namespace.search" => self.namespace_search(authority, call).await?,
            "namespace.chatCompletions" => self.namespace_chat(authority, call).await?,
            "instance.search" => self.instance_search(authority, call).await?,
            "instance.chatCompletions" => self.instance_chat(authority, call).await?,
            "instance.update" => self.instance_update(authority, call).await?,
            "instance.move" => self.instance_move(authority, call).await?,
            "instance.purgeCache" => {
                // Search results are never cached locally, so a purge has nothing to drop.
                self.resolve_instance(authority, call.instance.as_deref())?;
                json!({})
            }
            "instance.info" => self.instance_info_call(authority, &call)?,
            "instance.stats" => self.instance_stats(authority, &call)?,
            "items.list" => self.items_list(authority, call)?,
            "items.delete" => self.items_delete(authority, call).await?,
            "items.openComputeUpsert" => self.manual_upsert(authority, call).await?,
            "item.info" => self.item_info_call(authority, call)?,
            "item.sync" => self.item_sync(authority, call).await?,
            "item.logs" => self.item_logs(authority, call)?,
            "item.chunks" => self.item_chunks(authority, call)?,
            "jobs.list" => self.jobs_list(authority, call)?,
            "jobs.create" => self.jobs_create(authority, call).await?,
            "job.info" => self.job_info_call(authority, call)?,
            "job.logs" => self.job_logs(authority, call)?,
            "job.cancel" => self.job_cancel(authority, call)?,
            _ => return Err(protocol()),
        };
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_request(metric_operation, true);
        }
        Ok(result)
    }
}
