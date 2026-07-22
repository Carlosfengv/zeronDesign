use super::*;

const PROJECT_RUNTIME_ATTESTATION_SCHEMA: &str = "project-runtime-attestation@1";

pub(super) async fn project_runtime_attestation(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
) -> Result<ProjectRuntimeAttestation, ToolError> {
    let state = ctx.run.project_state_snapshot.as_ref().ok_or_else(|| {
        typed_recoverable(
            "project runtime attestation requires initialized project state",
            "project.runtime_attestation_missing",
            json!({
                "runId": ctx.run.id,
                "suggestedAction": "Run project.init, then call project.inspect again."
            }),
        )
    })?;
    let template = resolve_project_template_spec(ctx)?;
    let manifest = state.template_manifest_sha256.as_deref().ok_or_else(|| {
        typed_recoverable(
            "project runtime state is missing its exact Template manifest identity",
            "project.template_identity_unverified",
            json!({ "suggestedAction": "Restore the exact historical TemplateSpec identity before continuing." }),
        )
    })?;
    let app_root_relative = project_app_root_relative(ctx);
    let app_root = ctx.workspace_root.join(&app_root_relative);
    let materialized = workspace
        .path_kind(ctx, &app_root.join("package.json"))
        .await
        .is_ok();
    let app_root_materialization_state = if materialized {
        AppRootMaterializationState::Verified
    } else {
        AppRootMaterializationState::Missing
    };

    let expected = ctx
        .run
        .design_context_artifacts
        .get("inputs/template-style-contract.json")
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .unwrap_or_else(|| template.style.render(&template.id, &app_root_relative));
    let expected_identity =
        canonical_json_hash(&crate::style_contract::style_contract_identity(&expected));
    let actual = read_workspace_json(workspace, ctx, "state/style-contract.json").await;
    let style_contract_state = match actual {
        Some(actual) => {
            let actual_identity =
                canonical_json_hash(&crate::style_contract::style_contract_identity(&actual));
            if actual_identity == expected_identity {
                if ctx.run.design_context_manifest.is_some() {
                    StyleContractState::VerifiedDcp {
                        identity: actual_identity,
                    }
                } else {
                    StyleContractState::VerifiedTemplateDefault {
                        identity: actual_identity,
                    }
                }
            } else {
                StyleContractState::Mismatch {
                    expected: expected_identity.clone(),
                    actual: actual_identity,
                }
            }
        }
        None if !materialized => StyleContractState::PendingInitialization,
        None => StyleContractState::Mismatch {
            expected: expected_identity.clone(),
            actual: "missing".to_string(),
        },
    };

    ctx.store
        .set_run_design_context_style_contract_verified(
            &ctx.run.id,
            style_contract_state.is_verified(),
        )
        .await
        .map_err(|error| {
            ToolError::Terminal(format!("record style contract attestation: {error}"))
        })?;

    let attestation_hash = canonical_json_hash(&json!({
        "domain": "project-runtime-attestation-hash@1",
        "runId": ctx.run.id,
        "projectId": ctx.project_id,
        "sourceRevision": state.revision,
        "appRoot": state.app_root,
        "appRootMaterializationState": app_root_materialization_state,
        "templateIdentityState": "verified",
        "templateId": template.id.as_str(),
        "templateVersion": template.version.as_str(),
        "templateManifestSha256": manifest,
        "styleContractState": style_contract_state,
    }));
    Ok(ProjectRuntimeAttestation {
        schema_version: PROJECT_RUNTIME_ATTESTATION_SCHEMA.to_string(),
        run_id: ctx.run.id.clone(),
        project_id: ctx.project_id.clone(),
        source_revision: state.revision,
        app_root: state.app_root.clone(),
        app_root_materialization_state,
        template_identity_state: "verified".to_string(),
        template_id: template.id.as_str().to_string(),
        template_version: template.version.as_str().to_string(),
        template_manifest_sha256: manifest.to_string(),
        style_contract_state,
        attestation_hash,
        verified_at: Utc::now(),
    })
}

pub(super) fn require_materialized_project_attestation(
    attestation: &ProjectRuntimeAttestation,
    operation: &str,
) -> Result<(), ToolError> {
    if attestation.app_root_materialization_state == AppRootMaterializationState::Verified {
        return Ok(());
    }
    Err(typed_recoverable(
        format!("{operation} requires a materialized App Root"),
        "project.app_root_unmaterialized",
        json!({
            "attestationHash": attestation.attestation_hash,
            "appRoot": attestation.app_root,
            "suggestedAction": "Run project.init and inspect the resulting Runtime attestation before retrying."
        }),
    ))
}

pub(super) fn require_verified_style_contract(
    attestation: &ProjectRuntimeAttestation,
    operation: &str,
) -> Result<(), ToolError> {
    if attestation.style_contract_state.is_verified() {
        return Ok(());
    }
    Err(typed_recoverable(
        format!("{operation} requires a verified Runtime-owned Style Contract"),
        "style.contract_attestation_failed",
        json!({
            "attestationHash": attestation.attestation_hash,
            "styleContractState": attestation.style_contract_state,
            "suggestedAction": "Use project.inspect to diagnose the Style Contract identity mismatch."
        }),
    ))
}

pub(super) fn default_project_dir(ctx: &ToolContext) -> PathBuf {
    ctx.workspace_root.join(project_app_root_relative(ctx))
}

pub(super) fn resolve_project_template_spec(
    ctx: &ToolContext,
) -> Result<Arc<TemplateSpec>, ToolError> {
    let registry = BuiltInTemplateRegistry::built_in();
    let Some(state) = ctx.run.project_state_snapshot.as_ref() else {
        return registry.default_template().map_err(|error| {
            typed_recoverable(
                error.to_string(),
                "project.state_missing",
                json!({ "suggestedAction": "Run project.init before using project lifecycle operations." }),
            )
        });
    };
    let id = TemplateId::parse(&state.template_key)
        .map_err(|error| ToolError::Terminal(error.to_string()))?;
    resolve_registered_template_identity(
        &registry,
        &id,
        Some(&state.template_version),
        state.template_manifest_sha256.as_deref(),
    )
}

pub(super) fn resolve_registered_template_identity(
    registry: &BuiltInTemplateRegistry,
    id: &TemplateId,
    version: Option<&str>,
    manifest: Option<&str>,
) -> Result<Arc<TemplateSpec>, ToolError> {
    match (version, manifest) {
        (Some(version), Some(manifest)) => {
            let version = TemplateVersion::parse(version)
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
            let manifest = ManifestHash::parse(manifest)
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
            registry
                .resolve_version(id, &version, &manifest)
                .map_err(|error| {
                    typed_recoverable(
                        error.to_string(),
                        "template.version_incompatible",
                        json!({
                            "templateId": id.as_str(),
                            "templateVersion": version.to_string(),
                            "manifestSha256": manifest.to_string(),
                            "suggestedAction": "Restore the exact historical TemplateSpec required by this project."
                        }),
                    )
                })
        }
        (Some(version), None) if registry.versions(id).len() == 1 => {
            let current = registry.current(id).map_err(|error| {
                typed_recoverable(
                    error.to_string(),
                    "template.legacy_state_ambiguous",
                    json!({ "templateId": id.as_str() }),
                )
            })?;
            if current.version.as_str() == version {
                Ok(current)
            } else {
                Err(legacy_template_identity_error(id, version))
            }
        }
        _ => Err(legacy_template_identity_error(
            id,
            version.unwrap_or("unknown"),
        )),
    }
}

fn legacy_template_identity_error(id: &TemplateId, version: &str) -> ToolError {
    typed_recoverable(
        format!(
            "legacy project template identity is ambiguous: {} {}",
            id, version
        ),
        "template.legacy_state_ambiguous",
        json!({
            "templateId": id.as_str(),
            "templateVersion": version,
            "suggestedAction": "Migrate the project state to include an exact templateVersion and templateManifestSha256 before continuing."
        }),
    )
}

pub(super) fn project_app_root_relative(ctx: &ToolContext) -> PathBuf {
    ctx.run
        .project_state_snapshot
        .as_ref()
        .map(|state| state.app_root.clone())
        .and_then(|path| normalize_workspace_relative_path(&path).ok())
        .unwrap_or_else(|| PathBuf::from("project"))
}

pub(super) async fn package_manager_from_project_state_or_lockfiles(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
) -> String {
    if let Some(package_manager) = ctx
        .run
        .project_state_snapshot
        .as_ref()
        .map(|state| state.package_manager.clone())
    {
        return package_manager;
    }
    if workspace
        .path_kind(ctx, &app_root.join("pnpm-lock.yaml"))
        .await
        .is_ok()
    {
        return "pnpm".to_string();
    }
    "npm".to_string()
}

pub(super) async fn project_key_source_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root_relative: &Path,
    project_state: Option<&Value>,
) -> Vec<Value> {
    let template = project_state
        .and_then(|state| state.get("templateKey").and_then(Value::as_str))
        .and_then(|template| TemplateId::parse(template).ok())
        .and_then(|id| BuiltInTemplateRegistry::built_in().current(&id).ok())
        .or_else(|| BuiltInTemplateRegistry::built_in().default_template().ok());
    let candidates = template
        .map(|spec| spec.inspection_files)
        .unwrap_or_default();
    let mut files = Vec::with_capacity(candidates.len());
    for relative in candidates {
        let path = app_root_relative.join(relative);
        let absolute = ctx.workspace_root.join(&path);
        files.push(json!({
            "path": format!("/workspace/{}", path.to_string_lossy().replace('\\', "/")),
            "exists": workspace.path_kind(ctx, &absolute).await.is_ok(),
        }));
    }
    files
}

pub(super) fn normalize_workspace_relative_path(path: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(ToolError::PermissionDenied(
            "workspace path must be relative".to_string(),
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ToolError::PermissionDenied(
            "workspace path must stay inside the workspace".to_string(),
        ));
    }
    let normalized = normalize_path(path);
    if normalized.as_os_str().is_empty() {
        return Err(ToolError::PermissionDenied(
            "workspace path must stay inside the workspace".to_string(),
        ));
    }
    Ok(normalized)
}

pub(super) fn ensure_not_nested_package_root(
    path: &Path,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    if path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
        return Ok(());
    }
    if !matches!(
        ctx.run.phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair
    ) {
        return Ok(());
    }
    let app_root = default_project_dir(ctx);
    let app_package = app_root.join("package.json");
    if path != app_package && path.starts_with(&app_root) {
        let app_root_display = display_workspace_path(&app_root, ctx);
        let path_display = display_workspace_path(path, ctx);
        return Err(typed_recoverable(
            format!(
                "nested package root denied: write source files under {app_root_display} instead of creating {path_display}"
            ),
            "path.nested_package_root",
            json!({
                "path": path_display,
                "appRoot": app_root_display,
                "suggestedAction": "Use the existing app package.json at the app root, or write source files under the app root without creating another package.json."
            }),
        ));
    }
    Ok(())
}

pub(super) fn ensure_project_mutation_write_path(
    path: &Path,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    check_project_write_path(ctx, path).map_err(|violation| {
        let app_root = display_workspace_path(&violation.app_root, ctx);
        let path = display_workspace_path(&violation.path, ctx);
        typed_recoverable(
            format!(
                "template {} denied writing {path} under {app_root}",
                violation.template_id
            ),
            violation.error_kind,
            json!({
                "path": path,
                "appRoot": app_root,
                "forbiddenPaths": violation.forbidden_roots.iter().map(|root| display_workspace_path(root, ctx)).collect::<Vec<_>>(),
                "suggestedAction": violation.guidance,
            }),
        )
    })
}

pub(super) fn ensure_project_mutation_content(
    path: &Path,
    content: &str,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    check_project_write_content(ctx, path, content).map_err(|violation| {
        typed_recoverable(
            format!(
                "template {} denied static-export-incompatible source in {}",
                violation.template_id,
                display_workspace_path(&violation.path, ctx)
            ),
            violation.error_kind,
            json!({
                "path": display_workspace_path(&violation.path, ctx),
                "forbiddenPatterns": violation.patterns,
                "suggestedAction": violation.guidance,
            }),
        )
    })
}

pub(super) fn display_workspace_path(path: &Path, ctx: &ToolContext) -> String {
    path.strip_prefix(&ctx.workspace_root)
        .map(|path| format!("/workspace/{}", path.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

pub(super) async fn write_workspace_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &str,
    value: &Value,
) -> Result<(), ToolError> {
    let path = ctx.workspace_root.join(path);
    write_workspace_json_path(workspace, ctx, &path, value).await
}

pub(super) async fn write_workspace_json_path(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
    value: &Value,
) -> Result<(), ToolError> {
    workspace
        .write_string(
            ctx,
            path,
            &serde_json::to_string_pretty(value)
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))
}

pub(super) async fn record_chunk_write_health(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    chunk_write: Value,
) -> Result<(), ToolError> {
    let mut health = read_workspace_json(workspace, ctx, "state/run-health.json")
        .await
        .unwrap_or_else(|| json!({ "chunkWrites": [] }));
    let chunk_writes = health
        .as_object_mut()
        .and_then(|object| object.get_mut("chunkWrites"))
        .and_then(Value::as_array_mut);
    match chunk_writes {
        Some(entries) => {
            entries.push(chunk_write);
            if entries.len() > 20 {
                let drain_count = entries.len() - 20;
                entries.drain(0..drain_count);
            }
        }
        None => {
            health["chunkWrites"] = json!([chunk_write]);
        }
    }
    write_workspace_json(workspace, ctx, "state/run-health.json", &health).await
}

pub(super) async fn record_read_path(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
    content: &str,
) -> Result<(), ToolError> {
    record_mutation_lease(workspace, ctx, path, content, "observed_full").await
}

pub(super) async fn advance_mutation_lease(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
    content: &str,
) -> Result<(), ToolError> {
    record_mutation_lease(workspace, ctx, path, content, "self_authored").await
}

async fn record_mutation_lease(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
    content: &str,
    origin: &str,
) -> Result<(), ToolError> {
    let display_path = display_workspace_path(path, ctx);
    let mut tracking = read_workspace_json(workspace, ctx, "state/read-tracking.json")
        .await
        .unwrap_or_else(|| json!({ "schemaVersion": "mutation-lease@1", "paths": [] }));
    if !tracking.is_object() {
        tracking = json!({ "schemaVersion": "mutation-lease@1", "paths": [] });
    }
    tracking["schemaVersion"] = json!("mutation-lease@1");
    let paths = tracking
        .as_object_mut()
        .and_then(|object| object.get_mut("paths"))
        .and_then(Value::as_array_mut);
    let entry = json!({
        "path": display_path,
        "runId": ctx.run.id,
        "readAt": Utc::now(),
        "contentHash": sha256_hex(content.as_bytes()),
        "bytes": content.len(),
        "origin": origin,
    });
    match paths {
        Some(entries) => {
            entries.retain(|value| {
                value.get("path").and_then(Value::as_str)
                    != entry.get("path").and_then(Value::as_str)
                    || value.get("runId").and_then(Value::as_str)
                        != entry.get("runId").and_then(Value::as_str)
            });
            entries.push(entry);
            if entries.len() > 100 {
                let drain_count = entries.len() - 100;
                entries.drain(0..drain_count);
            }
        }
        None => {
            tracking["paths"] = json!([entry]);
        }
    }
    write_workspace_json(workspace, ctx, "state/read-tracking.json", &tracking).await
}

pub(super) async fn authorize_existing_file_mutation(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Result<Option<String>, ToolError> {
    match workspace.path_kind(ctx, path).await {
        Err(_) => return Ok(None),
        Ok(WorkspacePathKind::Dir) => {
            return Err(typed_recoverable(
                "recursive directory mutation requires an explicit Runtime-owned operation",
                "mutation.read_required",
                json!({
                    "path": display_workspace_path(path, ctx),
                    "suggestedAction": "Delete or modify individual observed files instead of recursively mutating a directory."
                }),
            ));
        }
        Ok(WorkspacePathKind::File) => {}
    }
    let content = workspace
        .read_to_string(ctx, path)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let current_hash = sha256_hex(content.as_bytes());
    let lease = read_tracking_entry(workspace, ctx, path)
        .await
        .ok_or_else(|| {
            typed_recoverable(
            "existing file mutation requires reading the target file first to establish a full observation lease",
                "mutation.read_required",
                json!({
                    "path": display_workspace_path(path, ctx),
                    "currentHash": current_hash,
                    "suggestedAction": "Call fs.read on this file, then retry the mutation."
                }),
            )
        })?;
    let lease_hash = lease.get("contentHash").and_then(Value::as_str);
    let origin = lease
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or("observed_full");
    if !matches!(origin, "observed_full" | "self_authored") {
        return Err(typed_recoverable(
            "existing file mutation lease has an unsupported origin",
            "mutation.read_required",
            json!({ "path": display_workspace_path(path, ctx), "origin": origin }),
        ));
    }
    if lease_hash != Some(current_hash.as_str()) {
        return Err(typed_recoverable(
            "file has been modified since fs.read established the current Run mutation lease",
            "mutation.stale_lease",
            json!({
                "path": display_workspace_path(path, ctx),
                "leaseHash": lease_hash,
                "currentHash": current_hash,
                "suggestedAction": "Call fs.read again and re-plan against the current file contents."
            }),
        ));
    }
    Ok(Some(content))
}

pub(super) async fn invalidate_mutation_lease(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), ToolError> {
    let Some(mut tracking) = read_workspace_json(workspace, ctx, "state/read-tracking.json").await
    else {
        return Ok(());
    };
    let display_path = display_workspace_path(path, ctx);
    if let Some(paths) = tracking.get_mut("paths").and_then(Value::as_array_mut) {
        paths.retain(|entry| {
            entry.get("path").and_then(Value::as_str) != Some(display_path.as_str())
                || entry.get("runId").and_then(Value::as_str) != Some(ctx.run.id.as_str())
        });
    }
    write_workspace_json(workspace, ctx, "state/read-tracking.json", &tracking).await
}

pub(super) async fn read_tracking_entry(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Option<Value> {
    let display_path = display_workspace_path(path, ctx);
    read_workspace_json(workspace, ctx, "state/read-tracking.json")
        .await
        .and_then(|tracking| tracking.get("paths").cloned())
        .and_then(|paths| paths.as_array().cloned())
        .and_then(|entries| {
            entries.into_iter().find(|entry| {
                entry.get("path").and_then(Value::as_str) == Some(display_path.as_str())
                    && entry.get("runId").and_then(Value::as_str) == Some(ctx.run.id.as_str())
            })
        })
}

// remote-fs-boundary: allow-begin local-staged-write-cleanup
pub fn cleanup_staged_writes_for_run(workspace_root: &Path, run_id: &str) {
    let root = workspace_root.join("outputs/staged-writes");
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let belongs_to_run = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|manifest| {
                manifest
                    .get("runId")
                    .and_then(Value::as_str)
                    .map(|manifest_run_id| manifest_run_id == run_id)
            })
            .unwrap_or(false);
        if belongs_to_run {
            let _ = fs::remove_dir_all(path);
        }
    }
}
// remote-fs-boundary: allow-end local-staged-write-cleanup

pub async fn cleanup_staged_writes_for_run_backend(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    run_id: &str,
) -> io::Result<()> {
    let root = ctx.workspace_root.join("outputs/staged-writes");
    let entries = match workspace.list_dir(ctx, &root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        if entry.kind != WorkspaceEntryKind::Dir {
            continue;
        }
        let manifest = match workspace
            .read_to_string(ctx, &entry.path.join("manifest.json"))
            .await
        {
            Ok(manifest) => manifest,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let belongs_to_run = serde_json::from_str::<Value>(&manifest)
            .ok()
            .and_then(|manifest| {
                manifest
                    .get("runId")
                    .and_then(Value::as_str)
                    .map(|manifest_run_id| manifest_run_id == run_id)
            })
            .unwrap_or(false);
        if belongs_to_run {
            workspace.remove_dir_all(ctx, &entry.path).await?;
        }
    }
    Ok(())
}

pub(super) async fn read_workspace_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &str,
) -> Option<Value> {
    workspace
        .read_to_string(ctx, &ctx.workspace_root.join(path))
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}
