use super::preview_fidelity::{
    evaluate_design_profile_fidelity, reject_unchanged_fidelity_republish,
};
use super::*;

pub(super) fn preview_rebuilding_tool() -> Arc<dyn Tool> {
    Arc::new(PreviewRebuildingTool)
}

pub(super) fn preview_report_candidate_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(PreviewReportCandidateTool { workspace })
}

pub(super) fn preview_publish_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewPublishTool { workspace, command })
}

struct PreviewRebuildingTool;

#[async_trait]
impl Tool for PreviewRebuildingTool {
    fn name(&self) -> &'static str {
        "preview.rebuilding"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "previousVersionId": string_schema("Previous promoted version id") }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview rebuild event allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let previous_version_id = input
            .get("previousVersionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let _ = ctx
            .store
            .append_event(AgentEvent::PreviewRebuilding {
                run_id: ctx.run.id.clone(),
                previous_version_id,
                timestamp: Utc::now(),
            })
            .await;
        Ok(ToolResult::ok(json!({ "rebuilding": true })))
    }
}

struct PreviewReportCandidateTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for PreviewReportCandidateTool {
    fn name(&self) -> &'static str {
        "preview.report_candidate"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "url": string_schema("Candidate preview URL"),
                "screenshotId": string_schema("Screenshot artifact id"),
                "sourceSnapshotUri": string_schema("Source snapshot URI")
            }),
            &["url"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "url", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
        if !is_internal_preview_url(url) {
            return PermissionResult::Deny {
                message: "preview.report_candidate public preview URL is not allowed".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "public preview candidate URL denied".to_string(),
                },
            };
        }
        allow_with_input(input, "preview candidate report allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let requested_url = required_str(&input, "url")?.to_string();
        let url = if ctx.remote_workspace {
            let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
                .await
                .ok_or_else(|| {
                    typed_recoverable(
                        "preview.report_candidate requires an active Runtime preview lease".to_string(),
                        "preview.lease_missing",
                        json!({ "suggestedAction": "Call preview.start before preview.report_candidate." }),
                    )
                })?;
            let proxy_url = preview
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    typed_recoverable(
                        "preview.report_candidate lease has no Runtime proxy URL".to_string(),
                        "preview.lease_invalid",
                        json!({ "preview": preview }),
                    )
                })?
                .to_string();
            if !proxy_url.starts_with(&ctx.runtime_public_base_url) {
                return Err(ToolError::Terminal(
                    "preview.report_candidate refused a URL outside the Runtime proxy".to_string(),
                ));
            }
            proxy_url
        } else {
            requested_url
        };
        verify_preview_accessible(&url).await?;
        let screenshot_id = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.report_candidate requires screenshotId from browser.screenshot before creating a candidate".to_string(),
                    "preview.screenshot_missing",
                    json!({
                        "suggestedAction": "Call browser.screenshot and pass screenshotId to preview.report_candidate."
                    }),
                )
            })?;
        let source_snapshot_uri = input
            .get("sourceSnapshotUri")
            .and_then(Value::as_str)
            .map(str::to_string);
        report_preview_candidate(
            &*self.workspace,
            &ctx,
            url,
            screenshot_id,
            source_snapshot_uri.as_deref(),
        )
        .await
        .map(ToolResult::ok)
    }
}

struct PreviewPublishTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewPublishTool {
    fn name(&self) -> &'static str {
        "preview.publish"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "cwd": string_schema("Workspace cwd"),
                "buildTimeoutMs": { "type": "integer", "minimum": 1 },
                "url": string_schema("Preview URL"),
                "port": { "type": "integer", "minimum": 1 },
                "command": string_schema("Preview command label"),
                "mode": string_schema("Preview mode: static or framework"),
                "screenshotId": string_schema("Screenshot artifact id")
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        if let Some(url) = input.get("url") {
            require_string(&json!({ "url": url.clone() }), "url", self.name())?;
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        if let Some(url) = input.get("url").and_then(Value::as_str) {
            if !is_internal_preview_url(url) {
                return PermissionResult::Deny {
                    message: "preview.publish public preview URL is not allowed".to_string(),
                    reason: PermissionReason::Rule {
                        source: RuleSource::Runtime,
                        rule_content: "public preview publish URL denied".to_string(),
                    },
                };
            }
        }
        allow_with_input(input, "preview publish allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let build_tool = project_build::ProjectBuildTool {
            workspace: self.workspace.clone(),
            command: self.command.clone(),
        };
        let mut build_input = json!({});
        if let Some(cwd) = input.get("cwd").cloned() {
            build_input["cwd"] = cwd;
        }
        if let Some(timeout) = input.get("buildTimeoutMs").cloned() {
            build_input["timeoutMs"] = timeout;
        }
        let build = build_tool
            .call(build_input, ctx.clone(), progress.clone())
            .await?
            .content;
        reject_unchanged_fidelity_republish(&ctx, &build).await?;

        let preview_tool = preview_lifecycle::PreviewStartTool {
            workspace: self.workspace.clone(),
            command: self.command.clone(),
        };
        let mut preview_input = json!({});
        if ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
            for key in ["url", "port", "command", "mode"] {
                if let Some(value) = input.get(key).cloned() {
                    preview_input[key] = value;
                }
            }
        }
        let preview = preview_tool
            .call(preview_input, ctx.clone(), progress.clone())
            .await?
            .content;
        let url = preview
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable(
                    "preview.publish preview.start did not return url".to_string(),
                )
            })?
            .to_string();

        let browser_tool = browser::BrowserOpenTool {
            workspace: self.workspace.clone(),
        };
        browser_tool
            .call(json!({ "url": url.clone() }), ctx.clone(), progress.clone())
            .await?;

        let screenshot_tool = browser::BrowserScreenshotTool {
            workspace: self.workspace.clone(),
        };
        let screenshot_input = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(|screenshot_id| json!({ "screenshotId": screenshot_id }))
            .unwrap_or_else(|| json!({}));
        let screenshot = screenshot_tool
            .call(screenshot_input, ctx.clone(), progress)
            .await?
            .content;
        let screenshot_id = screenshot
            .get("screenshotId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable(
                    "preview.publish browser.screenshot did not return screenshotId".to_string(),
                )
            })?
            .to_string();

        let published =
            report_preview_candidate(&*self.workspace, &ctx, url, screenshot_id, None).await?;
        let fidelity = evaluate_design_profile_fidelity(
            &*self.workspace,
            &ctx,
            preview
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            screenshot
                .get("screenshotId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &published,
        )
        .await?;
        Ok(ToolResult::ok(json!({
            "published": true,
            "build": build,
            "preview": preview,
            "screenshot": screenshot,
            "promotion": published,
            "designProfileFidelity": fidelity,
        })))
    }
}

async fn report_preview_candidate(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    url: String,
    screenshot_id: String,
    source_snapshot_uri: Option<&str>,
) -> Result<Value, ToolError> {
    verify_preview_accessible(&url).await?;
    verify_screenshot_artifact(workspace, ctx, &screenshot_id).await?;
    let latest_build = read_workspace_json(workspace, ctx, "outputs/build/latest.json")
        .await
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires successful project.build evidence".to_string(),
                "preview.build_missing",
                json!({
                    "suggestedAction": "Run project.build or preview.publish before reporting a candidate."
                }),
            )
        })?;
    if !latest_build
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(typed_recoverable(
            "preview.report_candidate blocked because latest project.build did not succeed"
                .to_string(),
            "preview.build_failed",
            json!({
                "latestBuild": latest_build,
                "suggestedAction": "Fix the build error, rerun project.build, then publish again."
            }),
        ));
    }
    let latest_source_snapshot_uri = latest_build
        .get("sourceSnapshotUri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires build sourceSnapshotUri evidence".to_string(),
                "preview.source_snapshot_missing",
                json!({
                    "latestBuild": latest_build.clone(),
                    "suggestedAction": "Rerun project.build so sourceSnapshotUri is recorded."
                }),
            )
        })?;
    let source_snapshot_uri = source_snapshot_uri.unwrap_or(latest_source_snapshot_uri);
    if source_snapshot_uri != latest_source_snapshot_uri {
        return Err(typed_recoverable(
            format!(
                "preview.report_candidate sourceSnapshotUri {source_snapshot_uri} does not match latest project.build {latest_source_snapshot_uri}"
            ),
            "preview.source_snapshot_mismatch",
            json!({
                "receivedSourceSnapshotUri": source_snapshot_uri,
                "latestSourceSnapshotUri": latest_source_snapshot_uri,
                "suggestedAction": "Use the latest project.build sourceSnapshotUri or rerun project.build."
            }),
        ));
    }
    let candidate = ctx
        .store
        .create_project_version_candidate(
            &ctx.project_id,
            &ctx.run.id,
            url.clone(),
            Some(screenshot_id.clone()),
            Some(source_snapshot_uri.to_string()),
        )
        .await;
    let candidate_manifest_hash = latest_build
        .get("candidateManifestHash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires candidate manifest evidence".to_string(),
                "preview.candidate_manifest_missing",
                json!({ "latestBuild": latest_build.clone() }),
            )
        })?;
    let candidate_output_path = latest_build
        .get("candidateOutputPath")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires candidate output evidence".to_string(),
                "preview.candidate_manifest_missing",
                json!({ "latestBuild": latest_build.clone() }),
            )
        })?;
    let candidate_root = resolve_path(candidate_output_path, &ctx.workspace_root);
    let candidate_manifest = workspace
        .read_to_string(
            ctx,
            &candidate_root.join(".anydesign-candidate-manifest.json"),
        )
        .await
        .map_err(|error| {
            typed_recoverable(
                format!("failed to read candidate manifest: {error}"),
                "artifact.candidate_mismatch",
                json!({ "candidateOutputPath": candidate_output_path }),
            )
        })?;
    let actual_manifest_hash = sha256_hex(candidate_manifest.as_bytes());
    if actual_manifest_hash != candidate_manifest_hash {
        return Err(typed_recoverable(
            "candidate snapshot does not match build evidence".to_string(),
            "artifact.candidate_mismatch",
            json!({
                "expectedManifestHash": candidate_manifest_hash,
                "actualManifestHash": actual_manifest_hash,
            }),
        ));
    }
    let publisher = FileArtifactPublisher::new(&ctx.runtime_storage_dir);
    let build_id = latest_build
        .get("buildId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires buildId evidence".to_string(),
                "preview.build_missing",
                json!({ "latestBuild": latest_build.clone() }),
            )
        })?;
    let expected_current_version_id = ctx
        .run
        .output_version_id
        .as_deref()
        .or(ctx.run.base_version_id.as_deref());
    let publish = ctx
        .store
        .begin_artifact_publish(
            &ctx.project_id,
            &ctx.run.id,
            build_id,
            &candidate.id,
            candidate_manifest_hash,
            source_snapshot_uri,
            expected_current_version_id,
        )
        .await
        .map_err(|error| ToolError::Terminal(format!("artifact stage state failed: {error}")))?;
    let export_root = ctx
        .runtime_storage_dir
        .join("artifact-exports")
        .join(safe_segment(&ctx.run.id))
        .join(safe_segment(&candidate.id));
    let export_receipt = workspace
        .export_tree(
            ctx,
            &candidate_root,
            &export_root,
            &[".anydesign-candidate-manifest.json".to_string()],
        )
        .await
        .map_err(|error| ToolError::Terminal(format!("artifact export failed: {error}")))?;
    let template = resolve_project_template_spec(ctx)?;
    let staged_artifact = match publisher
        .stage_directory(
            &ctx.project_id,
            &candidate.id,
            candidate_manifest_hash,
            &export_receipt.target_root,
            &template,
        )
        .await
    {
        Ok(staged) => staged,
        Err(error) => {
            cleanup_runtime_export(&export_receipt.target_root);
            ctx.store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::Failed,
                    None,
                    None,
                    None,
                    Some(&error.to_string()),
                )
                .await
                .ok();
            return Err(ToolError::Terminal(format!(
                "artifact stage failed: {error}"
            )));
        }
    };
    cleanup_runtime_export(&export_receipt.target_root);
    ctx.store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Staged,
            Some(&staged_artifact.artifact_manifest_hash),
            Some(&staged_artifact.staged_uri),
            None,
            None,
        )
        .await
        .map_err(|error| ToolError::Terminal(format!("artifact staged state failed: {error}")))?;
    let _ = ctx
        .store
        .append_event(AgentEvent::PreviewCandidate {
            run_id: ctx.run.id.clone(),
            url,
            version_id: candidate.id.clone(),
            screenshot_id: Some(screenshot_id.clone()),
            timestamp: Utc::now(),
        })
        .await;
    let review_run = match ctx
        .store
        .create_child_run(
            &ctx.run.id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{}", candidate.id)),
            vec![],
        )
        .await
    {
        Ok(run) => run,
        Err(error) => {
            publisher.abort(&staged_artifact).await.ok();
            ctx.store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::GarbageCollectable,
                    None,
                    None,
                    None,
                    Some(&error.to_string()),
                )
                .await
                .ok();
            return Err(ToolError::Recoverable(format!(
                "failed to create visual review child run: {error}"
            )));
        }
    };
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "progress",
            Some("assistant"),
            "Queued visual review for candidate preview.",
            Some(json!({
                "versionId": candidate.id.clone(),
                "reviewRunId": review_run.id.clone(),
            })),
        )
        .await;
    let gate_report =
        promotion_gate_report_from_workspace(workspace, ctx, Some(&screenshot_id)).await;
    ctx.store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Validating,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|error| {
            ToolError::Terminal(format!("artifact validating state failed: {error}"))
        })?;
    if let Err(error) = ctx
        .store
        .update_run_status(&review_run.id, AgentRunStatus::Completed)
        .await
    {
        publisher.abort(&staged_artifact).await.ok();
        ctx.store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::GarbageCollectable,
                None,
                None,
                None,
                Some(&error.to_string()),
            )
            .await
            .ok();
        return Err(ToolError::Recoverable(format!(
            "failed to complete visual review child run: {error}"
        )));
    }
    if let Err(error) = validate_preview_promotion(
        &ctx.store,
        &ctx.project_id,
        &ctx.run.id,
        &candidate.id,
        gate_report.clone(),
    )
    .await
    {
        publisher.abort(&staged_artifact).await.ok();
        ctx.store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::GarbageCollectable,
                None,
                None,
                None,
                Some(&error.to_string()),
            )
            .await
            .ok();
        return Err(ToolError::Recoverable(format!(
            "preview promotion rejected: {error}"
        )));
    }
    ctx.store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Promoting,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|error| {
            ToolError::Terminal(format!("artifact promoting state failed: {error}"))
        })?;
    let artifact_uri = match publisher.promote(&staged_artifact).await {
        Ok(uri) => uri,
        Err(error) => {
            ctx.store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::ReconcileRequired,
                    None,
                    None,
                    None,
                    Some(&error.to_string()),
                )
                .await
                .ok();
            return Err(ToolError::Terminal(format!(
                "artifact promote failed: {error}"
            )));
        }
    };
    ctx.store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Promoting,
            None,
            None,
            Some(&artifact_uri),
            None,
        )
        .await
        .map_err(|error| ToolError::Terminal(format!("artifact URI state failed: {error}")))?;
    let promoted = match promote_preview_cas(
        &ctx.store,
        &ctx.project_id,
        &ctx.run.id,
        &candidate.id,
        gate_report,
        expected_current_version_id,
    )
    .await
    {
        Ok(promoted) => promoted,
        Err(error) => {
            let committed = ctx
                .store
                .get_artifact_publish(&publish.id)
                .await
                .is_some_and(|record| record.status == ArtifactPublishStatus::Promoted);
            if !committed {
                ctx.store
                    .transition_artifact_publish(
                        &publish.id,
                        ArtifactPublishStatus::ReconcileRequired,
                        None,
                        None,
                        None,
                        Some(&error.to_string()),
                    )
                    .await
                    .ok();
            }
            return Err(ToolError::Recoverable(format!(
                "preview promotion rejected: {error}"
            )));
        }
    };
    Ok(json!({
        "versionId": promoted.id,
        "reviewRunId": review_run.id.clone(),
        "status": promoted.status,
        "url": promoted.preview_url,
        "artifactUri": artifact_uri,
        "artifactManifestHash": staged_artifact.artifact_manifest_hash,
        "artifactPublishId": publish.id,
        "artifactExportBytes": export_receipt.total_bytes,
        "artifactExportFileCount": export_receipt.file_count,
        "artifactExportManifestHash": export_receipt.manifest_hash,
        "candidateManifestHash": candidate_manifest_hash,
    }))
}

fn cleanup_runtime_export(path: &Path) {
    // remote-fs-boundary: allow-begin runtime-storage-export-cleanup
    fs::remove_dir_all(path).ok();
    // remote-fs-boundary: allow-end runtime-storage-export-cleanup
}

pub(super) async fn collect_artifact_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    candidate_root: &Path,
) -> Result<Vec<ArtifactFile>, ToolError> {
    let mut files = Vec::new();
    let mut stack = vec![candidate_root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = workspace
            .list_dir(ctx, &directory)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        for entry in entries {
            match entry.kind {
                WorkspaceEntryKind::Dir => stack.push(entry.path),
                WorkspaceEntryKind::File => {
                    let relative = entry
                        .path
                        .strip_prefix(candidate_root)
                        .map_err(|error| ToolError::Terminal(error.to_string()))?
                        .to_path_buf();
                    if relative == Path::new(".anydesign-candidate-manifest.json") {
                        continue;
                    }
                    files.push(ArtifactFile {
                        path: relative,
                        bytes: workspace
                            .read_bytes(ctx, &entry.path)
                            .await
                            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
                    });
                }
            }
        }
    }
    Ok(files)
}
