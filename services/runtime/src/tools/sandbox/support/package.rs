use super::*;

#[derive(Debug, Clone, Default)]
pub(super) struct DependencyRestoreOutcome {
    pub(super) attempted: bool,
    pub(super) succeeded: bool,
    pub(super) reason: Option<String>,
    pub(super) log_path: Option<String>,
}

pub(super) async fn maybe_restore_project_dependencies(
    workspace: &dyn WorkspaceBackend,
    command: &dyn SandboxCommandBackend,
    ctx: &ToolContext,
    progress: &ProgressSink,
    cwd: &Path,
    package_manager: &str,
) -> Result<DependencyRestoreOutcome, ToolError> {
    let reason = dependency_restore_reason(workspace, ctx, cwd).await?;
    let Some(reason) = reason else {
        return Ok(DependencyRestoreOutcome::default());
    };
    let registry = ctx.npm_registry.clone();
    if is_public_registry(&registry) && ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
        return Err(typed_recoverable(
            "project.build dependency restore requires package.install policy, and public npm registry is denied outside local-e2e policy profile".to_string(),
            "build.missing_dependency",
            json!({
                "registry": registry,
                "policyProfile": format!("{:?}", ctx.policy_profile),
                "suggestedAction": "Use the configured internal registry or local-e2e policy for public registry restores."
            }),
        ));
    }
    progress
        .emit_tool_output(
            "package.install",
            "stdout",
            format!("runtime dependency restore before project.build: {reason}\n"),
        )
        .await;
    let argv = package_install_argv(package_manager, "restore", &[], &registry);
    let output = command
        .run_with_output_events(
            ctx,
            &argv,
            cwd,
            120_000,
            Some(progress.clone()),
            "package.install",
        )
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                typed_recoverable(
                    "project.build dependency restore timed out".to_string(),
                    "build.missing_dependency",
                    json!({
                        "reason": reason,
                        "packageManager": package_manager,
                        "suggestedAction": "Retry project.build or run project.ensure_dependencies after checking package registry connectivity."
                    }),
                )
            } else if error.kind() == io::ErrorKind::Interrupted {
                ToolError::Recoverable(error.to_string())
            } else {
                typed_recoverable(
                    format!("project.build dependency restore failed to start {package_manager}: {error}"),
                    "build.missing_dependency",
                    json!({
                        "reason": reason,
                        "packageManager": package_manager,
                        "suggestedAction": "Run project.ensure_dependencies or verify the package manager is available."
                    }),
                )
            }
        })?;
    let restore_tool_use_id = format!("{}-restore", progress.tool_use_id());
    let log_path =
        write_package_install_log(workspace, ctx, &restore_tool_use_id, &argv, &output).await?;
    let state = json!({
        "needsRestore": !output.success,
        "reason": reason,
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": log_path,
        "packageManager": package_manager,
        "status": output.status,
        "success": output.success,
    });
    write_workspace_json(workspace, ctx, "state/dependency-state.json", &state).await?;
    if !output.success {
        return Err(typed_recoverable(
            format!(
                "project.build dependency restore failed with status {:?}; log: {}",
                output.status, log_path
            ),
            "build.missing_dependency",
            json!({
                "reason": reason,
                "packageManager": package_manager,
                "status": output.status,
                "logPath": log_path,
                "suggestedAction": "Open diagnostics.build_log or rerun project.ensure_dependencies after fixing dependency errors."
            }),
        ));
    }
    Ok(DependencyRestoreOutcome {
        attempted: true,
        succeeded: true,
        reason: Some(reason),
        log_path: Some(log_path),
    })
}

pub(super) fn validate_package_install_like_input(
    input: &Value,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let packages = package_specs_from_input(input);
    if input.get("packages").is_some()
        && !input
            .get("packages")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(|item| item.as_str().is_some()))
    {
        return Err(ValidationError::new(format!(
            "{tool_name} packages must be a string array"
        )));
    }
    let mode = package_install_mode_from_input(input)?;
    match mode.as_str() {
        "add" if packages.is_empty() => {
            return Err(ValidationError::new(format!(
                "{tool_name} mode=add requires a non-empty packages array"
            )));
        }
        "restore" if !packages.is_empty() => {
            return Err(ValidationError::new(format!(
                "{tool_name} mode=restore must omit packages"
            )));
        }
        "add" | "restore" => {}
        _ => unreachable!("package_install_mode_from_input validates mode"),
    }
    if let Some(package_manager) = input.get("packageManager").and_then(Value::as_str) {
        validate_package_manager(package_manager)?;
    }
    Ok(())
}

pub(super) fn package_install_permission(
    tool_name: &str,
    input: &Value,
    ctx: &ToolContext,
) -> PermissionResult {
    let registry = input
        .get("registry")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.npm_registry);
    let packages = package_specs_from_input(input);
    let configured_registry = registry.trim_end_matches('/')
        == ctx.npm_registry.trim_end_matches('/')
        && !registry.contains("registry.npmjs.org");
    let public_registry = (!configured_registry && is_public_registry(registry))
        || packages
            .iter()
            .any(|package| package.starts_with("http://") || package.starts_with("https://"));
    if public_registry {
        if ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
            return deny(
                tool_name,
                "public npm registry is denied outside local-e2e policy profile",
            );
        }
        return allow_with_input(input, "local-e2e public package source allowed");
    }
    for package in &packages {
        if let Some(local_path) = package.strip_prefix("file:") {
            let resolved = normalize_path(&resolve_path(local_path, &default_project_dir(ctx)));
            if let Err(error) = check_context_workspace_path(&resolved, ctx) {
                return deny(tool_name, format!("{error:?}"));
            }
        }
    }
    allow_with_input(input, "internal registry package install allowed")
}

pub(super) async fn run_package_install(
    tool_name: &str,
    workspace: &dyn WorkspaceBackend,
    command: &dyn SandboxCommandBackend,
    input: Value,
    ctx: ToolContext,
    progress: ProgressSink,
) -> Result<Value, ToolError> {
    let packages = package_specs_from_input(&input);
    let mode = package_install_mode_from_input(&input)
        .map_err(|error| ToolError::Recoverable(error.message))?;
    let registry = input
        .get("registry")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.npm_registry)
        .to_string();
    let cwd = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(|cwd| {
            check_context_workspace_path(&resolve_path(cwd, &ctx.workspace_root), &ctx)
                .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
        })
        .transpose()?
        .unwrap_or_else(|| default_project_dir(&ctx));
    ensure_project_package_json(workspace, &ctx, &cwd).await?;

    let package_manager =
        package_manager_from_input_or_project(workspace, &input, &ctx, &cwd).await?;
    let argv = package_install_argv(&package_manager, &mode, &packages, &registry);
    let timeout_ms = input
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(120_000);
    let output = command
        .run_with_output_events(
            &ctx,
            &argv,
            &cwd,
            timeout_ms,
            Some(progress.clone()),
            tool_name,
        )
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                ToolError::typed_recoverable(
                    format!("{tool_name} timed out"),
                    "dependency.install_timeout",
                    json!({
                        "toolName": tool_name,
                        "packageManager": package_manager,
                        "mode": mode,
                        "packages": packages,
                        "registry": registry,
                        "cwd": display_workspace_path(&cwd, &ctx),
                        "timeoutMs": timeout_ms,
                        "suggestedAction": "Retry project.ensure_dependencies with a larger timeoutMs after checking registry connectivity, then rerun project.build or preview.publish.",
                    }),
                )
            } else if error.kind() == io::ErrorKind::Interrupted {
                ToolError::Recoverable(error.to_string())
            } else {
                ToolError::typed_recoverable(
                    format!("{tool_name} failed to start {package_manager}: {error}"),
                    "dependency.install_failed",
                    json!({
                        "toolName": tool_name,
                        "packageManager": package_manager,
                        "mode": mode,
                        "packages": packages,
                        "registry": registry,
                        "cwd": display_workspace_path(&cwd, &ctx),
                        "suggestedAction": "Verify the package manager is available and retry project.ensure_dependencies before building.",
                    }),
                )
            }
        })?;
    let log_path =
        write_package_install_log(workspace, &ctx, progress.tool_use_id(), &argv, &output).await?;
    let dependency_state = json!({
        "needsRestore": !output.success,
        "reason": if output.success { Value::Null } else { json!(format!("{tool_name}_failed")) },
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": log_path.clone(),
        "packageManager": package_manager.clone(),
        "mode": mode.clone(),
        "packages": packages.clone(),
        "status": output.status,
        "success": output.success,
    });
    write_workspace_json(
        workspace,
        &ctx,
        "state/dependency-state.json",
        &dependency_state,
    )
    .await?;
    if !output.success {
        let error_kind = dependency_install_failure_kind(&output.stderr);
        return Err(ToolError::typed_recoverable(
            format!(
                "{tool_name} failed with status {:?}; log: {}",
                output.status, log_path
            ),
            error_kind,
            json!({
                "toolName": tool_name,
                "packageManager": package_manager,
                "mode": mode,
                "packages": packages,
                "registry": registry,
                "cwd": display_workspace_path(&cwd, &ctx),
                "status": output.status,
                "logPath": log_path,
                "stderr": truncate_for_metadata(&output.stderr),
                "suggestedAction": if error_kind == "infrastructure.registry_unavailable" {
                    "Verify the internal npm proxy and its upstream/cache availability, then rerun project.ensure_dependencies."
                } else {
                    "Open the package install log, fix registry or package errors, then rerun project.ensure_dependencies."
                },
            }),
        ));
    }

    Ok(json!({
        "installed": dependency_state["packages"],
        "registry": registry,
        "mode": dependency_state["mode"],
        "packageManager": dependency_state["packageManager"],
        "manager": dependency_state["packageManager"],
        "command": argv,
        "cwd": display_workspace_path(&cwd, &ctx),
        "status": output.status,
        "success": true,
        "logPath": log_path,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

pub(super) fn dependency_install_failure_kind(stderr: &str) -> &'static str {
    let stderr = stderr.to_ascii_lowercase();
    if [
        "eai_again",
        "enotfound",
        "econnrefused",
        "econnreset",
        "etimedout",
        "network timeout",
        "service unavailable",
        "bad gateway",
    ]
    .iter()
    .any(|marker| stderr.contains(marker))
    {
        "infrastructure.registry_unavailable"
    } else {
        "dependency.install_failed"
    }
}

pub(super) async fn dependency_restore_reason(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<Option<String>, ToolError> {
    if read_workspace_json(workspace, ctx, "state/dependency-state.json")
        .await
        .and_then(|state| state.get("needsRestore").and_then(Value::as_bool))
        == Some(true)
    {
        return Ok(Some(
            "source_snapshot_restored_without_node_modules".to_string(),
        ));
    }
    let package_json = workspace
        .read_to_string(ctx, &cwd.join("package.json"))
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    if !package_json_declares_dependencies(&package_json) {
        return Ok(None);
    }
    if workspace
        .path_kind(ctx, &cwd.join("node_modules"))
        .await
        .is_err()
    {
        return Ok(Some("node_modules_missing".to_string()));
    }
    Ok(None)
}

pub(super) fn package_json_declares_dependencies(package_json: &str) -> bool {
    serde_json::from_str::<Value>(package_json).is_ok_and(|value| {
        ["dependencies", "devDependencies", "optionalDependencies"]
            .iter()
            .any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_object)
                    .is_some_and(|dependencies| !dependencies.is_empty())
            })
    })
}

pub(super) async fn write_package_install_log(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    tool_use_id: &str,
    args: &[String],
    output: &SandboxCommandOutput,
) -> Result<String, ToolError> {
    let text = format!(
        "$ {}\n\nstatus: {:?}\n\nstdout:\n{}\n\nstderr:\n{}\n",
        args.join(" "),
        output.status,
        output.stdout,
        output.stderr
    );
    let log_path = format!("outputs/build/package-install-{tool_use_id}.log");
    let path = ctx.workspace_root.join(&log_path);
    workspace
        .write_string(ctx, &path, &text)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    workspace
        .write_string(
            ctx,
            &ctx.workspace_root
                .join("outputs/build/package-install-latest.log"),
            &text,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    Ok(format!("/workspace/{log_path}"))
}

pub(super) fn has_terminal_error(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    ["error:", "failed", "panic", "exception"]
        .iter()
        .any(|needle| lowered.contains(needle))
}

pub(super) fn argv_from_input(input: &Value) -> Result<Vec<String>, ToolError> {
    input
        .get("argv")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .ok_or_else(|| ToolError::Recoverable("shell.run requires argv".to_string()))
}

pub(super) fn package_specs_from_input(input: &Value) -> Vec<String> {
    input
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

pub(super) fn package_install_mode_from_input(input: &Value) -> Result<String, ValidationError> {
    if let Some(mode) = input.get("mode").and_then(Value::as_str) {
        if matches!(mode, "restore" | "add") {
            return Ok(mode.to_string());
        }
        return Err(ValidationError::new(
            "package.install mode must be restore or add",
        ));
    }
    if package_specs_from_input(input).is_empty() {
        Ok("restore".to_string())
    } else {
        Ok("add".to_string())
    }
}

pub(super) fn validate_package_manager(package_manager: &str) -> Result<(), ValidationError> {
    if matches!(package_manager, "npm" | "pnpm") {
        Ok(())
    } else {
        Err(ValidationError::new(
            "package.install packageManager must be npm or pnpm",
        ))
    }
}

pub(super) async fn package_manager_from_input_or_project(
    workspace: &dyn WorkspaceBackend,
    input: &Value,
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<String, ToolError> {
    if let Some(package_manager) = input.get("packageManager").and_then(Value::as_str) {
        validate_package_manager(package_manager)
            .map_err(|error| ToolError::Recoverable(error.message))?;
        return Ok(package_manager.to_string());
    }
    if let Some(package_manager) = ctx
        .run
        .project_state_snapshot
        .as_ref()
        .map(|state| state.package_manager.clone())
    {
        validate_package_manager(&package_manager)
            .map_err(|error| ToolError::Recoverable(error.message))?;
        return Ok(package_manager);
    }
    if workspace
        .path_kind(ctx, &cwd.join("pnpm-lock.yaml"))
        .await
        .is_ok()
    {
        return Ok("pnpm".to_string());
    }
    if workspace
        .path_kind(ctx, &cwd.join("package-lock.json"))
        .await
        .is_ok()
    {
        return Ok("npm".to_string());
    }
    Ok("npm".to_string())
}

pub(super) fn package_install_argv(
    package_manager: &str,
    mode: &str,
    packages: &[String],
    registry: &str,
) -> Vec<String> {
    let mut argv = match (package_manager, mode) {
        ("npm", "restore") | ("npm", "add") => vec![
            "npm".to_string(),
            "install".to_string(),
            "--ignore-scripts".to_string(),
            "--audit=false".to_string(),
            "--fund=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        ("pnpm", "restore") => vec![
            "pnpm".to_string(),
            "install".to_string(),
            "--ignore-scripts".to_string(),
            "--config.audit=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        ("pnpm", "add") => vec![
            "pnpm".to_string(),
            "add".to_string(),
            "--ignore-scripts".to_string(),
            "--config.audit=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        _ => vec![package_manager.to_string()],
    };
    if mode == "add" {
        argv.extend(packages.iter().cloned());
    }
    argv
}

pub(super) fn project_build_argv(package_manager: &str) -> Vec<String> {
    match package_manager {
        "pnpm" => vec!["pnpm".to_string(), "run".to_string(), "build".to_string()],
        _ => vec!["npm".to_string(), "run".to_string(), "build".to_string()],
    }
}

pub(super) fn is_public_registry(registry: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(registry) else {
        return false;
    };
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    if host == "registry.npmjs.org" {
        return true;
    }
    if host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".svc")
        || host.contains(".svc.")
        || host.split('.').any(|label| label == "internal")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
            std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
        })
    {
        return false;
    }
    matches!(url.scheme(), "http" | "https")
}
