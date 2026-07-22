use super::*;
use crate::{types::canonical_json_hash, visual_contracts::RUNTIME_DEPENDENCY_POLICY_VERSION};

pub(super) fn draft_snapshot_create_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(DraftSnapshotCreateTool { workspace })
}

pub(super) fn draft_restore_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(DraftRestoreTool { workspace, command })
}

struct DraftSnapshotCreateTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

struct DraftRestoreTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for DraftRestoreTool {
    fn name(&self) -> &'static str {
        "draft.restore"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "kind": string_schema("History item kind: draft_snapshot or work_version"),
                "itemId": string_schema("DraftSnapshot or WorkVersion identifier")
            }),
            &["kind", "itemId"],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "explicit Draft history restore allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let project_state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
            ToolError::Terminal("draft.restore requires project state".to_string())
        })?;
        if project_state.template_key != "next-app" {
            return Err(typed_recoverable(
                "draft.restore is currently available for next-app",
                "template.operation_unsupported",
                json!({ "blocking": false }),
            ));
        }
        let kind = required_str(&input, "kind")?;
        let item_id = required_str(&input, "itemId")?;
        let (source_uri, source_hash, based_on_snapshot_id, restored_from_version_id) = match kind {
            "draft_snapshot" => {
                let snapshot = ctx.store.get_draft_snapshot(item_id).await.ok_or_else(|| {
                    typed_recoverable(
                        format!("DraftSnapshot not found: {item_id}"),
                        "draft.restore_not_found",
                        json!({}),
                    )
                })?;
                if snapshot.project_id != ctx.project_id {
                    return Err(typed_recoverable(
                        format!("DraftSnapshot not found: {item_id}"),
                        "draft.restore_not_found",
                        json!({}),
                    ));
                }
                (
                    snapshot.source_snapshot_uri,
                    snapshot.source_hash,
                    Some(snapshot.snapshot_id),
                    None,
                )
            }
            "work_version" => {
                let version = ctx
                    .store
                    .get_project_version(item_id)
                    .await
                    .ok_or_else(|| {
                        typed_recoverable(
                            format!("WorkVersion not found: {item_id}"),
                            "draft.restore_not_found",
                            json!({}),
                        )
                    })?;
                if version.project_id != ctx.project_id {
                    return Err(typed_recoverable(
                        format!("WorkVersion not found: {item_id}"),
                        "draft.restore_not_found",
                        json!({}),
                    ));
                }
                let source_uri = version.source_snapshot_uri.ok_or_else(|| {
                    typed_recoverable(
                        "selected WorkVersion has no recoverable source snapshot",
                        "draft.restore_source_missing",
                        json!({}),
                    )
                })?;
                let source_hash = ctx
                    .store
                    .list_project_draft_snapshots(&ctx.project_id)
                    .await
                    .into_iter()
                    .find(|snapshot| snapshot.source_snapshot_uri == source_uri)
                    .map(|snapshot| snapshot.source_hash)
                    .unwrap_or_else(String::new);
                (source_uri, source_hash, None, Some(version.id))
            }
            _ => {
                return Err(typed_recoverable(
                    "draft.restore kind must be draft_snapshot or work_version",
                    "tool.input_schema_invalid",
                    json!({}),
                ));
            }
        };

        if let Some(preview) =
            read_workspace_json(&*self.workspace, &ctx, "state/dev-preview.json").await
        {
            if let Some(lease_id) = preview.get("leaseId").and_then(Value::as_str) {
                self.command.stop_process(&ctx, lease_id).await.ok();
                ctx.store.stop_preview_lease(lease_id).await.ok();
            }
            if let Some(session_id) = preview.get("sessionId").and_then(Value::as_str) {
                ctx.store
                    .draft_preview_store()
                    .stop(session_id, "superseded by draft.restore".to_string())
                    .ok();
            }
        }

        let runtime_path = source_uri
            .strip_prefix("runtime://source-snapshots/")
            .ok_or_else(|| {
                typed_recoverable(
                    "draft.restore only accepts Runtime-owned source snapshots",
                    "draft.restore_source_invalid",
                    json!({}),
                )
            })?;
        let segments = runtime_path.split('/').collect::<Vec<_>>();
        if segments.len() != 2 || segments[0] != safe_segment(&ctx.project_id) {
            return Err(typed_recoverable(
                "Draft restore source identity does not match the project",
                "draft.restore_source_invalid",
                json!({}),
            ));
        }
        let files = FileArtifactPublisher::read_source_snapshot(
            &ctx.runtime_storage_dir,
            &ctx.project_id,
            segments[1],
        )
        .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let project_root = default_project_dir(&ctx);
        match self.workspace.remove_dir_all(&ctx, &project_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ToolError::Recoverable(error.to_string())),
        }
        for file in &files {
            self.workspace
                .write_bytes(&ctx, &project_root.join(&file.path), &file.bytes)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        }
        let actual_hash = restored_source_hash(&files);
        if !source_hash.is_empty() && actual_hash != source_hash {
            return Err(ToolError::Terminal(
                "restored Draft source hash does not match history identity".to_string(),
            ));
        }
        let design_context_hash = ctx
            .run
            .design_context_content_hash
            .clone()
            .unwrap_or_else(|| canonical_json_hash(&json!({ "designContext": "none" })));
        let restored = ctx
            .store
            .create_restored_draft_snapshot(
                &ctx.project_id,
                source_uri,
                actual_hash,
                project_state.template_key.clone(),
                project_state.template_version.clone(),
                RUNTIME_DEPENDENCY_POLICY_VERSION.to_string(),
                design_context_hash,
                &ctx.run.id,
                based_on_snapshot_id,
                restored_from_version_id,
            )
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        write_workspace_json(
            &*self.workspace,
            &ctx,
            "state/dependency-state.json",
            &json!({
                "needsRestore": true,
                "reason": "draft_history_restored_without_node_modules",
                "sourceSnapshotUri": restored.source_snapshot_uri,
                "markedAt": Utc::now().to_rfc3339(),
            }),
        )
        .await?;

        let preview =
            preview_dev::preview_dev_start_tool(self.workspace.clone(), self.command.clone())
                .call(json!({}), ctx, progress)
                .await?;
        Ok(ToolResult::ok(json!({
            "status": "restored",
            "draftSnapshot": restored,
            "preview": preview.content,
            "productionBuildCreated": false,
            "publicationChanged": false,
        })))
    }
}

fn restored_source_hash(files: &[ArtifactFile]) -> String {
    let mut files = files
        .iter()
        .filter(|file| file.path != Path::new(".snapshot.json"))
        .map(|file| {
            (
                file.path.to_string_lossy().replace('\\', "/"),
                file.bytes.clone(),
            )
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

#[async_trait]
impl Tool for DraftSnapshotCreateTool {
    fn name(&self) -> &'static str {
        "draft.snapshot_create"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        if !input.as_object().is_some_and(|object| object.is_empty()) {
            return Err(ValidationError::with_kind(
                "draft.snapshot_create does not accept input fields",
                "tool.input_schema_invalid",
            ));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "draft snapshot creation allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let project_state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
            typed_recoverable(
                "draft.snapshot_create requires initialized project state",
                "draft.project_state_missing",
                json!({ "suggestedAction": "Call project.init before creating a DraftSnapshot." }),
            )
        })?;
        if project_state.template_key != "next-app" {
            return Err(typed_recoverable(
                "draft.snapshot_create is currently reserved for next-app",
                "template.operation_unsupported",
                json!({
                    "template": project_state.template_key,
                    "suggestedAction": "Use the existing preview.publish lifecycle for legacy templates."
                }),
            ));
        }

        // Dev Preview mutations already freeze and persist the current
        // revision before marking the DraftPreviewSession durable. Reuse that
        // authoritative snapshot instead of trying to create a second
        // build-based snapshot from stale outputs/build evidence.
        if let Some(session) = ctx
            .store
            .draft_preview_store()
            .active_for_project(&ctx.project_id)
        {
            if session.status == crate::visual_contracts::DraftPreviewSessionStatus::Ready
                && session.last_ready_revision == session.workspace_revision
                && session.durable_revision == session.workspace_revision
            {
                if let Some(snapshot) = ctx
                    .store
                    .get_draft_snapshot(&session.durable_snapshot_id)
                    .await
                {
                    return Ok(ToolResult::ok(json!({
                        "status": "snapshot_reused",
                        "draftSnapshot": snapshot,
                        "previewUrl": session.proxy_url,
                        "visualReview": {
                            "mode": "advisory",
                            "status": "not_requested",
                            "reason": "Visual Review sidechain has not run. This does not block generation completion."
                        }
                    })));
                }
            }
            return Err(typed_recoverable(
                "draft.snapshot_create is waiting for the current Dev Preview revision",
                "draft.preview_revision_pending",
                json!({
                    "sessionId": session.session_id,
                    "sessionEpoch": session.session_epoch,
                    "workspaceRevision": session.workspace_revision,
                    "lastReadyRevision": session.last_ready_revision,
                    "durableRevision": session.durable_revision,
                    "suggestedAction": "Call preview.dev_status until the current revision is ready and durable, then retry draft.snapshot_create."
                }),
            ));
        }

        let build = read_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json")
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "draft.snapshot_create requires successful build evidence",
                    "draft.build_missing",
                    json!({ "suggestedAction": "Call project.build and resolve any build failure first." }),
                )
            })?;
        if build.get("status").and_then(Value::as_str) != Some("success")
            || build.get("success").and_then(Value::as_bool) != Some(true)
        {
            return Err(typed_recoverable(
                "draft.snapshot_create refused unsuccessful build evidence",
                "draft.build_failed",
                json!({ "suggestedAction": "Repair the project and rerun project.build." }),
            ));
        }
        let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "draft.snapshot_create requires Runtime preview evidence",
                    "draft.preview_missing",
                    json!({ "suggestedAction": "Call preview.start after project.build." }),
                )
            })?;
        if preview.get("accessible").and_then(Value::as_bool) != Some(true) {
            return Err(typed_recoverable(
                "draft.snapshot_create refused an inaccessible preview",
                "draft.preview_unavailable",
                json!({ "suggestedAction": "Repair preview startup before completing the generation." }),
            ));
        }

        let source_snapshot_uri = build
            .get("sourceSnapshotUri")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ToolError::Terminal(
                    "successful project.build omitted sourceSnapshotUri".to_string(),
                )
            })?;
        let source_hash = build
            .get("sourceFingerprint")
            .and_then(Value::as_str)
            .filter(|value| value.len() == 64)
            .ok_or_else(|| {
                ToolError::Terminal(
                    "successful project.build omitted sourceFingerprint".to_string(),
                )
            })?;
        let design_context_hash = ctx
            .run
            .design_context_content_hash
            .clone()
            .unwrap_or_else(|| canonical_json_hash(&json!({ "designContext": "none" })));
        let based_on_snapshot_id = ctx
            .store
            .list_project_draft_snapshots(&ctx.project_id)
            .await
            .into_iter()
            .rev()
            .find(|snapshot| snapshot.created_by_run_id != ctx.run.id)
            .map(|snapshot| snapshot.snapshot_id);
        let snapshot = ctx
            .store
            .create_draft_snapshot(
                &ctx.project_id,
                source_snapshot_uri.to_string(),
                source_hash.to_string(),
                project_state.template_key.clone(),
                project_state.template_version.clone(),
                RUNTIME_DEPENDENCY_POLICY_VERSION.to_string(),
                design_context_hash,
                &ctx.run.id,
                based_on_snapshot_id,
                None,
            )
            .await
            .map_err(|error| {
                ToolError::Terminal(format!("failed to persist DraftSnapshot: {error}"))
            })?;

        Ok(ToolResult::ok(json!({
            "status": "snapshot_created",
            "draftSnapshot": snapshot,
            "previewUrl": preview.get("url").cloned().unwrap_or(Value::Null),
            "visualReview": {
                "mode": "advisory",
                "status": "not_requested",
                "reason": "Visual Review sidechain has not run. This does not block generation completion."
            }
        })))
    }
}
