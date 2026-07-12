use super::{internal, update_error, RunLifecycleOutcome, RunLifecycleService};
use crate::{
    config::SandboxBackendMode,
    tools::{
        runtime::ToolContext,
        sandbox::{
            cancel_run_sandbox_resources, cleanup_staged_writes_for_run,
            cleanup_staged_writes_for_run_backend, SandboxChannelWorkspaceBackend,
        },
    },
    types::{AgentEvent, AgentRunStatus},
};
use chrono::Utc;

impl RunLifecycleService {
    pub async fn cancel(
        &self,
        run_id: &str,
    ) -> Result<RunLifecycleOutcome, super::RunLifecycleError> {
        let cancelled = self
            .store
            .update_run_status(run_id, AgentRunStatus::Cancelled)
            .await
            .map_err(update_error)?;
        if let Some(run) = self.store.get_run(run_id).await {
            let workspace_root = effective_workspace_root(&self.config, &run.project_id);
            cancel_run_sandbox_resources(&self.config, &self.store, &run, workspace_root.clone())
                .await
                .map_err(internal)?;
            if self.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes
                && run.sandbox_id.is_some()
            {
                let mut ctx = ToolContext::new(self.store.clone(), run, workspace_root);
                ctx.remote_workspace = true;
                ctx.runtime_storage_dir = self.config.runtime_storage_dir.clone();
                let backend = SandboxChannelWorkspaceBackend::from_runtime_config(&self.config)
                    .map_err(internal)?;
                cleanup_staged_writes_for_run_backend(&backend, &ctx, run_id)
                    .await
                    .map_err(internal)?;
            } else {
                cleanup_staged_writes_for_run(&workspace_root, run_id);
            }
        }
        self.store
            .append_event(AgentEvent::RunCompleted {
                run_id: run_id.to_string(),
                status: "cancelled".to_string(),
                summary: "Run cancelled.".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)?;
        Ok(RunLifecycleOutcome {
            run_id: cancelled.id,
            status: "cancelled".to_string(),
        })
    }
}

fn effective_workspace_root(
    config: &crate::config::RuntimeConfig,
    project_id: &str,
) -> std::path::PathBuf {
    match config.sandbox_backend_mode {
        SandboxBackendMode::PhaseAContract => {
            let safe = project_id
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            config.workspace_root.join(safe)
        }
        SandboxBackendMode::Kubernetes => config.workspace_root.clone(),
    }
}
