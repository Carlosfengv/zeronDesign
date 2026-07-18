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
use std::{
    io,
    path::{Path, PathBuf},
};

pub struct RuntimeEditWorkspaceRestorer;

#[async_trait]
impl EditWorkspaceRestorer for RuntimeEditWorkspaceRestorer {
    async fn prepare_build(
        &self,
        store: &RuntimeStore,
        config: &RuntimeConfig,
        run: &AgentRun,
    ) -> anyhow::Result<()> {
        let workspace_root = effective_workspace_root(config, &run.project_id);
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
        clear_workspace_root(backend.as_ref(), &ctx, &workspace_root).await?;
        store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "workspace.prepare_build",
                "scope=entire_workspace",
                "allow",
                "fresh Build workspace prepared after exclusive Sandbox acquisition",
            )
            .await;
        Ok(())
    }

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

async fn clear_workspace_root(
    backend: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    workspace_root: &Path,
) -> anyhow::Result<()> {
    let entries = match backend.list_dir(ctx, workspace_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(anyhow::anyhow!(error)),
    };
    for entry in entries {
        match entry.kind {
            crate::tools::sandbox::WorkspaceEntryKind::Dir => {
                backend
                    .remove_dir_all(ctx, &entry.path)
                    .await
                    .map_err(|error| anyhow::anyhow!(error))?;
            }
            crate::tools::sandbox::WorkspaceEntryKind::File => {
                backend
                    .remove_file(ctx, &entry.path)
                    .await
                    .map_err(|error| anyhow::anyhow!(error))?;
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentPhase;

    #[tokio::test]
    async fn prepare_build_removes_all_previous_project_workspace_entries() {
        let root = std::env::temp_dir().join(format!(
            "runtime-build-workspace-prepare-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut config = RuntimeConfig::from_env();
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        config.workspace_root = root.clone();
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "new-project".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let workspace_root = effective_workspace_root(&config, &run.project_id);
        std::fs::create_dir_all(workspace_root.join("project/out")).unwrap();
        std::fs::create_dir_all(workspace_root.join("state")).unwrap();
        std::fs::write(workspace_root.join("project/out/previous.html"), "secret").unwrap();
        std::fs::write(workspace_root.join("state/project.json"), "{}").unwrap();
        std::fs::write(workspace_root.join("previous-project.txt"), "secret").unwrap();

        RuntimeEditWorkspaceRestorer
            .prepare_build(&store, &config, &run)
            .await
            .unwrap();

        assert!(workspace_root.exists());
        assert_eq!(std::fs::read_dir(&workspace_root).unwrap().count(), 0);
        assert!(store
            .audit_records()
            .await
            .iter()
            .any(|record| { record.run_id == run.id && record.tool == "workspace.prepare_build" }));
        std::fs::remove_dir_all(root).unwrap();
    }
}
