use super::*;

pub async fn recover_startup_runs(state: AppState) -> anyhow::Result<AppState> {
    ChannelManager::shared().reconcile(&state.store).await?;
    recover_project_init_transactions(&state).await?;
    audit_persisted_template_compatibility(&state).await?;
    state.store.reconcile_artifact_promotions().await?;
    garbage_collect_artifacts(&state).await?;
    let outcomes = recover_interrupted_runs(&state.store).await?;
    for outcome in outcomes {
        if let RecoveryOutcome::Resumed { run_id, .. } = outcome {
            spawn_session(state.clone(), run_id);
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
        let workspace_root = effective_workspace_root(&state.config, &run.project_id);
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
