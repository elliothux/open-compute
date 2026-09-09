use super::*;

impl MetricsRegistry {
    /// Prometheus text exposition, deterministically ordered.
    pub fn render(&self, status: &PlatformStatus) -> String {
        let g = self.lock();
        let mut out = String::new();
        write_help(
            &mut out,
            "platform_info",
            "gauge",
            "Platform build identity",
        );
        writeln!(
            &mut out,
            "platform_info{{version=\"{}\",workerd_version=\"{}\"}} 1",
            escape(&g.version),
            escape(&g.workerd_version)
        )
        .ok();
        write_help(
            &mut out,
            "platform_ready",
            "gauge",
            "1 if the component is healthy",
        );
        for name in component_order() {
            let ready = status
                .components
                .iter()
                .find(|c| c.name == name)
                .is_some_and(|c| c.state == ComponentState::Healthy);
            writeln!(
                &mut out,
                "platform_ready{{component=\"{}\"}} {}",
                name.as_str(),
                u64::from(ready)
            )
            .ok();
        }
        write_help(
            &mut out,
            "platform_start_total",
            "counter",
            "Startup stage outcomes",
        );
        for result in StartResult::ALL {
            for stage in StartStage::ALL {
                let i = start_index(result, stage);
                writeln!(
                    &mut out,
                    "platform_start_total{{result=\"{}\",stage=\"{}\"}} {}",
                    result.as_str(),
                    stage.as_str(),
                    g.start_total[i]
                )
                .ok();
            }
        }
        write_help(
            &mut out,
            "workerd_process_up",
            "gauge",
            "1 if workerd is up",
        );
        writeln!(&mut out, "workerd_process_up {}", g.process_up).ok();
        write_help(
            &mut out,
            "workerd_restart_total",
            "counter",
            "workerd restart counts",
        );
        for reason in RestartReason::ALL {
            writeln!(
                &mut out,
                "workerd_restart_total{{reason=\"{}\"}} {}",
                reason.as_str(),
                g.restart_total[restart_index(reason)]
            )
            .ok();
        }
        write_help(
            &mut out,
            "workerd_start_duration_seconds",
            "gauge",
            "Last workerd start duration",
        );
        writeln!(
            &mut out,
            "workerd_start_duration_seconds {}",
            g.start_duration
        )
        .ok();
        write_p1_metrics(&mut out, &g.p1);
        write_search_metrics(&mut out, &g.search, g.object_backend);
        write_help(
            &mut out,
            "sqlite_operation_duration_seconds",
            "gauge",
            "Last sqlite operation duration",
        );
        for op in SqliteOp::ALL {
            writeln!(
                &mut out,
                "sqlite_operation_duration_seconds{{database=\"control\",operation=\"{}\"}} {}",
                op.as_str(),
                g.sqlite_duration[sqlite_index(op)]
            )
            .ok();
        }
        write_help(
            &mut out,
            "object_storage_request_total",
            "counter",
            "Object-storage request counts",
        );
        for op in ObjectOp::ALL {
            for result in [ObjectResult::Failure, ObjectResult::Success] {
                writeln!(
                    &mut out,
                    "object_storage_request_total{{backend=\"{}\",operation=\"{}\",result=\"{}\"}} {}",
                    g.object_backend.as_str(),
                    op.as_str(),
                    result.as_str(),
                    g.object_total[object_total_index(op, result)]
                )
                .ok();
            }
        }
        write_help(
            &mut out,
            "object_storage_request_duration_seconds",
            "gauge",
            "Last object-storage request duration",
        );
        for op in ObjectOp::ALL {
            writeln!(
                &mut out,
                "object_storage_request_duration_seconds{{backend=\"{}\",operation=\"{}\"}} {}",
                g.object_backend.as_str(),
                op.as_str(),
                g.object_duration[object_op_index(op)]
            )
            .ok();
        }
        write_help(
            &mut out,
            "artifact_cache_bytes",
            "gauge",
            "Cache byte total",
        );
        writeln!(&mut out, "artifact_cache_bytes {}", g.cache_bytes).ok();
        write_help(
            &mut out,
            "artifact_cache_entries",
            "gauge",
            "Cache entry total",
        );
        writeln!(&mut out, "artifact_cache_entries {}", g.cache_entries).ok();
        write_help(
            &mut out,
            "artifact_cache_hit_total",
            "counter",
            "Cache hit total",
        );
        writeln!(&mut out, "artifact_cache_hit_total {}", g.cache_hits).ok();
        write_help(
            &mut out,
            "artifact_integrity_error_total",
            "counter",
            "Integrity error total",
        );
        writeln!(
            &mut out,
            "artifact_integrity_error_total {}",
            g.integrity_errors
        )
        .ok();
        write_resource_metrics(&mut out, &g);
        write_kv_metrics(&mut out, &g);
        write_r2_metrics(&mut out, &g);
        write_d1_metrics(&mut out, &g);
        write_do_metrics(&mut out, &g);
        write_scheduler_metrics(&mut out, &g);
        write_service_metrics(&mut out, &g);
        write_queue_metrics(&mut out, &g);
        workflow::write_workflow_metrics(&mut out, &g);
        write_cache_images_metrics(&mut out, &g);
        write_observability_metrics(&mut out, &g);
        let _ = self.max_label;
        out
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
