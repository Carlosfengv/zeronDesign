use super::*;
use crate::{
    draft_preview::StartDraftPreview,
    types::{canonical_json_hash, PreviewLeaseMode},
    visual_contracts::{DraftPreviewSessionStatus, RUNTIME_DEPENDENCY_POLICY_VERSION},
};

const DEV_PORT: u16 = 3000;
const DEV_STATE_PATH: &str = "state/dev-preview.json";

pub(super) fn preview_dev_start_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewDevStartTool { workspace, command })
}

pub(super) fn preview_dev_status_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewDevStatusTool { workspace, command })
}

pub(super) fn preview_dev_stop_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewDevStopTool { workspace, command })
}

struct PreviewDevStartTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewDevStartTool {
    fn name(&self) -> &'static str {
        "preview.dev_start"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "next-app development preview allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        ensure_dev_preview_supported(&ctx)?;
        if !ctx.remote_workspace {
            return Err(typed_recoverable(
                "preview.dev_start requires a managed sandbox process lease",
                "preview.dev_unavailable",
                json!({
                    "fallback": "Run project.build and preview.start.",
                    "blocking": false
                }),
            ));
        }
        if let Some(active) = ctx
            .store
            .draft_preview_store()
            .active_for_project(&ctx.project_id)
        {
            return Ok(ToolResult::ok(json!({
                "status": active.status,
                "session": active,
                "reused": true
            })));
        }

        let based_on_snapshot_id = ctx
            .store
            .list_project_draft_snapshots(&ctx.project_id)
            .await
            .into_iter()
            .max_by_key(|snapshot| snapshot.created_at)
            .map(|snapshot| snapshot.snapshot_id);
        let base_snapshot =
            freeze_draft_revision(&*self.workspace, &ctx, based_on_snapshot_id).await?;
        let source_hash = base_snapshot.source_hash.clone();
        let lease = ctx
            .store
            .create_preview_lease_with_mode(
                &ctx.run.id,
                format!("dev-{}", ctx.run.id),
                source_hash,
                PreviewLeaseMode::Dev,
                DEV_PORT,
                900,
            )
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let proxy_url = format!("{}/previews/{}/", ctx.runtime_public_base_url, lease.id);
        let binding_id = ctx.run.sandbox_id.clone().ok_or_else(|| {
            ToolError::Terminal("remote development preview has no sandbox binding".to_string())
        })?;
        let project_state = ctx.run.project_state_snapshot.as_ref().unwrap();
        let session = match ctx.store.draft_preview_store().start(StartDraftPreview {
            project_id: ctx.project_id.clone(),
            sandbox_binding_id: binding_id,
            template_id: project_state.template_key.clone(),
            base_snapshot_id: base_snapshot.snapshot_id.clone(),
            base_version_id: ctx.run.base_version_id.clone(),
            proxy_url: proxy_url.clone(),
            writer_ttl_seconds: 120,
        }) {
            Ok(session) => session,
            Err(error) => {
                ctx.store.stop_preview_lease(&lease.id).await.ok();
                return Err(typed_recoverable(
                    error.to_string(),
                    "preview.writer_conflict",
                    json!({ "blocking": false }),
                ));
            }
        };
        let cwd = default_project_dir(&ctx);
        let process = match self
            .command
            .start_process(&ctx, &lease.id, &dev_argv(&lease.id), &cwd)
            .await
        {
            Ok(process) => process,
            Err(error) => {
                ctx.store
                    .draft_preview_store()
                    .stop(
                        &session.session_id,
                        format!("development process failed to start: {error}"),
                    )
                    .ok();
                ctx.store.stop_preview_lease(&lease.id).await.ok();
                return Err(typed_recoverable(
                    format!("development preview failed to start: {error}"),
                    "preview.dev_process_failed",
                    json!({ "blocking": false }),
                ));
            }
        };
        let session = if process_looks_ready(&process) {
            ctx.store
                .draft_preview_store()
                .mark_ready(&session.session_id, session.session_epoch, 0)
                .unwrap_or(session)
        } else {
            session
        };
        let state = dev_state_json(&lease.id, &session, &process, &proxy_url, &cwd, &ctx);
        write_workspace_json(&*self.workspace, &ctx, DEV_STATE_PATH, &state).await?;
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

struct PreviewDevStatusTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewDevStatusTool {
    fn name(&self) -> &'static str {
        "preview.dev_status"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "development preview status allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let mut state = read_workspace_json(&*self.workspace, &ctx, DEV_STATE_PATH)
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "no development preview session exists",
                    "preview.dev_not_started",
                    json!({ "blocking": false }),
                )
            })?;
        let lease_id = required_state_string(&state, "leaseId")?;
        let session_id = required_state_string(&state, "sessionId")?;
        let mut session = ctx
            .store
            .draft_preview_store()
            .get(&session_id)
            .ok_or_else(|| ToolError::Terminal("DraftPreviewSession is missing".to_string()))?;
        let mut process = self
            .command
            .process_status(&ctx, &lease_id)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;

        if process.status == "running" {
            if let Some(error) = compile_error(&process) {
                session = ctx
                    .store
                    .draft_preview_store()
                    .mark_compile_error(&session_id, session.session_epoch, error)
                    .unwrap_or(session);
            } else if session.status != DraftPreviewSessionStatus::Ready
                && process_looks_ready(&process)
            {
                session = ctx
                    .store
                    .draft_preview_store()
                    .mark_ready(
                        &session_id,
                        session.session_epoch,
                        session.workspace_revision,
                    )
                    .unwrap_or(session);
            }
        } else if !matches!(
            session.status,
            DraftPreviewSessionStatus::Failed | DraftPreviewSessionStatus::Stopped
        ) {
            session = ctx
                .store
                .draft_preview_store()
                .begin_restart(
                    &session_id,
                    format!("development process exited with status {}", process.status),
                )
                .map_err(|error| ToolError::Recoverable(error.to_string()))?;
            if session.status == DraftPreviewSessionStatus::Restarting {
                process = self
                    .command
                    .start_process(
                        &ctx,
                        &lease_id,
                        &dev_argv(&lease_id),
                        &default_project_dir(&ctx),
                    )
                    .await
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?;
            }
        }

        state["status"] = json!(session.status);
        state["session"] = json!(session);
        state["sessionEpoch"] = json!(session.session_epoch);
        state["workspaceRevision"] = json!(session.workspace_revision);
        state["lastReadyRevision"] = json!(session.last_ready_revision);
        state["durableRevision"] = json!(session.durable_revision);
        state["processStatus"] = json!(process.status);
        state["pid"] = json!(process.pid);
        state["accessible"] = json!(process.status == "running");
        state["stdout"] = json!(truncate_for_metadata(&process.stdout));
        state["stderr"] = json!(truncate_for_metadata(&process.stderr));
        write_workspace_json(&*self.workspace, &ctx, DEV_STATE_PATH, &state).await?;
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

struct PreviewDevStopTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewDevStopTool {
    fn name(&self) -> &'static str {
        "preview.dev_stop"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "development preview stop allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let mut state = read_workspace_json(&*self.workspace, &ctx, DEV_STATE_PATH)
            .await
            .unwrap_or_else(|| json!({ "status": "stopped", "accessible": false }));
        if let Some(lease_id) = state.get("leaseId").and_then(Value::as_str) {
            self.command.stop_process(&ctx, lease_id).await.ok();
            ctx.store.stop_preview_lease(lease_id).await.ok();
        }
        if let Some(session_id) = state.get("sessionId").and_then(Value::as_str) {
            ctx.store
                .draft_preview_store()
                .stop(session_id, "requested by preview.dev_stop".to_string())
                .ok();
        }
        state["status"] = json!("stopped");
        state["accessible"] = json!(false);
        state["pid"] = Value::Null;
        write_workspace_json(&*self.workspace, &ctx, DEV_STATE_PATH, &state).await?;
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

fn ensure_dev_preview_supported(ctx: &ToolContext) -> Result<(), ToolError> {
    let state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
        typed_recoverable(
            "preview.dev_start requires initialized project state",
            "project.state_missing",
            json!({ "blocking": false }),
        )
    })?;
    if state.template_key != "next-app" {
        return Err(typed_recoverable(
            "development HMR preview is currently available for next-app only",
            "template.operation_unsupported",
            json!({ "template": state.template_key, "blocking": false }),
        ));
    }
    Ok(())
}

fn dev_argv(lease_id: &str) -> Vec<String> {
    vec![
        "env".to_string(),
        format!("ANYDESIGN_PREVIEW_BASE_PATH=/previews/{lease_id}"),
        "npm".to_string(),
        "run".to_string(),
        "dev".to_string(),
        "--".to_string(),
        "--port".to_string(),
        DEV_PORT.to_string(),
    ]
}

fn process_looks_ready(process: &SandboxProcessLease) -> bool {
    process.status == "running"
        && format!("{}\n{}", process.stdout, process.stderr)
            .to_ascii_lowercase()
            .contains("ready")
}

fn compile_error(process: &SandboxProcessLease) -> Option<String> {
    let output = format!("{}\n{}", process.stdout, process.stderr);
    let lowered = output.to_ascii_lowercase();
    [
        "failed to compile",
        "syntaxerror",
        "type error",
        "module not found",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    .then(|| truncate_for_metadata(&output))
}

fn required_state_string(state: &Value, key: &str) -> Result<String, ToolError> {
    state
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ToolError::Terminal(format!("development preview state omitted {key}")))
}

fn dev_state_json(
    lease_id: &str,
    session: &crate::visual_contracts::DraftPreviewSession,
    process: &SandboxProcessLease,
    proxy_url: &str,
    cwd: &Path,
    ctx: &ToolContext,
) -> Value {
    json!({
        "status": session.status,
        "url": proxy_url,
        "port": DEV_PORT,
        "command": "npm run dev",
        "mode": "dev",
        "cwd": display_workspace_path(cwd, ctx),
        "leaseId": lease_id,
        "sessionId": session.session_id,
        "sessionEpoch": session.session_epoch,
        "writerLeaseId": session.writer_lease_id,
        "writerLeaseExpiresAt": session.writer_lease_expires_at,
        "workspaceRevision": session.workspace_revision,
        "lastReadyRevision": session.last_ready_revision,
        "durableRevision": session.durable_revision,
        "durableSnapshotId": session.durable_snapshot_id,
        "pid": process.pid,
        "processStatus": process.status,
        "accessible": process.status == "running",
        "managed": true,
        "hmr": true,
        "session": session,
    })
}

pub(super) async fn freeze_draft_revision(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    based_on_snapshot_id: Option<String>,
) -> Result<crate::visual_contracts::DraftSnapshot, ToolError> {
    let project_state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
        ToolError::Terminal("cannot freeze Draft revision without project state".to_string())
    })?;
    let cwd = default_project_dir(ctx);
    let workspace_source_hash = project_source_fingerprint(workspace, ctx, &cwd).await?;
    let snapshot_key = format!("draft-{}-{}", ctx.run.id, &workspace_source_hash[..16]);
    let relative = format!("outputs/draft/source-snapshots/{snapshot_key}");
    snapshot_project_source(workspace, ctx, &cwd, &relative).await?;
    let files = collect_artifact_files(workspace, ctx, &ctx.workspace_root.join(&relative)).await?;
    let source_hash = frozen_source_fingerprint(&files)?;
    let source_snapshot_uri = FileArtifactPublisher::new(&ctx.runtime_storage_dir)
        .publish_source_snapshot(&ctx.project_id, &snapshot_key, files)
        .await
        .map_err(|error| ToolError::Terminal(error.to_string()))?;
    let design_context_hash = ctx
        .run
        .design_context_content_hash
        .clone()
        .unwrap_or_else(|| canonical_json_hash(&json!({ "designContext": "none" })));
    ctx.store
        .create_draft_revision_snapshot(
            &ctx.project_id,
            source_snapshot_uri,
            source_hash,
            project_state.template_key.clone(),
            project_state.template_version.clone(),
            RUNTIME_DEPENDENCY_POLICY_VERSION.to_string(),
            design_context_hash,
            &ctx.run.id,
            based_on_snapshot_id,
            None,
        )
        .await
        .map_err(|error| ToolError::Terminal(error.to_string()))
}

pub(super) async fn record_dev_mutation(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
) -> Option<Value> {
    let store = ctx.store.draft_preview_store();
    let session = match ctx.run.edit_base.as_ref() {
        Some(crate::visual_contracts::EditBase::Draft { session_id, .. }) => {
            store.get(session_id)?
        }
        _ => store.active_for_project(&ctx.project_id)?,
    };
    let old_revision = session.workspace_revision;
    let committed = match store.commit_revision(
        &session.session_id,
        &session.writer_lease_id,
        session.session_epoch,
        session.workspace_revision,
    ) {
        Ok(session) => session,
        Err(error) => {
            return Some(json!({
                "status": "revision_commit_failed",
                "blocking": false,
                "error": error.to_string()
            }));
        }
    };
    let snapshot =
        match freeze_draft_revision(workspace, ctx, Some(committed.durable_snapshot_id.clone()))
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Some(json!({
                    "status": "durability_pending",
                    "blocking": false,
                    "sessionId": committed.session_id,
                    "sessionEpoch": committed.session_epoch,
                    "workspaceRevision": committed.workspace_revision,
                    "durableRevision": committed.durable_revision,
                    "error": format!("{error:?}")
                }));
            }
        };
    match store.mark_durable(
        &committed.session_id,
        &committed.writer_lease_id,
        committed.session_epoch,
        committed.workspace_revision,
        snapshot.snapshot_id.clone(),
        snapshot.source_hash.clone(),
    ) {
        Ok(session) => {
            if ctx.run.edit_base.is_some() {
                if let Err(error) = ctx
                    .store
                    .advance_run_draft_edit_base(
                        &ctx.run.id,
                        session.session_epoch,
                        old_revision,
                        session.durable_snapshot_id.clone(),
                        session.workspace_revision,
                    )
                    .await
                {
                    return Some(json!({
                        "status": "edit_base_advance_failed",
                        "blocking": false,
                        "sessionId": session.session_id,
                        "workspaceRevision": session.workspace_revision,
                        "durableRevision": session.durable_revision,
                        "error": error.to_string()
                    }));
                }
            }
            Some(json!({
                "status": "durable",
                "blocking": false,
                "sessionId": session.session_id,
                "sessionEpoch": session.session_epoch,
                "workspaceRevision": session.workspace_revision,
                "durableRevision": session.durable_revision,
                "durableSnapshotId": session.durable_snapshot_id,
                "sourceHash": snapshot.source_hash
            }))
        }
        Err(error) => Some(json!({
            "status": "durability_pending",
            "blocking": false,
            "sessionId": committed.session_id,
            "workspaceRevision": committed.workspace_revision,
            "error": error.to_string()
        })),
    }
}

pub(super) async fn record_dev_file_mutation(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Option<Value> {
    if !dev_file_mutation_targets_project(ctx, path) {
        return None;
    }
    record_dev_mutation(workspace, ctx).await
}

pub(super) fn validate_dev_file_mutation(ctx: &ToolContext, path: &Path) -> Result<(), ToolError> {
    if !dev_file_mutation_targets_project(ctx, path) {
        return Ok(());
    }
    validate_dev_mutation(ctx)
}

fn dev_file_mutation_targets_project(ctx: &ToolContext, path: &Path) -> bool {
    let app_root = default_project_dir(ctx);
    path == app_root || path.starts_with(app_root)
}

pub(super) fn validate_dev_mutation(ctx: &ToolContext) -> Result<(), ToolError> {
    let draft_store = ctx.store.draft_preview_store();
    match ctx.run.edit_base.as_ref() {
        Some(crate::visual_contracts::EditBase::Draft {
            snapshot_id,
            session_id,
            expected_session_epoch,
            expected_workspace_revision,
            writer_lease_id,
        }) => {
            let session = draft_store.get(session_id).ok_or_else(|| {
                typed_recoverable(
                    "Draft EditBase session no longer exists",
                    "edit.base_stale",
                    json!({ "blocking": true }),
                )
            })?;
            if session.project_id != ctx.project_id
                || session.session_epoch != *expected_session_epoch
                || session.workspace_revision != *expected_workspace_revision
                || session.durable_snapshot_id != *snapshot_id
                || session.writer_lease_id != *writer_lease_id
                || session.writer_lease_expires_at <= Utc::now()
            {
                return Err(typed_recoverable(
                    "Draft EditBase changed before the file transaction",
                    "edit.base_stale",
                    json!({
                        "blocking": true,
                        "latestSessionEpoch": session.session_epoch,
                        "latestWorkspaceRevision": session.workspace_revision,
                        "latestSnapshotId": session.durable_snapshot_id,
                    }),
                ));
            }
        }
        None if ctx.run.phase == crate::types::AgentPhase::Edit
            && draft_store.active_for_project(&ctx.project_id).is_some() =>
        {
            return Err(typed_recoverable(
                "Draft Edit run is missing its frozen EditBase",
                "edit.base_stale",
                json!({ "blocking": true }),
            ));
        }
        _ => {}
    }
    Ok(())
}
