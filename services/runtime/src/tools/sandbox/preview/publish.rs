use super::preview_acceptance::collect_and_persist_acceptance;
use super::preview_fidelity::{
    evaluate_design_profile_fidelity, reject_unchanged_fidelity_republish,
};
use super::preview_validation::{
    collect_and_persist_generation_validation, validation_failure_owners,
};
use super::*;
use crate::{
    runtime_storage::FileAcceptanceReportStore, types::canonical_json_hash,
    visual_contracts::RUNTIME_DEPENDENCY_POLICY_VERSION,
};

const DEFAULT_MAX_ACCEPTANCE_REPAIR_CYCLES: u32 = 3;
const REPAIR_CONTEXT_MAX_BYTES: usize = 16 * 1024;
const REPAIR_CONTEXT_MAX_ESTIMATED_TOKENS: u64 = 4_000;

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
        reopen_run_after_candidate_rejection(&ctx).await?;
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

    fn is_enabled(&self, ctx: &ToolContext) -> bool {
        ctx.policy_profile == RuntimePolicyProfile::LocalE2e
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

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        if ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
            return PermissionResult::Deny {
                message: "preview.report_candidate is retired outside local E2E; use preview.publish so generation, acceptance, and atomic completion gates cannot be bypassed".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "manual candidate reporting is local-e2e-only".to_string(),
                },
            };
        }
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
            None,
            &ctx,
            url,
            screenshot_id,
            source_snapshot_uri.as_deref(),
            None,
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
        if ctx
            .run
            .project_state_snapshot
            .as_ref()
            .is_some_and(|state| state.template_key == "next-app")
        {
            return Err(typed_recoverable(
                "preview.publish is a legacy candidate/version operation and is not valid for next-app generation",
                "template.operation_unsupported",
                json!({
                    "template": "next-app",
                    "suggestedAction": "Call project.build, preview.start, browser.screenshot, and draft.snapshot_create. A WorkVersion is created only by the user-initiated PublishWorkflow."
                }),
            ));
        }
        let source_root = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|cwd| {
                check_context_workspace_path(&resolve_path(cwd, &ctx.workspace_root), &ctx)
                    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
            })
            .transpose()?
            .unwrap_or_else(|| default_project_dir(&ctx));
        reject_unchanged_candidate_validation_republish(&*self.workspace, &ctx, &source_root)
            .await?;
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

        // Create the immutable candidate identity before fidelity evaluation so
        // every failure (including an unavailable enforced verifier) has a
        // durable candidate/evidence target. Artifact staging happens only
        // after the DCP gate accepts this exact candidate; current-version
        // promotion is deferred to the atomic run.complete boundary.
        let source_snapshot_uri = build
            .get("sourceSnapshotUri")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.publish build did not return sourceSnapshotUri".to_string(),
                    "preview.source_snapshot_missing",
                    json!({ "build": build }),
                )
            })?;
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
        let _ = ctx
            .store
            .append_event(AgentEvent::PreviewCandidate {
                run_id: ctx.run.id.clone(),
                url: url.clone(),
                version_id: candidate.id.clone(),
                screenshot_id: Some(screenshot_id.clone()),
                timestamp: Utc::now(),
            })
            .await;
        let fidelity = match evaluate_design_profile_fidelity(
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
            &json!({ "versionId": candidate.id.clone() }),
        )
        .await
        {
            Ok(fidelity) => fidelity,
            Err(error) => {
                reopen_run_after_candidate_rejection(&ctx).await?;
                return Err(error);
            }
        };
        if let Err(error) = block_required_design_context_verification(
            ctx.run
                .design_context_effective_compatibility_mode
                .as_deref(),
            &fidelity,
        ) {
            reopen_run_after_candidate_rejection(&ctx).await?;
            return Err(error);
        }
        let published = report_preview_candidate(
            &*self.workspace,
            Some(&*self.command),
            &ctx,
            url,
            screenshot_id,
            Some(source_snapshot_uri),
            Some(candidate),
        )
        .await?;
        let project_state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
            ToolError::Terminal(
                "preview.publish requires a frozen project state to create DraftSnapshot"
                    .to_string(),
            )
        })?;
        let source_hash = build
            .get("sourceFingerprint")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.publish build did not return sourceFingerprint".to_string(),
                    "preview.source_fingerprint_missing",
                    json!({ "build": build }),
                )
            })?;
        let design_context_hash = ctx
            .run
            .design_context_content_hash
            .clone()
            .unwrap_or_else(|| canonical_json_hash(&json!({ "designContext": "none" })));
        let draft_snapshot = ctx
            .store
            .create_draft_revision_snapshot(
                &ctx.project_id,
                source_snapshot_uri.to_string(),
                source_hash.to_string(),
                project_state.template_key.clone(),
                project_state.template_version.clone(),
                RUNTIME_DEPENDENCY_POLICY_VERSION.to_string(),
                design_context_hash,
                &ctx.run.id,
                None,
                None,
            )
            .await
            .map_err(|error| {
                ToolError::Terminal(format!(
                    "preview.publish failed to persist DraftSnapshot: {error}"
                ))
            })?;
        Ok(ToolResult::ok(json!({
            "published": true,
            "build": build,
            "preview": preview,
            "screenshot": screenshot,
            "promotion": published,
            "draftSnapshot": draft_snapshot,
            "designProfileFidelity": fidelity,
        })))
    }
}

async fn reject_unchanged_candidate_validation_republish(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    source_root: &Path,
) -> Result<(), ToolError> {
    let latest_validation = ctx
        .store
        .conversation_items(&ctx.project_id)
        .await
        .into_iter()
        .rev()
        .find(|item| {
            item.run_id.as_deref() == Some(&ctx.run.id)
                && matches!(
                    item.kind.as_str(),
                    "generation_validation_checked" | "acceptance_validation_checked"
                )
        });
    let Some(latest_validation) = latest_validation else {
        return Ok(());
    };
    let Some(metadata) = latest_validation.metadata else {
        return Ok(());
    };
    if metadata.get("status").and_then(Value::as_str) != Some("failed") {
        return Ok(());
    }
    let previous_fingerprint = metadata
        .get("sourceFingerprint")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let Some(previous_fingerprint) = previous_fingerprint else {
        return Ok(());
    };
    let current_fingerprint = project_source_fingerprint(workspace, ctx, source_root).await?;
    if current_fingerprint != previous_fingerprint {
        return Ok(());
    }
    let validation_kind = match latest_validation.kind.as_str() {
        "acceptance_validation_checked" => "acceptance",
        _ => "generation",
    };
    Err(ToolError::typed_recoverable(
        format!(
            "preview.publish blocked because project source is unchanged since the failed {validation_kind} validation"
        ),
        format!("{validation_kind}.no_source_change_after_validation_failure"),
        json!({
            "sourceFingerprint": current_fingerprint,
            "failedCheckIds": metadata.get("failedCheckIds").cloned().unwrap_or_else(|| json!([])),
            "candidateVersionId": metadata.get("candidateVersionId").cloned(),
            "validationKind": validation_kind,
            "suggestedAction": "Read the persisted validation report, edit project source to address the failed checks, then call preview.publish again. Rebuilding unchanged source does not count as a repair."
        }),
    ))
}

async fn reopen_run_after_candidate_rejection(ctx: &ToolContext) -> Result<(), ToolError> {
    ctx.store
        .update_run_status(&ctx.run.id, AgentRunStatus::Running)
        .await
        .map(|_| ())
        .map_err(|error| {
            ToolError::Terminal(format!(
                "failed to reopen run after candidate rejection: {error}"
            ))
        })
}

fn block_required_design_context_verification(
    effective_compatibility_mode: Option<&str>,
    fidelity: &Value,
) -> Result<(), ToolError> {
    let required_failures = fidelity
        .get("requiredFailedRuleIds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if effective_compatibility_mode == Some("enforced") && !required_failures.is_empty() {
        return Err(ToolError::typed_recoverable(
            "preview.publish blocked by required Design Context verification failures",
            "design_context.required_verification_failed",
            json!({
                "requiredFailedRuleIds": required_failures,
                "fidelityReportPath": "state/design-profile-fidelity.json",
                "suggestedAction": "Read the fidelity report, repair source under the declared app root, then rebuild and publish again."
            }),
        ));
    }
    Ok(())
}

async fn report_preview_candidate(
    workspace: &dyn WorkspaceBackend,
    command: Option<&dyn SandboxCommandBackend>,
    ctx: &ToolContext,
    url: String,
    screenshot_id: String,
    source_snapshot_uri: Option<&str>,
    existing_candidate: Option<crate::types::ProjectVersion>,
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
    let candidate_was_announced = existing_candidate.is_some();
    let candidate = match existing_candidate {
        Some(candidate) => {
            if candidate.project_id != ctx.project_id
                || candidate.created_by_run_id != ctx.run.id
                || candidate.preview_url != url
                || candidate.screenshot_id.as_deref() != Some(screenshot_id.as_str())
                || candidate.source_snapshot_uri.as_deref() != Some(source_snapshot_uri)
            {
                return Err(ToolError::Terminal(
                    "preview candidate identity does not match the current publish evidence"
                        .to_string(),
                ));
            }
            candidate
        }
        None => {
            ctx.store
                .create_project_version_candidate(
                    &ctx.project_id,
                    &ctx.run.id,
                    url.clone(),
                    Some(screenshot_id.clone()),
                    Some(source_snapshot_uri.to_string()),
                )
                .await
        }
    };
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
    let expected_current_version_id = match ctx.run.base_version_id.clone() {
        Some(base_version_id) => Some(base_version_id),
        None => ctx
            .store
            .current_project_version(&ctx.project_id)
            .await
            .map(|version| version.id),
    };
    let publish = ctx
        .store
        .begin_artifact_publish(
            &ctx.project_id,
            &ctx.run.id,
            build_id,
            &candidate.id,
            candidate_manifest_hash,
            source_snapshot_uri,
            expected_current_version_id.as_deref(),
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
    if !candidate_was_announced {
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
    }
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
            "preview candidate validation rejected: {error}"
        )));
    }
    if candidate_was_announced {
        let (validation_report, validation_report_uri) =
            match collect_and_persist_generation_validation(
                workspace,
                command,
                ctx,
                &template,
                &candidate.id,
                &candidate_manifest,
                &latest_build,
                &staged_artifact,
                &candidate.preview_url,
            )
            .await
            {
                Ok(validation) => validation,
                Err(error) => {
                    publisher.abort(&staged_artifact).await.ok();
                    ctx.store
                        .transition_artifact_publish(
                            &publish.id,
                            ArtifactPublishStatus::GarbageCollectable,
                            None,
                            None,
                            None,
                            Some(&format!("{error:?}")),
                        )
                        .await
                        .ok();
                    return Err(error);
                }
            };
        let generation_contract = template.generation_contract().map_err(|error| {
            ToolError::Terminal(format!("generation contract is invalid: {error}"))
        })?;
        let validation_blockers = validation_report.promotion_blockers(&generation_contract);
        if !validation_blockers.is_empty() {
            publisher.abort(&staged_artifact).await.ok();
            ctx.store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::GarbageCollectable,
                    None,
                    None,
                    None,
                    Some("generation validation failed"),
                )
                .await
                .ok();
            let entry_route_probe = validation_report
                .evidence
                .get("entryRouteProbe")
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "status": "failed",
                        "owner": "runtime",
                        "reason": "entry_route_probe_missing"
                    })
                });
            let failure_owners = validation_failure_owners(&validation_report, &entry_route_probe);
            let non_source_owners = validation_blockers
                .iter()
                .filter_map(|blocker| {
                    failure_owners
                        .get(&blocker.check_id)
                        .filter(|owner| owner.as_str() != "source")
                        .map(|owner| (blocker.check_id.clone(), owner.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            if !non_source_owners.is_empty() {
                return Err(ToolError::TerminalWithMetadata {
                    message: "preview.publish stopped because validation is owned by Runtime, Artifact, or Serving rather than project source"
                        .to_string(),
                    error_kind: "generation.platform_validation_failed".to_string(),
                    metadata: json!({
                        "validationReportUri": validation_report_uri,
                        "candidateManifestHash": candidate_manifest_hash,
                        "failureOwners": non_source_owners,
                        "entryRouteProbe": entry_route_probe,
                        "repairAllowed": false,
                        "suggestedAction": "Do not modify project source. Preserve the Candidate evidence and repair the owning platform component."
                    }),
                });
            }
            let repair_attempt = ctx
                .store
                .conversation_items(&ctx.project_id)
                .await
                .into_iter()
                .filter(|item| {
                    item.run_id.as_deref() == Some(&ctx.run.id)
                        && item.kind == "generation_validation_checked"
                        && item
                            .metadata
                            .as_ref()
                            .and_then(|metadata| metadata.get("status"))
                            .and_then(Value::as_str)
                            == Some("failed")
                })
                .count();
            if repair_attempt > 1 {
                return Err(ToolError::TerminalWithMetadata {
                    message: "preview.publish exhausted the bounded generation repair cycle"
                        .to_string(),
                    error_kind: "generation.repair_exhausted".to_string(),
                    metadata: json!({
                        "candidateManifestHash": candidate_manifest_hash,
                        "repairAttempt": repair_attempt,
                        "maxRepairCycles": 1,
                        "repairAllowed": false,
                        "failureOwners": failure_owners,
                    }),
                });
            }
            let mut target_files = template
                .editable_surface
                .primary_routes
                .iter()
                .map(|route| route.source.clone())
                .chain(template.editable_surface.inspection_hints.iter().cloned())
                .filter(|path| {
                    matches!(
                        Path::new(path)
                            .extension()
                            .and_then(|extension| extension.to_str()),
                        Some("js" | "jsx" | "ts" | "tsx" | "md" | "mdx" | "css" | "json")
                    )
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(8)
                .collect::<Vec<_>>();
            if target_files.is_empty() {
                target_files.push("project source selected by the failed check".to_string());
            }
            let repair_context = json!({
                "schemaVersion": "generation-repair-context@1",
                "candidateManifestHash": candidate_manifest_hash,
                "entryRouteProbe": entry_route_probe,
                "blockers": validation_blockers.iter().map(|blocker| json!({
                    "checkId": blocker.check_id,
                    "status": blocker.status,
                    "owner": "source",
                    "diagnostic": blocker.message.as_deref().unwrap_or("source validation failed").chars().take(512).collect::<String>(),
                })).collect::<Vec<_>>(),
                "targetFiles": target_files,
                "limits": {
                    "maxBytes": REPAIR_CONTEXT_MAX_BYTES,
                    "maxEstimatedTokens": REPAIR_CONTEXT_MAX_ESTIMATED_TOKENS,
                    "maxRepairCycles": 1
                }
            });
            let repair_context_text = serde_json::to_string_pretty(&repair_context)
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
            let (repair_context_bytes, repair_context_estimated_tokens) =
                validate_repair_context_size(&repair_context_text)?;
            write_workspace_json(workspace, ctx, "state/repair-context.json", &repair_context)
                .await?;
            reopen_run_after_candidate_rejection(ctx).await?;
            return Err(ToolError::typed_recoverable(
                "preview.publish blocked by the generation validation contract",
                "generation.validation_failed",
                json!({
                    "validationReportUri": validation_report_uri,
                    "repairContextPath": "state/repair-context.json",
                    "validationReportPath": "state/repair-context.json",
                    "candidateManifestHash": candidate_manifest_hash,
                    "repairContextBytes": repair_context_bytes,
                    "repairContextEstimatedTokens": repair_context_estimated_tokens,
                    "blockers": validation_blockers.iter().map(|blocker| json!({
                        "checkId": blocker.check_id,
                        "status": blocker.status,
                        "owner": "source",
                    })).collect::<Vec<_>>(),
                    "repairAllowed": true,
                    "suggestedAction": "Read only state/repair-context.json, make one bounded mutation to a listed target file, then publish one new candidate."
                }),
            ));
        }
        let (acceptance_report, acceptance_report_uri) = match collect_and_persist_acceptance(
            workspace,
            ctx,
            &candidate.id,
            candidate_manifest_hash,
        )
        .await
        {
            Ok(report) => report,
            Err(error) => {
                publisher.abort(&staged_artifact).await.ok();
                ctx.store
                    .transition_artifact_publish(
                        &publish.id,
                        ArtifactPublishStatus::GarbageCollectable,
                        None,
                        None,
                        None,
                        Some(&format!("{error:?}")),
                    )
                    .await
                    .ok();
                return Err(error);
            }
        };
        if !acceptance_report.passed() {
            let repair_attempt = FileAcceptanceReportStore::new(&ctx.runtime_storage_dir)
                .failed_report_count(&ctx.project_id, &ctx.run.id)
                .map_err(|error| {
                    ToolError::Terminal(format!(
                        "acceptance repair budget could not be evaluated: {error}"
                    ))
                })?;
            let max_repair_cycles = std::env::var("RUNTIME_MAX_ACCEPTANCE_REPAIR_CYCLES")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_MAX_ACCEPTANCE_REPAIR_CYCLES);
            let repair_exhausted = repair_attempt >= max_repair_cycles;
            publisher.abort(&staged_artifact).await.ok();
            ctx.store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::GarbageCollectable,
                    None,
                    None,
                    None,
                    Some("acceptance contract failed"),
                )
                .await
                .ok();
            let state = if repair_exhausted {
                "acceptance_repair_exhausted"
            } else {
                "acceptance_repairing"
            };
            let _ = ctx
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: ctx.run.id.clone(),
                    state: format!("{state}:attempt={repair_attempt}:limit={max_repair_cycles}"),
                    timestamp: Utc::now(),
                })
                .await;
            let metadata = json!({
                "acceptanceReportUri": acceptance_report_uri,
                "acceptanceReportPath": "state/acceptance-report.json",
                "contractDigest": acceptance_report.contract_digest,
                "candidateManifestHash": candidate_manifest_hash,
                "failedChecks": acceptance_report.checks.iter().filter(|check| {
                    check.status == crate::acceptance_contract::AcceptanceCheckStatus::Failed
                }).collect::<Vec<_>>(),
                "repairAttempt": repair_attempt,
                "maxRepairCycles": max_repair_cycles,
                "repairExhausted": repair_exhausted,
                "suggestedAction": if repair_exhausted {
                    "Repair budget is exhausted. Preserve the rejected Candidate evidence and request a revised user Brief or explicit retry."
                } else {
                    "Read state/acceptance-report.json, repair the existing Candidate source without changing the Brief, then publish a new Candidate."
                }
            });
            if !repair_exhausted {
                reopen_run_after_candidate_rejection(ctx).await?;
            }
            return if repair_exhausted {
                Err(ToolError::TerminalWithMetadata {
                    message: "preview.publish exhausted the frozen Brief acceptance repair budget"
                        .to_string(),
                    error_kind: "acceptance.repair_exhausted".to_string(),
                    metadata,
                })
            } else {
                Err(ToolError::typed_recoverable(
                    "preview.publish blocked by the frozen Brief acceptance contract",
                    "acceptance.validation_failed",
                    metadata,
                ))
            };
        }
        let ready = ctx
            .store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::Ready,
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(|error| {
                ToolError::Terminal(format!("artifact candidate-ready state failed: {error}"))
            })?;
        ctx.store
            .set_run_output_version(&ctx.run.id, candidate.id.clone())
            .await
            .map_err(|error| {
                ToolError::Terminal(format!("candidate output version state failed: {error}"))
            })?;
        return Ok(json!({
            "versionId": candidate.id,
            "reviewRunId": review_run.id,
            "status": "candidate_ready",
            "url": candidate.preview_url.clone(),
            "previewUrl": candidate.preview_url,
            "artifactManifestHash": staged_artifact.artifact_manifest_hash,
            "artifactPublishId": ready.id,
            "artifactExportBytes": export_receipt.total_bytes,
            "artifactExportFileCount": export_receipt.file_count,
            "artifactExportManifestHash": export_receipt.manifest_hash,
            "candidateManifestHash": candidate_manifest_hash,
            "validationReportUri": validation_report_uri,
            "validationChecks": validation_report.checks,
            "acceptanceReportUri": acceptance_report_uri,
            "acceptanceChecks": acceptance_report.checks,
        }));
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
        expected_current_version_id.as_deref(),
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

fn validate_repair_context_size(text: &str) -> Result<(usize, u64), ToolError> {
    let bytes = text.len();
    if bytes > REPAIR_CONTEXT_MAX_BYTES {
        return Err(ToolError::Terminal(
            "bounded generation repair context exceeded 16 KiB".to_string(),
        ));
    }
    let estimated_tokens = (bytes as u64).div_ceil(4);
    if estimated_tokens > REPAIR_CONTEXT_MAX_ESTIMATED_TOKENS {
        return Err(ToolError::Terminal(
            "bounded generation repair context exceeded 4,000 estimated tokens".to_string(),
        ));
    }
    Ok((bytes, estimated_tokens))
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
                    if relative == Path::new(".anydesign-candidate-manifest.json")
                        || relative == Path::new(".snapshot.json")
                    {
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

#[cfg(test)]
mod tests {
    use super::{block_required_design_context_verification, validate_repair_context_size};
    use crate::tools::runtime::ToolError;
    use serde_json::json;

    #[test]
    fn enforced_publish_is_blocked_but_observe_mode_is_not() {
        let report = json!({
            "requiredFailedRuleIds": ["craft:accessibility-baseline:image-alt"]
        });
        let error =
            block_required_design_context_verification(Some("enforced"), &report).unwrap_err();
        match error {
            ToolError::RecoverableWithMetadata {
                error_kind,
                metadata,
                ..
            } => {
                assert_eq!(error_kind, "design_context.required_verification_failed");
                assert_eq!(
                    metadata["requiredFailedRuleIds"],
                    json!(["craft:accessibility-baseline:image-alt"])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(block_required_design_context_verification(Some("observe"), &report).is_ok());
        assert!(block_required_design_context_verification(Some("enforced"), &json!({})).is_ok());
    }

    #[test]
    fn repair_context_enforces_byte_and_estimated_token_limits_independently() {
        assert_eq!(
            validate_repair_context_size(&"x".repeat(16_000)).unwrap(),
            (16_000, 4_000)
        );
        assert!(matches!(
            validate_repair_context_size(&"x".repeat(16_001)),
            Err(ToolError::Terminal(message)) if message.contains("4,000 estimated tokens")
        ));
        assert!(matches!(
            validate_repair_context_size(&"x".repeat(16 * 1024 + 1)),
            Err(ToolError::Terminal(message)) if message.contains("16 KiB")
        ));
    }
}
