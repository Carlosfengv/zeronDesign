use super::{adapters::*, ports::*};
use crate::{
    artifact_publisher::{safe_segment, ArtifactFile, ArtifactPublisher, FileArtifactPublisher},
    channel_manager::ChannelManager,
    config::{RuntimeConfig, RuntimePolicyProfile},
    conversation::RuntimeStore,
    permission::{
        check_command_policy, check_existing_path, check_lexical_workspace_path,
        check_workspace_path, PermissionError, PermissionReason, PermissionResult, RuleSource,
    },
    preview::{promote_preview_cas, validate_preview_promotion, PromotionGateReport},
    project::{
        check_project_write_path, ProjectInitWorkspaceTransaction, WorkspaceTransactionError,
    },
    templates::{
        BuiltInTemplateRegistry, ManifestHash, RenderPageRequest, SourceSnapshot, TemplateId,
        TemplateRegistry, TemplateSpec, TemplateVersion, TemplateWriteMode,
    },
    tools::{
        runtime::{ProgressSink, Tool, ToolContext, ToolError, ToolResult, ValidationError},
        schema::{object_schema, string_schema},
    },
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, ArtifactPublishStatus, ReviewFindingCategory,
        ReviewFindingEvidence, ReviewFindingSeverity,
    },
    workspace_auth::WorkspaceChannelJwtIssuer,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use scraper::{Html as ParsedHtml, Selector};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, net::TcpStream, process::Command as TokioCommand, time};

#[path = "browser.rs"]
mod browser;
#[path = "support/build.rs"]
mod build_support;
#[path = "fs/delete.rs"]
mod delete;
#[path = "diagnostics.rs"]
mod diagnostics;
#[path = "support/fs.rs"]
mod fs_support;
#[path = "support/package.rs"]
mod package_support;
#[path = "preview/fidelity.rs"]
mod preview_fidelity;
#[path = "preview/lifecycle.rs"]
mod preview_lifecycle;
#[path = "preview/publish.rs"]
mod preview_publish;
#[path = "support/preview.rs"]
mod preview_support;
#[path = "project/build.rs"]
mod project_build;
#[path = "project/initializer.rs"]
mod project_initializer;
#[path = "project/lifecycle.rs"]
mod project_lifecycle;
#[path = "support/project.rs"]
mod project_support;
#[path = "fs/read.rs"]
mod read;
#[path = "fs/search.rs"]
mod search;
#[path = "shell.rs"]
mod shell;
#[path = "style.rs"]
mod style;
#[path = "support/style.rs"]
mod style_support;
#[path = "fs/write.rs"]
mod write;

use build_support::*;
use fs_support::*;
use package_support::*;
use preview_support::*;
use project_support::*;
pub use project_support::{cleanup_staged_writes_for_run, cleanup_staged_writes_for_run_backend};
use style_support::*;

const MAX_DIRECT_WRITE_ARGUMENT_BYTES: usize = 96_000;
const MAX_DIRECT_WRITE_TEXT_CHARS: usize = 48_000;
const MAX_CHUNK_ARGUMENT_BYTES: usize = 48_000;
const MAX_CHUNK_TEXT_CHARS: usize = 24_000;
const MAX_CHUNKS_PER_WRITE: u64 = 512;
const STAGED_WRITE_TTL_SECS: i64 = 24 * 60 * 60;
const LARGE_WRITE_GUIDANCE: &str = "fs.write input is too large for direct tool-call JSON. Use fs.patch for existing files or fs.write_chunk/fs.commit_chunks for large new files. Do not retry the same full fs.write payload.";

fn truncate_for_metadata(text: &str) -> String {
    project_build::truncate_for_metadata(text)
}

async fn detect_static_preview_output_dir_backend(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
) -> Option<PathBuf> {
    preview_lifecycle::detect_static_preview_output_dir_backend(workspace, ctx, app_root).await
}

async fn collect_artifact_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    candidate_root: &Path,
) -> Result<Vec<ArtifactFile>, ToolError> {
    preview_publish::collect_artifact_files(workspace, ctx, candidate_root).await
}

pub fn sandbox_tools() -> Vec<Arc<dyn Tool>> {
    sandbox_tools_with_workspace_backend(Arc::new(LocalWorkspaceBackend))
}

pub fn sandbox_tools_with_workspace_backend(
    workspace_backend: Arc<dyn WorkspaceBackend>,
) -> Vec<Arc<dyn Tool>> {
    sandbox_tools_with_backends(workspace_backend, Arc::new(LocalCommandBackend))
}

pub fn sandbox_tools_with_backends(
    workspace_backend: Arc<dyn WorkspaceBackend>,
    command_backend: Arc<dyn SandboxCommandBackend>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        read::fs_read_tool(workspace_backend.clone()),
        read::design_source_read_sections_tool(),
        search::fs_list_tool(workspace_backend.clone()),
        search::fs_search_tool(workspace_backend.clone()),
        write::fs_write_tool(workspace_backend.clone()),
        write::fs_write_chunk_tool(workspace_backend.clone()),
        write::fs_commit_chunks_tool(workspace_backend.clone()),
        write::fs_patch_tool(workspace_backend.clone()),
        write::fs_multi_patch_tool(workspace_backend.clone()),
        style::style_update_tokens_tool(workspace_backend.clone()),
        delete::fs_delete_tool(workspace_backend.clone()),
        shell::shell_run_tool(command_backend.clone()),
        project_lifecycle::project_init_tool(workspace_backend.clone()),
        project_lifecycle::project_write_page_tool(workspace_backend.clone()),
        project_lifecycle::project_inspect_tool(workspace_backend.clone()),
        project_build::project_build_tool(workspace_backend.clone(), command_backend.clone()),
        project_build::project_ensure_dependencies_tool(
            workspace_backend.clone(),
            command_backend.clone(),
        ),
        project_build::package_install_tool(workspace_backend.clone(), command_backend.clone()),
        preview_publish::preview_rebuilding_tool(),
        preview_publish::preview_report_candidate_tool(workspace_backend.clone()),
        preview_publish::preview_publish_tool(workspace_backend.clone(), command_backend.clone()),
        preview_lifecycle::preview_start_tool(workspace_backend.clone(), command_backend.clone()),
        preview_lifecycle::preview_status_tool(workspace_backend.clone(), command_backend.clone()),
        preview_lifecycle::preview_stop_tool(workspace_backend.clone(), command_backend.clone()),
        diagnostics::diagnostics_build_log_tool(workspace_backend.clone()),
        diagnostics::diagnostics_typescript_tool(workspace_backend.clone()),
        browser::browser_open_tool(workspace_backend.clone()),
        browser::browser_screenshot_tool(workspace_backend.clone()),
        browser::browser_inspect_tool(workspace_backend),
    ]
}

pub async fn cancel_run_sandbox_resources(
    config: &RuntimeConfig,
    store: &RuntimeStore,
    run: &crate::types::AgentRun,
    workspace_root: PathBuf,
) -> anyhow::Result<usize> {
    let Some(binding_id) = run.sandbox_id.as_deref() else {
        return Ok(0);
    };
    let leases = store.active_preview_leases_for_binding(binding_id).await?;
    if leases.is_empty() {
        ChannelManager::shared()
            .release_binding(store, binding_id)
            .await?;
        return Ok(0);
    }

    let mut ctx = ToolContext::new(store.clone(), run.clone(), workspace_root);
    ctx.remote_workspace =
        config.sandbox_backend_mode == crate::config::SandboxBackendMode::Kubernetes;
    ctx.runtime_storage_dir = config.runtime_storage_dir.clone();
    ctx.runtime_public_base_url = config.runtime_public_base_url.clone();

    if ctx.remote_workspace {
        let key_file = config
            .workspace_channel_signing_key_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("workspace channel signing key is not configured"))?;
        let issuer = WorkspaceChannelJwtIssuer::from_pkcs8_der_file(
            key_file,
            config.workspace_channel_token_ttl_seconds,
        )?;
        let tls = WorkspaceChannelClientTls::from_runtime_config(config)?;
        let scheme = if tls.is_some() { "wss" } else { "ws" };
        let command = SandboxChannelCommandBackend::new()
            .with_tls(tls)
            .with_endpoint_resolver(Arc::new(
                SandboxBindingEndpointResolver::with_token_issuer(issuer)
                    .with_channel_scheme(scheme),
            ));
        for lease in &leases {
            command.stop_process(&ctx, &lease.id).await?;
            store.stop_preview_lease(&lease.id).await?;
        }
    } else {
        preview_lifecycle::stop_preview_pid(&ctx);
        for lease in &leases {
            store.stop_preview_lease(&lease.id).await?;
        }
    }
    ChannelManager::shared()
        .release_binding(store, binding_id)
        .await?;
    Ok(leases.len())
}
