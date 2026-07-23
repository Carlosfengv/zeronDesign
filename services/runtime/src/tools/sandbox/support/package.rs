use super::*;

#[derive(Debug, Clone, Default)]
pub(super) struct DependencyRestoreOutcome {
    pub(super) attempted: bool,
    pub(super) succeeded: bool,
    pub(super) attempts: u32,
    pub(super) reason: Option<String>,
    pub(super) log_path: Option<String>,
}

const MAX_AUTOMATIC_DEPENDENCY_RESTORE_ATTEMPTS: u32 = 2;

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
    prepare_dependency_install_workspace(workspace, ctx, cwd).await?;
    let argv = package_install_argv(package_manager, "restore", &[], &registry);
    let mut attempts = 0;
    let (output, log_path) = loop {
        attempts += 1;
        let output = match command
            .run_with_output_events(
                ctx,
                &argv,
                cwd,
                120_000,
                Some(progress.clone()),
                "package.install",
            )
            .await
        {
            Ok(output) => output,
            Err(error) => {
                let failure_kind = if error.kind() == io::ErrorKind::TimedOut {
                    "dependency.install_timeout"
                } else if error.kind() == io::ErrorKind::Interrupted {
                    "dependency.install_interrupted"
                } else {
                    "dependency.install_failed"
                };
                mark_dependency_install_incomplete(
                    workspace,
                    ctx,
                    cwd,
                    package_manager,
                    "restore",
                    &[],
                    failure_kind,
                )
                .await?;
                if error.kind() == io::ErrorKind::TimedOut {
                    return Err(typed_recoverable(
                        "project.build dependency restore timed out".to_string(),
                        "build.missing_dependency",
                        json!({
                            "reason": reason,
                            "packageManager": package_manager,
                            "attempts": attempts,
                            "suggestedAction": "Retry project.build or run project.ensure_dependencies after checking package registry connectivity."
                        }),
                    ));
                } else if error.kind() == io::ErrorKind::Interrupted {
                    return Err(ToolError::Recoverable(error.to_string()));
                } else {
                    return Err(typed_recoverable(
                        format!("project.build dependency restore failed to start {package_manager}: {error}"),
                        "build.missing_dependency",
                        json!({
                            "reason": reason,
                            "packageManager": package_manager,
                            "attempts": attempts,
                            "suggestedAction": "Run project.ensure_dependencies or verify the package manager is available."
                        }),
                    ));
                }
            }
        };
        let restore_tool_use_id = format!("{}-restore-attempt-{attempts}", progress.tool_use_id());
        let log_path =
            write_package_install_log(workspace, ctx, &restore_tool_use_id, &argv, &output).await?;
        if !should_retry_automatic_dependency_restore(output.success, &output.stderr, attempts) {
            break (output, log_path);
        }
        mark_dependency_install_incomplete(
            workspace,
            ctx,
            cwd,
            package_manager,
            "restore",
            &[],
            dependency_install_failure_kind(&output.stderr),
        )
        .await?;
        prepare_dependency_install_workspace(workspace, ctx, cwd).await?;
        progress
            .emit_tool_output(
                "package.install",
                "stderr",
                format!(
                    "transient registry failure during dependency restore; retrying ({}/{})\n",
                    attempts + 1,
                    MAX_AUTOMATIC_DEPENDENCY_RESTORE_ATTEMPTS
                ),
            )
            .await;
        time::sleep(Duration::from_millis(500)).await;
    };
    let state = json!({
        "needsRestore": !output.success,
        "reason": reason,
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": log_path,
        "packageManager": package_manager,
        "attempts": attempts,
        "status": output.status,
        "success": output.success,
        "cleanupNodeModules": !output.success,
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
                "attempts": attempts,
                "status": output.status,
                "logPath": log_path,
                "failureKind": dependency_install_failure_kind(&output.stderr),
                "suggestedAction": if dependency_install_failure_kind(&output.stderr) == "infrastructure.registry_unavailable" {
                    "Verify the internal npm proxy and its upstream/cache availability, then rerun project.ensure_dependencies."
                } else {
                    "Open diagnostics.build_log or rerun project.ensure_dependencies after fixing dependency errors."
                }
            }),
        ));
    }
    Ok(DependencyRestoreOutcome {
        attempted: true,
        succeeded: true,
        attempts,
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

const NEXT_APP_VISUAL_CATALOG: &[&str] = &[
    "@base-ui/react",
    "class-variance-authority",
    "clsx",
    "date-fns",
    "embla-carousel-react",
    "lucide-react",
    "motion",
    "recharts",
    "tailwind-merge",
];

pub(super) fn validate_template_dependency_catalog(
    input: &Value,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<(), ValidationError> {
    if package_install_mode_from_input(input)? != "add" {
        return Ok(());
    }
    let Some(state) = ctx.run.project_state_snapshot.as_ref() else {
        return Ok(());
    };
    if state.template_key != "next-app" {
        return Ok(());
    }
    let denied = package_specs_from_input(input)
        .into_iter()
        .filter(|spec| {
            package_name_from_spec(spec).is_none_or(|name| !NEXT_APP_VISUAL_CATALOG.contains(&name))
        })
        .collect::<Vec<_>>();
    if denied.is_empty() {
        return Ok(());
    }
    Err(ValidationError::with_kind(
        format!(
            "{tool_name} rejected packages outside the next-app visual catalog: {}",
            denied.join(", ")
        ),
        "dependency.not_in_catalog",
    )
    .with_metadata(json!({
        "template": "next-app",
        "dependencyPolicyVersion": "runtime-dependency-policy@1",
        "deniedPackages": denied,
        "suggestedAction": "Use an already seeded dependency or choose a package from the approved visual catalog."
    })))
}

fn package_name_from_spec(spec: &str) -> Option<&str> {
    let spec = spec.trim();
    if spec.is_empty()
        || spec.starts_with("git")
        || spec.starts_with("http:")
        || spec.starts_with("https:")
        || spec.starts_with("file:")
    {
        return None;
    }
    if spec.starts_with('@') {
        let slash = spec.find('/')?;
        let version = spec[slash + 1..].find('@').map(|offset| slash + 1 + offset);
        return Some(version.map_or(spec, |index| &spec[..index]));
    }
    Some(spec.split_once('@').map_or(spec, |(name, _)| name))
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
    if mode == "add" {
        preview_dev::validate_dev_mutation(&ctx)?;
    }
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
    prepare_dependency_install_workspace(workspace, &ctx, &cwd).await?;
    let argv = package_install_argv(&package_manager, &mode, &packages, &registry);
    let timeout_ms = input
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(120_000);
    let output = match command
        .run_with_output_events(
            &ctx,
            &argv,
            &cwd,
            timeout_ms,
            Some(progress.clone()),
            tool_name,
        )
        .await
    {
        Ok(output) => output,
        Err(error) => {
            mark_dependency_install_incomplete(
                workspace,
                &ctx,
                &cwd,
                &package_manager,
                &mode,
                &packages,
                if error.kind() == io::ErrorKind::TimedOut {
                    "dependency.install_timeout"
                } else if error.kind() == io::ErrorKind::Interrupted {
                    "dependency.install_interrupted"
                } else {
                    "dependency.install_failed"
                },
            )
            .await?;
            if error.kind() == io::ErrorKind::TimedOut {
                return Err(ToolError::typed_recoverable(
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
                ));
            } else if error.kind() == io::ErrorKind::Interrupted {
                return Err(ToolError::Recoverable(error.to_string()));
            } else {
                return Err(ToolError::typed_recoverable(
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
                ));
            }
        }
    };
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
        "cleanupNodeModules": !output.success,
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
    let draft_preview = if mode == "add" {
        preview_dev::record_dev_mutation(workspace, &ctx).await
    } else {
        None
    };

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
        "draftPreview": draft_preview,
    }))
}

async fn prepare_dependency_install_workspace(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<bool, ToolError> {
    let Some(mut state) = read_workspace_json(workspace, ctx, "state/dependency-state.json").await
    else {
        return Ok(false);
    };
    let cleanup_required = state.get("needsRestore").and_then(Value::as_bool) == Some(true)
        && state.get("cleanupNodeModules").and_then(Value::as_bool) == Some(true);
    if !cleanup_required {
        return Ok(false);
    }

    let node_modules = cwd.join("node_modules");
    match workspace.path_kind(ctx, &node_modules).await {
        Ok(WorkspacePathKind::Dir) => workspace
            .remove_dir_all(ctx, &node_modules)
            .await
            .map_err(|error| {
                ToolError::typed_recoverable(
                    format!(
                        "failed to clean incomplete dependency tree before restore: {error}"
                    ),
                    "dependency.cleanup_failed",
                    json!({
                        "cwd": display_workspace_path(cwd, ctx),
                        "path": display_workspace_path(&node_modules, ctx),
                        "suggestedAction": "Repair the Sandbox workspace or start a fresh Build before retrying dependency restore."
                    }),
                )
            })?,
        Ok(WorkspacePathKind::File) => workspace
            .remove_file(ctx, &node_modules)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ToolError::typed_recoverable(
                format!("failed to inspect dependency tree before restore: {error}"),
                "dependency.cleanup_failed",
                json!({
                    "cwd": display_workspace_path(cwd, ctx),
                    "path": display_workspace_path(&node_modules, ctx),
                }),
            ));
        }
    }

    if let Some(object) = state.as_object_mut() {
        object.insert("cleanupNodeModules".to_string(), Value::Bool(false));
        object.insert(
            "lastCleanupAt".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );
    }
    write_workspace_json(workspace, ctx, "state/dependency-state.json", &state).await?;
    Ok(true)
}

async fn mark_dependency_install_incomplete(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    cwd: &Path,
    package_manager: &str,
    mode: &str,
    packages: &[String],
    failure_kind: &str,
) -> Result<(), ToolError> {
    let state = json!({
        "needsRestore": true,
        "reason": "previous_dependency_install_incomplete",
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": Value::Null,
        "packageManager": package_manager,
        "mode": mode,
        "packages": packages,
        "status": Value::Null,
        "success": false,
        "failureKind": failure_kind,
        "cleanupNodeModules": true,
        "cwd": display_workspace_path(cwd, ctx),
    });
    write_workspace_json(workspace, ctx, "state/dependency-state.json", &state)
        .await
        .map_err(|error| {
            ToolError::typed_recoverable(
                format!(
                    "dependency install failed and its cleanup state could not be recorded: {error:?}"
                ),
                "dependency.install_state_failed",
                json!({
                    "failureKind": failure_kind,
                    "cwd": display_workspace_path(cwd, ctx),
                    "suggestedAction": "Start a fresh Build workspace before retrying dependency installation."
                }),
            )
        })
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

fn should_retry_automatic_dependency_restore(success: bool, stderr: &str, attempts: u32) -> bool {
    !success
        && attempts < MAX_AUTOMATIC_DEPENDENCY_RESTORE_ATTEMPTS
        && dependency_install_failure_kind(stderr) == "infrastructure.registry_unavailable"
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

pub(super) fn package_json_declares_next(package_json: &str) -> bool {
    serde_json::from_str::<Value>(package_json).is_ok_and(|value| {
        ["dependencies", "devDependencies", "optionalDependencies"]
            .iter()
            .any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_object)
                    .is_some_and(|dependencies| dependencies.contains_key("next"))
            })
    })
}

pub(super) async fn verify_project_build_dependencies(
    workspace: &dyn WorkspaceBackend,
    command: &dyn SandboxCommandBackend,
    ctx: &ToolContext,
    progress: &ProgressSink,
    cwd: &Path,
    package_manager: &str,
) -> Result<(), ToolError> {
    let package_json = workspace
        .read_to_string(ctx, &cwd.join("package.json"))
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    if !package_json_declares_next(&package_json) {
        return Ok(());
    }

    let script = r#"const libc = process.report?.getReport?.().header?.glibcVersionRuntime ? "gnu" : "musl"; const pkg = `@next/swc-linux-${process.arch}-${libc}`; require(pkg); process.stdout.write(JSON.stringify({ package: pkg, arch: process.arch, libc }));"#;
    let argv = vec!["node".to_string(), "-e".to_string(), script.to_string()];
    let output = match command.run(ctx, &argv, cwd, 15_000).await {
        Ok(output) => output,
        Err(error) => {
            mark_dependency_install_incomplete(
                workspace,
                ctx,
                cwd,
                package_manager,
                "restore",
                &[],
                "environment.next_swc_unavailable",
            )
            .await?;
            return Err(ToolError::typed_recoverable(
                format!("Next.js native compiler preflight failed to start: {error}"),
                "environment.next_swc_unavailable",
                json!({
                    "cwd": display_workspace_path(cwd, ctx),
                    "packageManager": package_manager,
                    "suggestedAction": "Retry project.build; Runtime will discard the incomplete dependency tree and restore dependencies from a clean state."
                }),
            ));
        }
    };
    if output.success {
        progress
            .emit_tool_output(
                "project.build",
                "stdout",
                format!(
                    "Next.js native compiler preflight passed: {}\n",
                    output.stdout
                ),
            )
            .await;
        return Ok(());
    }

    mark_dependency_install_incomplete(
        workspace,
        ctx,
        cwd,
        package_manager,
        "restore",
        &[],
        "environment.next_swc_unavailable",
    )
    .await?;
    Err(ToolError::typed_recoverable(
        format!(
            "Next.js native compiler is unavailable or corrupt (status {:?})",
            output.status
        ),
        "environment.next_swc_unavailable",
        json!({
            "cwd": display_workspace_path(cwd, ctx),
            "packageManager": package_manager,
            "status": output.status,
            "stdout": truncate_for_metadata(&output.stdout),
            "stderr": truncate_for_metadata(&output.stderr),
            "suggestedAction": "Retry project.build; Runtime will discard the incomplete dependency tree and restore dependencies from a clean state."
        }),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_restore_retries_only_bounded_transient_registry_failures() {
        assert!(should_retry_automatic_dependency_restore(
            false,
            "npm error code ECONNRESET",
            1,
        ));
        assert!(!should_retry_automatic_dependency_restore(
            false,
            "npm error code ECONNRESET",
            2,
        ));
        assert!(!should_retry_automatic_dependency_restore(
            false,
            "npm error ERESOLVE unable to resolve dependency tree",
            1,
        ));
        assert!(!should_retry_automatic_dependency_restore(
            true,
            "npm error code ECONNRESET",
            1,
        ));
    }

    #[test]
    fn next_preflight_only_applies_to_projects_declaring_next() {
        assert!(package_json_declares_next(
            r#"{"dependencies":{"next":"16.2.10"}}"#
        ));
        assert!(package_json_declares_next(
            r#"{"devDependencies":{"next":"16.2.10"}}"#
        ));
        assert!(!package_json_declares_next(
            r#"{"dependencies":{"react":"19.0.0"}}"#
        ));
        assert!(!package_json_declares_next("not-json"));
    }

    #[tokio::test]
    async fn incomplete_install_marker_forces_clean_dependency_restore() {
        let workspace_root = std::env::temp_dir().join(format!(
            "zerondesign-dependency-cleanup-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cwd = workspace_root.join("project");
        let node_modules = cwd.join("node_modules");
        fs::create_dir_all(node_modules.join("@next/swc-linux-arm64-gnu")).unwrap();
        fs::write(
            node_modules.join("@next/swc-linux-arm64-gnu/next-swc.node"),
            b"truncated",
        )
        .unwrap();
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "dependency-cleanup-project".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                Vec::new(),
            )
            .await;
        let ctx = ToolContext::new(store, run, workspace_root.clone());

        mark_dependency_install_incomplete(
            &LocalWorkspaceBackend,
            &ctx,
            &cwd,
            "npm",
            "restore",
            &[],
            "dependency.install_timeout",
        )
        .await
        .unwrap();
        let cleaned = prepare_dependency_install_workspace(&LocalWorkspaceBackend, &ctx, &cwd)
            .await
            .unwrap();

        assert!(cleaned);
        assert!(!node_modules.exists());
        let state =
            read_workspace_json(&LocalWorkspaceBackend, &ctx, "state/dependency-state.json")
                .await
                .unwrap();
        assert_eq!(state["needsRestore"], true);
        assert_eq!(state["cleanupNodeModules"], false);
        fs::remove_dir_all(workspace_root).unwrap();
    }
}
