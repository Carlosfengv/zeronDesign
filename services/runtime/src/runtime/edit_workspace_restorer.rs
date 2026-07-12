use crate::{
    artifact_publisher::{safe_segment, FileArtifactPublisher},
    config::{RuntimeConfig, SandboxBackendMode},
    conversation::RuntimeStore,
    run_lifecycle::EditWorkspaceRestorer,
    tools::{
        runtime::ToolContext,
        sandbox::{LocalWorkspaceBackend, SandboxChannelWorkspaceBackend, WorkspaceBackend},
    },
    types::AgentRun,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::path::{Path, PathBuf};

pub struct RuntimeEditWorkspaceRestorer;

#[async_trait]
impl EditWorkspaceRestorer for RuntimeEditWorkspaceRestorer {
    async fn restore(
        &self,
        store: &RuntimeStore,
        config: &RuntimeConfig,
        run: &AgentRun,
        source_snapshot_uri: &str,
    ) -> anyhow::Result<()> {
        let workspace_root = effective_workspace_root(config, &run.project_id);
        let project_root = workspace_root.join("project");
        let mut ctx = ToolContext::new(store.clone(), run.clone(), workspace_root.clone());
        ctx.remote_workspace = config.sandbox_backend_mode == SandboxBackendMode::Kubernetes;
        ctx.runtime_storage_dir = config.runtime_storage_dir.clone();
        let backend: Box<dyn WorkspaceBackend> = match config.sandbox_backend_mode {
            SandboxBackendMode::Kubernetes => Box::new(
                SandboxChannelWorkspaceBackend::from_runtime_config(config)
                    .map_err(|error| anyhow::anyhow!(error))?,
            ),
            SandboxBackendMode::PhaseAContract => Box::new(LocalWorkspaceBackend),
        };
        if let Err(error) = backend.remove_dir_all(&ctx, &project_root).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(anyhow::anyhow!(error));
            }
        }
        if let Some(runtime_path) = source_snapshot_uri.strip_prefix("runtime://source-snapshots/")
        {
            restore_runtime_snapshot(
                store,
                config,
                run,
                &ctx,
                backend.as_ref(),
                &project_root,
                runtime_path,
            )
            .await?;
        } else {
            let snapshot_root =
                workspace_file_uri_to_workspace_path(&workspace_root, source_snapshot_uri)?;
            backend
                .copy_dir_all(&ctx, &snapshot_root, &project_root, &[])
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        let dependency_state = serde_json::to_string_pretty(&json!({
            "needsRestore": true,
            "reason": "source_snapshot_restored_without_node_modules",
            "sourceSnapshotUri": source_snapshot_uri,
            "markedAt": Utc::now().to_rfc3339(),
        }))?;
        backend
            .write_string(
                &ctx,
                &workspace_root.join("state/dependency-state.json"),
                &dependency_state,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))
    }
}

async fn restore_runtime_snapshot(
    _store: &RuntimeStore,
    config: &RuntimeConfig,
    run: &AgentRun,
    ctx: &ToolContext,
    backend: &dyn WorkspaceBackend,
    project_root: &Path,
    runtime_path: &str,
) -> anyhow::Result<()> {
    let segments = runtime_path.split('/').collect::<Vec<_>>();
    if segments.len() != 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(anyhow::anyhow!("invalid Runtime source snapshot URI"));
    }
    if segments[0] != safe_segment(&run.project_id) {
        return Err(anyhow::anyhow!("source snapshot project mismatch"));
    }
    for file in FileArtifactPublisher::read_source_snapshot(
        &config.runtime_storage_dir,
        &run.project_id,
        segments[1],
    )? {
        let target = project_root.join(&file.path);
        backend
            .write_bytes(ctx, &target, &file.bytes)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let restored = backend
            .read_bytes(ctx, &target)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        if restored != file.bytes {
            return Err(anyhow::anyhow!(
                "source snapshot integrity check failed after restore: {}",
                file.path.display()
            ));
        }
    }
    Ok(())
}

fn workspace_file_uri_to_workspace_path(
    workspace_root: &Path,
    uri: &str,
) -> anyhow::Result<PathBuf> {
    let path = uri
        .strip_prefix("file:///workspace/")
        .ok_or_else(|| anyhow::anyhow!("unsupported source snapshot URI: {uri}"))?;
    let relative = Path::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!("source snapshot escapes workspace: {uri}"));
    }
    Ok(workspace_root.join(relative))
}

fn effective_workspace_root(config: &RuntimeConfig, project_id: &str) -> PathBuf {
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
