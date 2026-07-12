use super::RuntimeSupervisor;
use crate::{
    artifact_publisher::FileArtifactPublisher,
    channel_manager::ChannelManager,
    config::SandboxBackendMode,
    http_api::{self, AppState},
    project::{audit_project_template_compatibility, ProjectInitWorkspaceTransaction},
    recovery::{recover_interrupted_runs, RecoveryOutcome},
    templates::BuiltInTemplateRegistry,
    tools::{
        runtime::ToolContext,
        sandbox::{LocalWorkspaceBackend, SandboxChannelWorkspaceBackend},
    },
    RuntimeConfig,
};
use chrono::Utc;
use std::time::Duration;
use tokio::net::TcpListener;

pub struct RecoveredRuntime {
    pub state: http_api::AppState,
    pub supervisor: RuntimeSupervisor,
}

pub struct RuntimeBootstrap {
    config: RuntimeConfig,
}

pub async fn recover_startup_runs(state: AppState) -> anyhow::Result<AppState> {
    ChannelManager::shared().reconcile(&state.store).await?;
    recover_project_init_transactions(&state).await?;
    audit_persisted_template_compatibility(&state).await?;
    state.store.reconcile_artifact_promotions().await?;
    garbage_collect_artifacts(&state).await?;
    let outcomes = recover_interrupted_runs(&state.store).await?;
    for outcome in outcomes {
        if let RecoveryOutcome::Resumed { run_id, .. } = outcome {
            http_api::spawn_supervised_session(state.clone(), run_id);
        }
    }
    Ok(state)
}

async fn audit_persisted_template_compatibility(state: &AppState) -> anyhow::Result<()> {
    let states = state.store.list_project_runtime_states().await?;
    let issues =
        audit_project_template_compatibility(&states, &BuiltInTemplateRegistry::built_in());
    if issues.is_empty() {
        return Ok(());
    }
    let summary = issues
        .iter()
        .map(|issue| {
            format!(
                "{} [{}]: {}",
                issue.project_id, issue.error_kind, issue.message
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!("persisted project template compatibility audit failed: {summary}")
}

async fn recover_project_init_transactions(state: &AppState) -> anyhow::Result<()> {
    for run in state.store.runs_requiring_recovery().await {
        let workspace_root = http_api::resolved_workspace_root(&state.config, &run.project_id);
        let mut ctx = ToolContext::new(state.store.clone(), run, workspace_root);
        ctx.remote_workspace = state.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes;
        ctx.runtime_storage_dir = state.config.runtime_storage_dir.clone();
        ctx.runtime_public_base_url = state.config.runtime_public_base_url.clone();
        match state.config.sandbox_backend_mode {
            SandboxBackendMode::PhaseAContract => {
                ProjectInitWorkspaceTransaction::recover_pending(&LocalWorkspaceBackend, &ctx)
                    .await?;
            }
            SandboxBackendMode::Kubernetes => {
                let backend = SandboxChannelWorkspaceBackend::from_runtime_config(&state.config)
                    .map_err(anyhow::Error::new)?;
                ProjectInitWorkspaceTransaction::recover_pending(&backend, &ctx).await?;
            }
        }
    }
    Ok(())
}

async fn garbage_collect_artifacts(state: &AppState) -> anyhow::Result<()> {
    let publisher = FileArtifactPublisher::new(&state.config.runtime_storage_dir);
    for publish in state
        .store
        .garbage_collectable_artifact_publishes(Utc::now())
        .await?
    {
        let is_current = state
            .store
            .current_project_version(&publish.project_id)
            .await
            .is_some_and(|version| version.id == publish.version_id);
        if is_current {
            continue;
        }
        publisher.garbage_collect(&publish)?;
        state
            .store
            .transition_artifact_publish(
                &publish.id,
                crate::types::ArtifactPublishStatus::GarbageCollected,
                None,
                None,
                None,
                None,
            )
            .await?;
    }
    Ok(())
}

impl RuntimeBootstrap {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    pub async fn recover(self) -> anyhow::Result<RecoveredRuntime> {
        self.config.validate_startup().map_err(anyhow::Error::msg)?;
        let supervisor = RuntimeSupervisor::new();
        let state = http_api::recover_startup_runs(http_api::app_state_with_supervisor(
            self.config,
            supervisor.clone(),
        ))
        .await?;
        supervisor.mark_recovered();
        Ok(RecoveredRuntime { state, supervisor })
    }

    pub async fn run(self) -> anyhow::Result<super::ShutdownEvidence> {
        self.recover().await?.serve().await
    }
}

impl RecoveredRuntime {
    pub async fn serve(self) -> anyhow::Result<super::ShutdownEvidence> {
        let public_listener = TcpListener::bind(self.state.config.bind_addr()).await?;
        let capture_listener =
            TcpListener::bind(self.state.config.runtime_browser_proxy_bind).await?;
        let public_app = http_api::router_with_state(self.state.clone());
        let capture_app = http_api::capture_router_with_state(self.state);

        self.supervisor
            .spawn_with_shutdown("server/public", true, |mut shutdown| async move {
                axum::serve(public_listener, public_app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown.changed().await;
                    })
                    .await
                    .map_err(anyhow::Error::new)
            })?;
        self.supervisor
            .spawn_with_shutdown("server/capture", true, |mut shutdown| async move {
                axum::serve(capture_listener, capture_app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown.changed().await;
                    })
                    .await
                    .map_err(anyhow::Error::new)
            })?;

        let fatal_failure = tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                None
            }
            failure = self.supervisor.wait_for_fatal_failure() => Some(failure),
        };
        let evidence = self.supervisor.shutdown(Duration::from_secs(10)).await;
        if let Some(failure) = fatal_failure {
            anyhow::bail!(failure);
        }
        Ok(evidence)
    }
}

pub struct TestRuntimeBuilder;

impl TestRuntimeBuilder {
    pub fn fresh(config: RuntimeConfig) -> RecoveredRuntime {
        let supervisor = RuntimeSupervisor::new();
        supervisor.mark_recovered();
        let state = http_api::app_state_with_supervisor(config, supervisor.clone());
        RecoveredRuntime { state, supervisor }
    }

    pub async fn recover(config: RuntimeConfig) -> anyhow::Result<RecoveredRuntime> {
        RuntimeBootstrap::new(config).recover().await
    }

    pub async fn recover_state(state: AppState) -> anyhow::Result<RecoveredRuntime> {
        let supervisor = state.supervisor.clone();
        let state = recover_startup_runs(state).await?;
        supervisor.mark_recovered();
        Ok(RecoveredRuntime { state, supervisor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_config(name: &str) -> RuntimeConfig {
        let root = std::env::temp_dir().join(format!(
            "runtime-bootstrap-{name}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut config = RuntimeConfig::from_env();
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        config.policy_profile = crate::config::RuntimePolicyProfile::LocalE2e;
        config.public_principal_auth_mode = crate::config::PublicPrincipalAuthMode::Disabled;
        config.runtime_storage_dir = root.join("runtime");
        config.workspace_root = root.join("workspaces");
        config
    }

    #[test]
    fn fresh_builder_is_explicitly_ready_without_recovery() {
        let runtime = TestRuntimeBuilder::fresh(test_config("fresh"));
        assert!(runtime.supervisor.readiness().is_ready());
        assert!(runtime.supervisor.readiness().active_tasks.is_empty());
    }

    #[tokio::test]
    async fn recovered_builder_completes_recovery_before_readiness() {
        let config = test_config("recovered");
        let root = config.runtime_storage_dir.parent().unwrap().to_path_buf();
        let runtime = TestRuntimeBuilder::recover(config).await.unwrap();
        assert!(runtime.supervisor.readiness().is_ready());
        assert!(runtime.state.supervisor.readiness().is_ready());
        let _ = fs::remove_dir_all(root);
    }
}
