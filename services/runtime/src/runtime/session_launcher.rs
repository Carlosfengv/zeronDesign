use crate::{
    config::{RuntimeConfig, SandboxBackendMode},
    conversation::RuntimeStore,
    model_gateway::ModelClient,
    query_session::QuerySession,
    run_lifecycle::RunSessionLauncher,
    runtime::RuntimeSupervisor,
    tools::control_plane::control_plane_executor_for_config,
    types::{AgentEvent, AgentRunStatus},
};
use chrono::Utc;
use std::sync::Arc;

pub struct RuntimeSessionLauncher {
    config: RuntimeConfig,
    store: RuntimeStore,
    model: Arc<dyn ModelClient>,
    supervisor: RuntimeSupervisor,
}

impl RuntimeSessionLauncher {
    pub fn new(
        config: RuntimeConfig,
        store: RuntimeStore,
        model: Arc<dyn ModelClient>,
        supervisor: RuntimeSupervisor,
    ) -> Self {
        Self {
            config,
            store,
            model,
            supervisor,
        }
    }
}

impl RunSessionLauncher for RuntimeSessionLauncher {
    fn launch(&self, run_id: String) -> anyhow::Result<()> {
        let task_name = format!("session/{run_id}");
        let supervisor = self.supervisor.clone();
        let config = self.config.clone();
        let store = self.store.clone();
        let model = self.model.clone();
        supervisor.clone().spawn(task_name, false, async move {
            let tool_executor = if let Some(run) = store.get_run(&run_id).await {
                let workspace_root = effective_workspace_root(&config, &run.project_id);
                if config.sandbox_backend_mode == SandboxBackendMode::PhaseAContract {
                    // remote-fs-boundary: allow-begin phase-a-workspace-bootstrap
                    let _ = std::fs::create_dir_all(&workspace_root);
                    // remote-fs-boundary: allow-end phase-a-workspace-bootstrap
                }
                control_plane_executor_for_config(&config).with_workspace_root(workspace_root)
            } else {
                control_plane_executor_for_config(&config)
            };
            let session_store = store.clone();
            let session = QuerySession::with_tool_executor(store, model, tool_executor);
            if let Err(error) = session.submit_run(&run_id).await {
                if let Some(run) = session_store.get_run(&run_id).await {
                    if !run.status.is_terminal() && run.status != AgentRunStatus::NeedsUserInput {
                        session_store
                            .update_run_status(&run_id, AgentRunStatus::Failed)
                            .await
                            .ok();
                        let summary = format!("Agent session failed: {error}");
                        let _ = session_store
                            .append_event(AgentEvent::RunCompleted {
                                run_id: run_id.clone(),
                                status: "failed".to_string(),
                                summary: summary.clone(),
                                timestamp: Utc::now(),
                            })
                            .await;
                        session_store
                            .append_conversation_item(
                                &run.project_id,
                                Some(&run_id),
                                "run_completed",
                                Some("system"),
                                summary,
                                Some(serde_json::json!({
                                    "status": "failed",
                                    "reason": "session_execution_error",
                                })),
                            )
                            .await;
                    }
                }
                return Err(error);
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn effective_workspace_root(config: &RuntimeConfig, project_id: &str) -> std::path::PathBuf {
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
