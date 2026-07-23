use super::*;

pub(super) fn project_build_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(ProjectBuildTool { workspace, command })
}

pub(super) fn project_ensure_dependencies_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(ProjectEnsureDependenciesTool { workspace, command })
}

pub(super) fn package_install_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PackageInstallTool { workspace, command })
}

pub(super) struct ProjectBuildTool {
    pub(super) workspace: Arc<dyn WorkspaceBackend>,
    pub(super) command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ProjectBuildTool {
    fn name(&self) -> &'static str {
        "project.build"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project build allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let runtime_attestation =
            if ctx.run.generation_context_runtime_mode.as_deref() == Some("enabled") {
                let attestation = project_runtime_attestation(&*self.workspace, &ctx).await?;
                require_materialized_project_attestation(&attestation, self.name())?;
                require_verified_style_contract(&attestation, self.name())?;
                Some(attestation)
            } else {
                None
            };
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|cwd| {
                check_context_workspace_path(&resolve_path(cwd, &ctx.workspace_root), &ctx)
                    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
            })
            .transpose()?
            .unwrap_or_else(|| default_project_dir(&ctx));
        ensure_project_package_json(&*self.workspace, &ctx, &cwd).await?;
        validate_project_source_contract(&*self.workspace, &ctx, &cwd).await?;
        let timeout_ms = input
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(180_000);
        let package_manager =
            package_manager_from_input_or_project(&*self.workspace, &json!({}), &ctx, &cwd).await?;
        let dependency_restore = maybe_restore_project_dependencies(
            &*self.workspace,
            &*self.command,
            &ctx,
            &progress,
            &cwd,
            &package_manager,
        )
        .await?;
        verify_project_build_dependencies(
            &*self.workspace,
            &*self.command,
            &ctx,
            &progress,
            &cwd,
            &package_manager,
        )
        .await?;
        let argv = project_build_argv(&package_manager);
        let started_at = Utc::now();
        let output = self.command.run(&ctx, &argv, &cwd, timeout_ms).await;
        let finished_at = Utc::now();
        let (status, output, error_message) = match output {
            Ok(output) => {
                let status = if output.success { "success" } else { "failed" };
                (status, Some(output), None)
            }
            Err(error) => {
                let status = if error.kind() == io::ErrorKind::TimedOut {
                    "timeout"
                } else {
                    "failed"
                };
                (status, None, Some(error.to_string()))
            }
        };
        let log_name = format!("build-{}.log", finished_at.timestamp_millis());
        let log_path = format!("outputs/build/{log_name}");
        let log_text = match &output {
            Some(output) => format!(
                "$ {}\n\ncwd: {}\nstatus: {:?}\nstartedAt: {}\nfinishedAt: {}\n\nstdout:\n{}\n\nstderr:\n{}\n",
                argv.join(" "),
                display_workspace_path(&cwd, &ctx),
                output.status,
                started_at.to_rfc3339(),
                finished_at.to_rfc3339(),
                output.stdout,
                output.stderr
            ),
            None => format!(
                "$ {}\n\ncwd: {}\nstatus: {status}\nstartedAt: {}\nfinishedAt: {}\n\nerror:\n{}\n",
                argv.join(" "),
                display_workspace_path(&cwd, &ctx),
                started_at.to_rfc3339(),
                finished_at.to_rfc3339(),
                error_message.as_deref().unwrap_or("build command failed to start")
            ),
        };
        self.workspace
            .write_string(&ctx, &ctx.workspace_root.join(&log_path), &log_text)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        self.workspace
            .write_string(
                &ctx,
                &ctx.workspace_root.join("outputs/build/build.log"),
                &log_text,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let build_id = format!("build-{}", finished_at.timestamp_millis());
        let source_snapshot_path = format!("outputs/build/source-snapshots/{build_id}");
        snapshot_project_source(&*self.workspace, &ctx, &cwd, &source_snapshot_path).await?;
        let source_snapshot_root = ctx.workspace_root.join(&source_snapshot_path);
        let source_snapshot_files =
            collect_artifact_files(&*self.workspace, &ctx, &source_snapshot_root).await?;
        let source_fingerprint = frozen_source_fingerprint(&source_snapshot_files)?;
        let source_snapshot_uri = FileArtifactPublisher::new(&ctx.runtime_storage_dir)
            .publish_source_snapshot(&ctx.project_id, &build_id, source_snapshot_files)
            .await
            .map_err(|error| {
                ToolError::Terminal(format!(
                    "failed to publish Runtime source snapshot: {error}"
                ))
            })?;
        let source_snapshot_text = format!(
            "buildId: {build_id}\ncwd: {}\nstatus: {status}\nfinishedAt: {}\nlogPath: /workspace/{log_path}\n",
            display_workspace_path(&cwd, &ctx),
            finished_at.to_rfc3339(),
        );
        self.workspace
            .write_string(
                &ctx,
                &ctx.workspace_root.join("outputs/build/source-snapshot.txt"),
                &source_snapshot_text,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;

        let static_output_dir = if status == "success" {
            detect_static_preview_output_dir_backend(&*self.workspace, &ctx, &cwd).await
        } else {
            None
        };
        let static_output_path = static_output_dir
            .as_ref()
            .map(|path| display_workspace_path(path, &ctx));
        let static_output_name = static_output_dir
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string);

        let candidate = if status == "success" {
            let static_output_dir = static_output_dir.as_ref().ok_or_else(|| {
                ToolError::typed_recoverable(
                    "project.build succeeded but produced neither dist/ nor out/".to_string(),
                    "project.static_output_missing",
                    json!({
                        "cwd": display_workspace_path(&cwd, &ctx),
                        "suggestedAction": "Configure the project build to emit a static dist/ or out/ directory, then rerun project.build."
                    }),
                )
            })?;
            let route_contract = resolve_project_template_spec(&ctx)?
                .generation_contract()
                .and_then(|contract| contract.effective_route_contract())
                .map_err(|error| {
                    ToolError::typed_recoverable(
                        format!("project build route contract is invalid: {error}"),
                        "artifact.route_contract_invalid",
                        json!({ "suggestedAction": "Repair the Runtime-owned template route contract before rebuilding." }),
                    )
                })?;
            Some(
                create_candidate_snapshot(
                    &*self.workspace,
                    &ctx,
                    &build_id,
                    static_output_dir,
                    &route_contract,
                )
                .await?,
            )
        } else {
            None
        };

        let latest = json!({
            "buildId": build_id,
            "status": status,
            "success": status == "success",
            "cwd": display_workspace_path(&cwd, &ctx),
            "argv": argv,
            "packageManager": package_manager,
            "dependencyRestoreAttempted": dependency_restore.attempted,
            "dependencyRestoreSucceeded": dependency_restore.succeeded,
            "dependencyRestoreAttempts": dependency_restore.attempts,
            "dependencyRestoreReason": dependency_restore.reason,
            "dependencyRestoreLogPath": dependency_restore.log_path,
            "startedAt": started_at.to_rfc3339(),
            "finishedAt": finished_at.to_rfc3339(),
            "exitCode": output.as_ref().and_then(|output| output.status),
            "logPath": format!("/workspace/{log_path}"),
            "sourceSnapshotUri": source_snapshot_uri,
            "sourceFingerprint": source_fingerprint,
            "staticOutputPath": static_output_path,
            "staticOutputName": static_output_name,
            "candidateOutputPath": candidate.as_ref().map(|candidate| candidate.output_path.clone()),
            "candidateManifestPath": candidate.as_ref().map(|candidate| candidate.manifest_path.clone()),
            "candidateManifestHash": candidate.as_ref().map(|candidate| candidate.manifest_hash.clone()),
            "artifactRouteManifestPath": candidate.as_ref().map(|candidate| candidate.route_manifest_path.clone()),
            "artifactRouteManifestHash": candidate.as_ref().map(|candidate| candidate.route_manifest_hash.clone()),
            "error": error_message,
        });
        write_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json", &latest).await?;
        if status != "success" {
            let classification =
                classify_project_build_failure(status, output.as_ref(), error_message.as_deref());
            return Err(ToolError::typed_recoverable(
                format!("project.build {status}; log: /workspace/{log_path}"),
                classification.error_kind,
                json!({
                    "logPath": format!("/workspace/{log_path}"),
                    "status": status,
                    "exitCode": output.as_ref().and_then(|output| output.status),
                    "sourceSnapshotUri": source_snapshot_uri,
                    "sourceFingerprint": source_fingerprint,
                    "stderr": output.as_ref().map(|output| truncate_for_metadata(&output.stderr)),
                    "error": error_message,
                    "suggestedAction": classification.suggested_action,
                }),
            ));
        }
        ctx.store
            .update_run_status(&ctx.run.id, AgentRunStatus::Validating)
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let mut latest = latest;
        latest["runtimeAttestation"] = json!(runtime_attestation);
        Ok(ToolResult::ok(latest))
    }
}

#[derive(Debug, Clone)]
struct CandidateSnapshot {
    output_path: String,
    manifest_path: String,
    manifest_hash: String,
    route_manifest_path: String,
    route_manifest_hash: String,
}

async fn create_candidate_snapshot(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    build_id: &str,
    static_output_dir: &Path,
    route_contract: &crate::artifact_routes::ArtifactRouteContract,
) -> Result<CandidateSnapshot, ToolError> {
    let candidates_root = ctx.workspace_root.join("outputs/candidates");
    let staging_root = candidates_root.join(format!(".staging-{build_id}"));
    let candidate_root = candidates_root.join(build_id);
    let _ = workspace.remove_dir_all(ctx, &staging_root).await;
    workspace
        .copy_dir_all(ctx, static_output_dir, &staging_root, &[])
        .await
        .map_err(|error| {
            ToolError::typed_recoverable(
                format!("failed to create candidate snapshot: {error}"),
                "project.candidate_snapshot_failed",
                json!({ "buildId": build_id }),
            )
        })?;

    let mut files = Vec::new();
    let mut route_files = Vec::new();
    let mut stack = vec![staging_root.clone()];
    while let Some(directory) = stack.pop() {
        let entries = workspace
            .list_dir(ctx, &directory)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        for entry in entries {
            match entry.kind {
                WorkspaceEntryKind::Dir => stack.push(entry.path),
                WorkspaceEntryKind::File => {
                    let bytes = workspace
                        .read_bytes(ctx, &entry.path)
                        .await
                        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
                    let relative = entry
                        .path
                        .strip_prefix(&staging_root)
                        .map_err(|error| ToolError::Terminal(error.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let sha256 = sha256_hex(&bytes);
                    route_files.push(ArtifactRouteFile {
                        path: relative.clone(),
                        sha256: sha256.clone(),
                    });
                    files.push(json!({
                        "path": relative,
                        "bytes": bytes.len(),
                        "sha256": sha256,
                    }));
                }
            }
        }
    }
    files.sort_by(|left, right| {
        left.get("path")
            .and_then(Value::as_str)
            .cmp(&right.get("path").and_then(Value::as_str))
    });
    let route_manifest = match ArtifactRouteManifest::build(build_id, route_contract, route_files) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = workspace.remove_dir_all(ctx, &staging_root).await;
            return Err(ToolError::typed_recoverable(
                error.to_string(),
                error.error_kind,
                json!({
                    "route": error.route,
                    "files": error.files,
                    "suggestedAction": "Ensure the static export contains exactly one artifact file for every contracted route."
                }),
            ));
        }
    };
    let route_manifest_text = serde_json::to_string_pretty(&route_manifest)
        .map_err(|error| ToolError::Terminal(error.to_string()))?;
    let route_manifest_hash = sha256_hex(route_manifest_text.as_bytes());
    workspace
        .write_string(
            ctx,
            &staging_root.join(ARTIFACT_ROUTE_MANIFEST_FILE),
            &route_manifest_text,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    files.push(json!({
        "path": ARTIFACT_ROUTE_MANIFEST_FILE,
        "bytes": route_manifest_text.len(),
        "sha256": sha256_hex(route_manifest_text.as_bytes()),
    }));
    files.sort_by(|left, right| {
        left.get("path")
            .and_then(Value::as_str)
            .cmp(&right.get("path").and_then(Value::as_str))
    });
    let manifest = json!({
        "schemaVersion": "candidate-manifest@1",
        "buildId": build_id,
        "artifactRouteManifestPath": ARTIFACT_ROUTE_MANIFEST_FILE,
        "artifactRouteManifestHash": route_manifest_hash.clone(),
        "files": files,
    });
    let manifest_text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| ToolError::Terminal(error.to_string()))?;
    let manifest_hash = sha256_hex(manifest_text.as_bytes());
    workspace
        .write_string(
            ctx,
            &staging_root.join(".anydesign-candidate-manifest.json"),
            &manifest_text,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    workspace
        .rename(ctx, &staging_root, &candidate_root)
        .await
        .map_err(|error| {
            ToolError::typed_recoverable(
                format!("failed to freeze candidate snapshot: {error}"),
                "project.candidate_snapshot_failed",
                json!({ "buildId": build_id }),
            )
        })?;

    Ok(CandidateSnapshot {
        output_path: format!("/workspace/outputs/candidates/{build_id}"),
        manifest_path: format!(
            "/workspace/outputs/candidates/{build_id}/.anydesign-candidate-manifest.json"
        ),
        manifest_hash,
        route_manifest_path: format!(
            "/workspace/outputs/candidates/{build_id}/{ARTIFACT_ROUTE_MANIFEST_FILE}"
        ),
        route_manifest_hash,
    })
}

struct BuildFailureClassification {
    error_kind: &'static str,
    suggested_action: &'static str,
}

fn classify_project_build_failure(
    status: &str,
    output: Option<&SandboxCommandOutput>,
    error_message: Option<&str>,
) -> BuildFailureClassification {
    let stderr = output.map(|output| output.stderr.as_str()).unwrap_or("");
    let lowered = format!("{} {}", stderr, error_message.unwrap_or("")).to_lowercase();
    if output.and_then(|output| output.status) == Some(127)
        || lowered.contains("command not found")
        || lowered.contains("module not found")
        || lowered.contains("cannot find module")
    {
        return BuildFailureClassification {
            error_kind: "build.missing_dependency",
            suggested_action: "Run project.ensure_dependencies with mode=restore, verify dependency installation completed, then rerun project.build or preview.publish.",
        };
    }
    if status == "timeout" {
        return BuildFailureClassification {
            error_kind: "build.timeout",
            suggested_action: "Increase timeoutMs if the build is legitimately long, or inspect diagnostics.build_log before retrying project.build.",
        };
    }
    BuildFailureClassification {
        error_kind: "build.failed",
        suggested_action:
            "Open diagnostics.build_log, fix the source or dependency error, then rerun project.build or preview.publish.",
    }
}

pub(super) fn truncate_for_metadata(text: &str) -> String {
    const LIMIT: usize = 2048;
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(LIMIT).collect::<String>())
    }
}

struct ProjectEnsureDependenciesTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ProjectEnsureDependenciesTool {
    fn name(&self) -> &'static str {
        "project.ensure_dependencies"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "mode": string_schema("Install mode: restore or add"),
                "packages": { "type": "array", "items": { "type": "string" } },
                "cwd": string_schema("Workspace cwd"),
                "packageManager": string_schema("Package manager: npm or pnpm"),
                "registry": string_schema("Internal registry URL"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        validate_package_install_like_input(&input, self.name())?;
        validate_template_dependency_catalog(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        package_install_permission(self.name(), input, ctx)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let runtime_attestation =
            if ctx.run.generation_context_runtime_mode.as_deref() == Some("enabled") {
                let attestation = project_runtime_attestation(&*self.workspace, &ctx).await?;
                require_materialized_project_attestation(&attestation, self.name())?;
                Some(attestation)
            } else {
                None
            };
        let result = run_package_install(
            self.name(),
            &*self.workspace,
            &*self.command,
            input,
            ctx,
            progress,
        )
        .await?;
        Ok(ToolResult::ok(json!({
            "ensured": true,
            "dependencyState": result,
            "runtimeAttestation": runtime_attestation,
        })))
    }
}

struct PackageInstallTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PackageInstallTool {
    fn name(&self) -> &'static str {
        "package.install"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "packages": { "type": "array", "items": { "type": "string" } },
                "mode": string_schema("Install mode: restore or add"),
                "packageManager": string_schema("Package manager: npm or pnpm"),
                "registry": string_schema("Internal registry URL"),
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        validate_package_install_like_input(&input, self.name())?;
        validate_template_dependency_catalog(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        package_install_permission(self.name(), input, ctx)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let result = run_package_install(
            self.name(),
            &*self.workspace,
            &*self.command,
            input,
            ctx,
            progress,
        )
        .await?;
        Ok(ToolResult::ok(result))
    }
}
