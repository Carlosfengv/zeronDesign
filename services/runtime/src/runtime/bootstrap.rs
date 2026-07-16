use super::RuntimeSupervisor;
use crate::{
    artifact_publisher::FileArtifactPublisher,
    channel_manager::ChannelManager,
    config::{SandboxBackendMode, WorkRuntimeBackendMode},
    http_api::{self, AppState},
    project::{
        audit_project_template_compatibility, ProjectInitWorkspaceTransaction,
        WorkspaceTransactionError,
    },
    publication::{
        ControlPlaneOnlyBackend, KubernetesWorkRuntimeBackend, WorkRuntimeBackend,
        WorkRuntimeController,
    },
    recovery::{recover_interrupted_runs, RecoveryOutcome},
    release::{
        ProcessReleasePackagingBackend, ReleasePackagingController, TrustedReleasePackagingBackend,
    },
    run_lifecycle::RunSessionLauncher,
    runtime::RuntimeSessionLauncher,
    templates::BuiltInTemplateRegistry,
    tools::{
        runtime::ToolContext,
        sandbox::{LocalWorkspaceBackend, SandboxChannelWorkspaceBackend, WorkspaceBackend},
    },
    types::{AgentEvent, AgentRun, AgentRunStatus},
    RuntimeConfig,
};
use chrono::Utc;
use serde_json::json;
use std::{collections::BTreeMap, env, sync::Arc, time::Duration};
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
    state
        .store
        .publication_store()
        .replay_nonterminal_outbox()?;
    if let Some(backend) = release_packaging_backend(&state.config)? {
        ReleasePackagingController::new(
            state.store.release_store(),
            backend,
            state.config.runtime_storage_dir.clone(),
            Duration::from_secs(2),
        )
        .spawn(&state.supervisor)?;
    }
    let work_runtime_backend: Arc<dyn WorkRuntimeBackend> =
        match state.config.work_runtime_backend_mode {
            WorkRuntimeBackendMode::ControlPlaneOnly => Arc::new(ControlPlaneOnlyBackend),
            WorkRuntimeBackendMode::Kubernetes => {
                Arc::new(KubernetesWorkRuntimeBackend::from_runtime_config(&state.config).await?)
            }
        };
    WorkRuntimeController::new(
        state.store.publication_store(),
        state.store.release_store(),
        work_runtime_backend,
        Duration::from_secs(5),
    )
    .spawn(&state.supervisor)?;
    let outcomes = recover_interrupted_runs(&state.store).await?;
    for outcome in outcomes {
        if let RecoveryOutcome::Resumed { run_id, .. } = outcome {
            RuntimeSessionLauncher::new(
                state.config.clone(),
                state.store.clone(),
                state.model.clone(),
                state.supervisor.clone(),
            )
            .launch(run_id)?;
        }
    }
    Ok(state)
}

fn release_packaging_backend(
    config: &crate::RuntimeConfig,
) -> anyhow::Result<Option<Arc<dyn TrustedReleasePackagingBackend>>> {
    let (Some(program), Some(expected_sha256)) = (
        config.release_packaging_helper_path.clone(),
        config.release_packaging_helper_sha256.clone(),
    ) else {
        return Ok(None);
    };
    let mut environment = BTreeMap::new();
    for name in ["PATH", "HOME", "DOCKER_HOST", "TMPDIR", "XDG_CONFIG_HOME"] {
        if let Ok(value) = env::var(name) {
            environment.insert(name.to_string(), value);
        }
    }
    let packager_root = config
        .release_packager_root
        .clone()
        .unwrap_or_else(|| config.runtime_storage_dir.join("release-packager"));
    environment.insert(
        "ANYDESIGN_PACKAGER_ROOT".to_string(),
        packager_root.display().to_string(),
    );
    if let Some(tools) = config.release_packager_tools.clone() {
        environment.insert("ANYDESIGN_PACKAGER_TOOLS".to_string(), tools);
    }
    let backend = ProcessReleasePackagingBackend::new(
        program,
        expected_sha256,
        environment,
        Duration::from_secs(config.release_packaging_deadline_seconds),
    )?;
    Ok(Some(Arc::new(backend)))
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
    match state.config.sandbox_backend_mode {
        SandboxBackendMode::PhaseAContract => {
            recover_project_init_transactions_with_backend(state, &LocalWorkspaceBackend, false)
                .await
        }
        SandboxBackendMode::Kubernetes => {
            let backend = SandboxChannelWorkspaceBackend::from_runtime_config(&state.config)
                .map_err(anyhow::Error::new)?;
            recover_project_init_transactions_with_backend(state, &backend, true).await
        }
    }
}

async fn recover_project_init_transactions_with_backend(
    state: &AppState,
    backend: &dyn WorkspaceBackend,
    remote_workspace: bool,
) -> anyhow::Result<()> {
    for run in state.store.runs_requiring_recovery().await {
        let workspace_root = http_api::resolved_workspace_root(&state.config, &run.project_id);
        let mut ctx = ToolContext::new(state.store.clone(), run.clone(), workspace_root);
        ctx.remote_workspace = remote_workspace;
        ctx.runtime_storage_dir = state.config.runtime_storage_dir.clone();
        ctx.runtime_public_base_url = state.config.runtime_public_base_url.clone();
        if let Err(error) = ProjectInitWorkspaceTransaction::recover_pending(backend, &ctx).await {
            isolate_project_init_recovery_failure(state, &run, &error).await?;
        }
    }
    Ok(())
}

async fn isolate_project_init_recovery_failure(
    state: &AppState,
    run: &AgentRun,
    error: &WorkspaceTransactionError,
) -> anyhow::Result<()> {
    let reason = format!(
        "Runtime startup isolated project.init recovery failure [{}]: {}",
        error.error_kind, error.message
    );
    state
        .store
        .append_audit_record(
            &run.project_id,
            &run.id,
            "project.init.startup_recovery",
            format!(
                "sandboxBindingId={}",
                run.sandbox_id.as_deref().unwrap_or("none")
            ),
            "recovery_required",
            reason.clone(),
        )
        .await;
    state
        .store
        .update_run_status(&run.id, AgentRunStatus::Failed)
        .await?;
    state
        .store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "failed".to_string(),
            summary: reason.clone(),
            timestamp: Utc::now(),
        })
        .await?;
    state
        .store
        .append_conversation_item(
            &run.project_id,
            Some(&run.id),
            "error_summary",
            Some("system"),
            &reason,
            Some(json!({
                "recoverable": true,
                "errorKind": error.error_kind,
                "checkpointId": run.checkpoint_id,
                "sandboxBindingId": run.sandbox_id,
                "journalPreserved": true,
                "suggestedAction": "Restore or replace the sandbox workspace binding, then inspect the preserved project.init journal before retrying."
            })),
        )
        .await;
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
    use async_trait::async_trait;
    use std::{fs, io, path::Path};

    struct SelectiveUnavailableWorkspace;

    #[async_trait]
    impl WorkspaceBackend for SelectiveUnavailableWorkspace {
        async fn read_to_string(&self, ctx: &ToolContext, _path: &Path) -> io::Result<String> {
            if ctx.project_id == "stale-project-init" {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "failed to lookup address information: Name or service not known",
                ));
            }
            Err(io::Error::new(io::ErrorKind::NotFound, "no journal"))
        }

        async fn write_string(
            &self,
            _ctx: &ToolContext,
            _path: &Path,
            _text: &str,
        ) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }

        async fn list_dir(
            &self,
            _ctx: &ToolContext,
            _path: &Path,
        ) -> io::Result<Vec<crate::tools::sandbox::WorkspaceEntry>> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }

        async fn path_kind(
            &self,
            _ctx: &ToolContext,
            _path: &Path,
        ) -> io::Result<crate::tools::sandbox::WorkspacePathKind> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }

        async fn remove_file(&self, _ctx: &ToolContext, _path: &Path) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }

        async fn remove_dir_all(&self, _ctx: &ToolContext, _path: &Path) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }

        async fn copy_dir_all(
            &self,
            _ctx: &ToolContext,
            _from: &Path,
            _to: &Path,
            _skip_dir_names: &[String],
        ) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unused"))
        }
    }

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
        assert!(runtime
            .supervisor
            .readiness()
            .active_tasks
            .contains(&"controller/work-runtime".to_string()));
        runtime.supervisor.shutdown(Duration::from_secs(1)).await;
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn startup_isolates_unreachable_project_init_journal_and_continues_other_runs() {
        let config = test_config("stale-project-init");
        let root = config.runtime_storage_dir.parent().unwrap().to_path_buf();
        let store = crate::RuntimeStore::with_checkpoint_dir(&config.runtime_storage_dir);
        let stale = store
            .create_run(
                "stale-project-init".to_string(),
                crate::types::AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let healthy = store
            .create_run(
                "healthy-project-init".to_string(),
                crate::types::AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let state = AppState {
            supervisor: RuntimeSupervisor::new(),
            store: store.clone(),
            model: Arc::new(crate::model_gateway::EmptyModelClient),
            config,
        };

        recover_project_init_transactions_with_backend(
            &state,
            &SelectiveUnavailableWorkspace,
            true,
        )
        .await
        .unwrap();

        assert_eq!(
            store.get_run(&stale.id).await.unwrap().status,
            AgentRunStatus::Failed
        );
        assert_eq!(
            store.get_run(&healthy.id).await.unwrap().status,
            AgentRunStatus::Queued
        );
        assert_eq!(
            store
                .runs_requiring_recovery()
                .await
                .into_iter()
                .map(|run| run.id)
                .collect::<Vec<_>>(),
            vec![healthy.id.clone()]
        );
        let audit = store
            .audit_records()
            .await
            .into_iter()
            .find(|record| record.run_id == stale.id)
            .expect("startup recovery audit must be preserved");
        assert_eq!(audit.tool, "project.init.startup_recovery");
        assert_eq!(audit.decision, "recovery_required");
        assert!(audit
            .reason
            .contains("project.init_recovery_workspace_unavailable"));
        let completion = store
            .events(&stale.id)
            .await
            .into_iter()
            .find(|event| event.is_run_completed())
            .expect("failed recovery must emit a terminal event");
        assert!(format!("{completion:?}").contains("project.init_recovery_workspace_unavailable"));
        let summary = store
            .conversation_items(&stale.project_id)
            .await
            .into_iter()
            .find(|item| item.run_id.as_deref() == Some(stale.id.as_str()))
            .expect("failed recovery must preserve an operator summary");
        assert_eq!(summary.metadata.as_ref().unwrap()["journalPreserved"], true);
        assert_eq!(
            summary.metadata.as_ref().unwrap()["errorKind"],
            "project.init_recovery_workspace_unavailable"
        );

        let _ = fs::remove_dir_all(root);
    }
}
