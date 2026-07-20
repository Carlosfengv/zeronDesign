use super::*;

pub(super) async fn snapshot_project_source(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    source_root: &Path,
    snapshot_relative: &str,
) -> Result<(), ToolError> {
    let snapshot_root = ctx.workspace_root.join(snapshot_relative);
    let _ = workspace.remove_dir_all(ctx, &snapshot_root).await;
    let skip_dir_names = source_snapshot_skip_dir_names();
    workspace
        .copy_dir_all(ctx, source_root, &snapshot_root, &skip_dir_names)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let manifest = json!({
        "sourceRoot": display_workspace_path(source_root, ctx),
        "snapshotRoot": format!("/workspace/{snapshot_relative}"),
        "createdAt": Utc::now().to_rfc3339(),
    });
    write_workspace_json(
        workspace,
        ctx,
        &format!("{snapshot_relative}/.snapshot.json"),
        &manifest,
    )
    .await?;
    Ok(())
}

pub(super) async fn project_source_fingerprint(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    source_root: &Path,
) -> Result<String, ToolError> {
    let skip_dir_names = source_snapshot_skip_dir_names();
    let mut pending = vec![source_root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = workspace
            .list_dir(ctx, &directory)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        for entry in entries {
            match entry.kind {
                WorkspaceEntryKind::Dir => {
                    if !skip_dir_names.contains(&entry.name) {
                        pending.push(entry.path);
                    }
                }
                WorkspaceEntryKind::File => {
                    let relative_path = entry
                        .path
                        .strip_prefix(source_root)
                        .unwrap_or(&entry.path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let content = workspace
                        .read_to_string(ctx, &entry.path)
                        .await
                        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
                    files.push((relative_path, content));
                }
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (path, content) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((content.len() as u64).to_be_bytes());
        digest.update(content.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn frozen_source_fingerprint(files: &[ArtifactFile]) -> Result<String, ToolError> {
    crate::artifact_publisher::source_snapshot_fingerprint(files)
        .map_err(|error| ToolError::Terminal(error.to_string()))
}

pub(super) fn source_snapshot_skip_dir_names() -> Vec<String> {
    ["node_modules", "dist", "out", ".next", ".source"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(super) async fn promotion_gate_report_from_workspace(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    screenshot_id: Option<&str>,
) -> PromotionGateReport {
    let latest_build = read_workspace_json(workspace, ctx, "outputs/build/latest.json").await;
    let preview = read_workspace_json(workspace, ctx, "state/preview.json").await;
    let screenshot = match screenshot_id {
        Some(id) => {
            read_workspace_json(workspace, ctx, &format!("outputs/screenshots/{id}.json")).await
        }
        None => None,
    };

    PromotionGateReport {
        build_log_has_terminal_error: latest_build
            .as_ref()
            .map(|build| {
                build.get("status").and_then(Value::as_str) != Some("success")
                    || build.get("success").and_then(Value::as_bool) != Some(true)
            })
            .unwrap_or(true),
        preview_accessible: preview
            .as_ref()
            .and_then(|value| value.get("accessible"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_blank: screenshot
            .as_ref()
            .and_then(|value| value.get("blank"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_available: screenshot.is_some(),
        blocking_findings: 0,
    }
}

pub(super) async fn ensure_project_package_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
) -> Result<(), ToolError> {
    let package_json = project_dir.join("package.json");
    if workspace.path_kind(ctx, &package_json).await.is_ok() {
        return Ok(());
    }
    workspace
        .write_string(
            ctx,
            &package_json,
            &serde_json::to_string_pretty(&json!({
                "type": "module",
                "private": true,
                "dependencies": {}
            }))
            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))
}

pub(super) async fn validate_project_source_contract(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
) -> Result<(), ToolError> {
    let template = if ctx.run.project_state_snapshot.is_some() {
        resolve_project_template_spec(ctx)?
    } else if let Some(id) = read_workspace_json(workspace, ctx, "state/project.json")
        .await
        .and_then(|state| {
            state
                .get("templateKey")
                .or_else(|| state.get("template"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    {
        let id = TemplateId::parse(&id).map_err(|error| ToolError::Terminal(error.to_string()))?;
        BuiltInTemplateRegistry::built_in()
            .current(&id)
            .map_err(|error| ToolError::Terminal(error.to_string()))?
    } else {
        resolve_project_template_spec(ctx)?
    };
    let mut snapshot = SourceSnapshot::default();
    for relative in template.operations.source_contract_paths() {
        let text = match workspace
            .read_to_string(ctx, &project_dir.join(relative))
            .await
        {
            Ok(text) => Some(text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(ToolError::Recoverable(error.to_string())),
        };
        snapshot.files.insert((*relative).to_string(), text);
    }
    for relative in template.operations.source_contract_roots() {
        match workspace.path_kind(ctx, &project_dir.join(relative)).await {
            Ok(_) => {
                snapshot.present_roots.insert((*relative).to_string());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ToolError::Recoverable(error.to_string())),
        }
    }
    let report = template.operations.validate_source(&snapshot);
    if report.is_valid() {
        return Ok(());
    }
    Err(typed_recoverable(
        format!("{}: {}", report.summary, report.violations.join(", ")),
        report.error_kind,
        json!({
            "missing": report.violations,
            "templateId": template.id.as_str(),
            "appRoot": display_workspace_path(project_dir, ctx),
            "suggestedAction": report.guidance
        }),
    ))
}

pub(super) async fn write_project_template_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
    template_spec: &TemplateSpec,
) -> Result<(), ToolError> {
    for file in template_spec.files {
        let path = app_root.join(file.path);
        let should_write = match file.write_mode {
            TemplateWriteMode::ReplaceOnInit => true,
            TemplateWriteMode::CreateOnly => match workspace.path_kind(ctx, &path).await {
                Ok(_) => false,
                Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                Err(error) => return Err(ToolError::Recoverable(error.to_string())),
            },
            TemplateWriteMode::PreserveIfPresent => {
                matches!(workspace.path_kind(ctx, &path).await, Err(error) if error.kind() == io::ErrorKind::NotFound)
            }
        };
        if should_write {
            workspace
                .write_string(ctx, &path, file.content_for_write())
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        }
    }
    Ok(())
}

pub(super) async fn apply_design_profile_initial_tokens(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    contract: &Value,
) -> Result<Vec<Value>, ToolError> {
    let Some(profile) = read_workspace_json(workspace, ctx, "inputs/design-profile.json").await
    else {
        return Ok(Vec::new());
    };
    let mut requested = Vec::new();
    for field in ["runtimeTokenMapping", "extendedTokenMapping"] {
        if let Some(tokens) = profile.get(field).and_then(Value::as_object) {
            requested.extend(tokens.iter());
        }
    }
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let token_file = contract
        .get("tokenFile")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Recoverable("style contract missing tokenFile".to_string()))?;
    let token_path = resolve_path(token_file, &ctx.workspace_root);
    if !token_path.starts_with(&ctx.workspace_root) {
        return Err(ToolError::Recoverable(format!(
            "design profile token initialization rejected tokenFile outside workspace: {token_file}"
        )));
    }
    let contract_tokens = contract
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| ToolError::Recoverable("style contract missing tokens map".to_string()))?;
    let mut content = workspace
        .read_to_string(ctx, &token_path)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let mut changes = Vec::new();
    for (token_name, value) in requested {
        let Some(css_variable) = contract_tokens.get(token_name).and_then(Value::as_str) else {
            continue;
        };
        let Some(new_value) = value.as_str() else {
            return Err(ToolError::Recoverable(format!(
                "design profile runtimeTokenMapping.{token_name} must be a string"
            )));
        };
        validate_style_token_value(new_value).map_err(|message| {
            ToolError::Recoverable(format!(
                "design profile runtimeTokenMapping.{token_name} {message}"
            ))
        })?;
        let (updated, old_value) =
            replace_css_variable_value(&content, css_variable, new_value, ctx, &token_path)?;
        content = updated;
        changes.push(json!({
            "token": token_name,
            "cssVariable": css_variable,
            "before": old_value,
            "after": new_value,
            "reason": "initial_build",
        }));
    }
    if !changes.is_empty() {
        workspace
            .write_string(ctx, &token_path, &content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(workspace, ctx, &token_path, &content).await?;
    }
    Ok(changes)
}

pub(super) async fn cleanup_conflicting_template_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
    new_template: &TemplateSpec,
) -> Result<(), ToolError> {
    let Some(state) = read_workspace_json(workspace, ctx, "state/project.json").await else {
        return Ok(());
    };
    let Some(old_template_id) = state
        .get("templateKey")
        .or_else(|| state.get("template"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let old_template_id = TemplateId::parse(old_template_id).map_err(|error| {
        typed_recoverable(
            error.to_string(),
            "template.legacy_state_ambiguous",
            json!({
                "suggestedAction": "Repair the authoritative project template identity before switching templates."
            }),
        )
    })?;
    if old_template_id == new_template.id {
        return Ok(());
    }
    let registry = BuiltInTemplateRegistry::built_in();
    let old_template = resolve_registered_template_identity(
        &registry,
        &old_template_id,
        state.get("templateVersion").and_then(Value::as_str),
        state.get("templateManifestSha256").and_then(Value::as_str),
    )?;
    let new_paths = new_template
        .files
        .iter()
        .map(|file| file.path)
        .collect::<std::collections::HashSet<_>>();
    for file in old_template
        .files
        .iter()
        .filter(|file| !new_paths.contains(file.path))
    {
        remove_workspace_path_if_exists(workspace, ctx, &app_root.join(file.path)).await?;
    }
    Ok(())
}

pub(super) async fn remove_workspace_path_if_exists(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), ToolError> {
    match workspace.path_kind(ctx, path).await {
        Ok(WorkspacePathKind::Dir) => workspace.remove_dir_all(ctx, path).await.map_err(|error| {
            ToolError::Recoverable(format!(
                "failed to remove stale template directory {}: {error}",
                display_workspace_path(path, ctx)
            ))
        }),
        Ok(WorkspacePathKind::File) => workspace.remove_file(ctx, path).await.map_err(|error| {
            ToolError::Recoverable(format!(
                "failed to remove stale template file {}: {error}",
                display_workspace_path(path, ctx)
            ))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ToolError::Recoverable(format!(
            "failed to inspect stale template path {}: {error}",
            display_workspace_path(path, ctx)
        ))),
    }
}
