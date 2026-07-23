use super::*;

pub(super) fn preview_start_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewStartTool { workspace, command })
}

pub(super) fn preview_status_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewStatusTool { workspace, command })
}

pub(super) fn preview_stop_tool(
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
) -> Arc<dyn Tool> {
    Arc::new(PreviewStopTool { workspace, command })
}

pub(super) struct PreviewStartTool {
    pub(super) workspace: Arc<dyn WorkspaceBackend>,
    pub(super) command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewStartTool {
    fn name(&self) -> &'static str {
        "preview.start"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "url": string_schema("Preview URL"),
                "port": { "type": "integer", "minimum": 1 },
                "command": string_schema("Preview command label"),
                "mode": string_schema("Preview mode: static or framework")
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview start allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let build = read_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json")
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.start requires a successful project.build first".to_string(),
                    "preview.build_missing",
                    json!({
                        "suggestedAction": "Run project.build or preview.publish before preview.start."
                    }),
                )
            })?;
        if build.get("status").and_then(Value::as_str) != Some("success")
            || build.get("success").and_then(Value::as_bool) != Some(true)
        {
            return Err(typed_recoverable(
                "preview.start blocked because latest project.build did not succeed".to_string(),
                "preview.build_failed",
                json!({
                    "latestBuild": build.clone(),
                    "suggestedAction": "Fix the build error, rerun project.build, then start preview."
                }),
            ));
        }
        if ctx.remote_workspace {
            let build_id = build
                .get("buildId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    typed_recoverable(
                        "preview.start requires buildId evidence".to_string(),
                        "preview.build_evidence_invalid",
                        json!({ "latestBuild": build.clone() }),
                    )
                })?;
            let manifest_hash = build
                .get("candidateManifestHash")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    typed_recoverable(
                        "preview.start requires candidate manifest evidence".to_string(),
                        "preview.candidate_manifest_missing",
                        json!({ "latestBuild": build.clone() }),
                    )
                })?;
            let lease = ctx
                .store
                .create_preview_lease(
                    &ctx.run.id,
                    build_id.to_string(),
                    manifest_hash.to_string(),
                    900,
                )
                .await
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
            let process = match self
                .command
                .start_process(
                    &ctx,
                    &lease.id,
                    &[
                        "node".to_string(),
                        "/opt/anydesign/bootstrap/static-preview-server.js".to_string(),
                    ],
                    &ctx.workspace_root,
                )
                .await
            {
                Ok(process) => process,
                Err(error) => {
                    ctx.store.stop_preview_lease(&lease.id).await.ok();
                    return Err(typed_recoverable(
                        format!("preview process failed to start: {error}"),
                        "preview.process_failed",
                        json!({ "leaseId": lease.id }),
                    ));
                }
            };
            let readiness = self
                .command
                .run(
                    &ctx,
                    &[
                        "node".to_string(),
                        "-e".to_string(),
                        "const url='http://127.0.0.1:4321/healthz';const deadline=Date.now()+10000;const probe=()=>fetch(url).then(r=>{if(!r.ok)throw new Error(`HTTP ${r.status}`)}).then(()=>process.exit(0)).catch(error=>{if(Date.now()>=deadline){console.error(error.message);process.exit(1)}setTimeout(probe,100)});probe();".to_string(),
                    ],
                    &ctx.workspace_root,
                    12_000,
                )
                .await;
            if !readiness.as_ref().is_ok_and(|output| output.success) {
                let process_status = self.command.process_status(&ctx, &lease.id).await.ok();
                self.command.stop_process(&ctx, &lease.id).await.ok();
                ctx.store.stop_preview_lease(&lease.id).await.ok();
                let readiness_detail = readiness
                    .map(|output| output.stderr)
                    .unwrap_or_else(|error| error.to_string());
                let process_detail = process_status
                    .map(|status| {
                        format!(
                            "status={}, stdout={}, stderr={}",
                            status.status, status.stdout, status.stderr
                        )
                    })
                    .unwrap_or_else(|| "process status unavailable".to_string());
                return Err(typed_recoverable(
                    format!(
                        "preview process did not become ready: {readiness_detail}; {process_detail}"
                    ),
                    "preview.process_not_ready",
                    json!({ "leaseId": lease.id }),
                ));
            }
            let url = format!("{}/previews/{}/", ctx.runtime_public_base_url, lease.id);
            let state = json!({
                "status": "running",
                "url": url,
                "port": 4321,
                "command": "runtime-static-candidate",
                "mode": "static",
                "cwd": build.get("cwd").cloned().unwrap_or(Value::Null),
                "staticOutputPath": build.get("candidateOutputPath").cloned().unwrap_or(Value::Null),
                "candidateManifestHash": manifest_hash,
                "leaseId": lease.id,
                "leaseExpiresAt": lease.expires_at,
                "pid": process.pid,
                "processStatus": process.status,
                "build": build,
                "accessible": true,
                "managed": true,
            });
            write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
            return Ok(ToolResult::ok(state));
        }
        let cwd = default_project_dir(&ctx);
        let explicit_url = input.get("url").and_then(Value::as_str);
        let port = input
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .or_else(|| explicit_url.and_then(url_port))
            .map(Ok)
            .unwrap_or_else(allocate_preview_port)?;
        let url = explicit_url
            .map(str::to_string)
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}"))
            .to_string();
        let static_output_dir =
            if explicit_url.is_none() || verify_preview_accessible(&url).await.is_err() {
                let static_output = start_static_preview_server(&ctx, &cwd, &build, port).await?;
                wait_for_preview_accessible(
                    &format!("{}/healthz", url.trim_end_matches('/')),
                    Duration::from_secs(10),
                )
                .await?;
                Some(static_output)
            } else {
                optional_static_preview_output_dir(&ctx, &cwd, &build)
            };
        let state = json!({
            "status": "running",
            "url": url,
            "port": port,
            "command": input.get("command").and_then(Value::as_str).unwrap_or("static"),
            "mode": input.get("mode").and_then(Value::as_str).unwrap_or("static"),
            "cwd": display_workspace_path(&cwd, &ctx),
            "staticOutputPath": static_output_dir.as_ref().map(|path| display_workspace_path(path, &ctx)),
            "pid": read_preview_pid(&ctx),
            "build": build,
            "accessible": true,
            "managed": explicit_url.is_none(),
        });
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

fn allocate_preview_port() -> Result<u16, ToolError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        ToolError::Recoverable(format!(
            "preview.start failed to allocate a local port: {error}"
        ))
    })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| {
            ToolError::Recoverable(format!(
                "preview.start failed to read allocated port: {error}"
            ))
        })
}

fn static_preview_output_candidates(ctx: &ToolContext) -> Vec<String> {
    ctx.run
        .project_state_snapshot
        .as_ref()
        .and_then(|state| TemplateId::parse(&state.template_key).ok())
        .and_then(|id| BuiltInTemplateRegistry::built_in().current(&id).ok())
        .map(|spec| spec.preview.output_directories.clone())
        .unwrap_or_else(|| vec!["dist".to_string(), "out".to_string()])
}

// remote-fs-boundary: allow-begin local-preview-process
fn detect_static_preview_output_dir(ctx: &ToolContext, app_root: &Path) -> Option<PathBuf> {
    static_preview_output_candidates(ctx)
        .into_iter()
        .map(|name| app_root.join(name))
        .find(|path| path.is_dir())
}

pub(super) async fn detect_static_preview_output_dir_backend(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
) -> Option<PathBuf> {
    for name in static_preview_output_candidates(ctx) {
        let path = app_root.join(name);
        if matches!(
            workspace.path_kind(ctx, &path).await,
            Ok(WorkspacePathKind::Dir)
        ) {
            return Some(path);
        }
    }
    None
}

fn static_preview_output_dir_from_build(
    ctx: &ToolContext,
    latest_build: &Value,
) -> Option<PathBuf> {
    latest_build
        .get("staticOutputPath")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| resolve_path(path, &ctx.workspace_root))
        .filter(|path| path.is_dir())
}

fn optional_static_preview_output_dir(
    ctx: &ToolContext,
    app_root: &Path,
    latest_build: &Value,
) -> Option<PathBuf> {
    static_preview_output_dir_from_build(ctx, latest_build)
        .or_else(|| detect_static_preview_output_dir(ctx, app_root))
}

async fn start_static_preview_server(
    ctx: &ToolContext,
    _app_root: &Path,
    latest_build: &Value,
    port: u16,
) -> Result<PathBuf, ToolError> {
    let build_id = latest_build
        .get("buildId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.start requires buildId evidence".to_string(),
                "preview.build_evidence_invalid",
                json!({ "latestBuild": latest_build }),
            )
        })?;
    let candidate_output = latest_build
        .get("candidateOutputPath")
        .and_then(Value::as_str)
        .map(|path| resolve_path(path, &ctx.workspace_root))
        .ok_or_else(|| {
            typed_recoverable(
                "preview.start requires a frozen candidate output".to_string(),
                "preview.candidate_manifest_missing",
                json!({ "latestBuild": latest_build }),
            )
        })?;
    if !candidate_output.is_dir() {
        return Err(typed_recoverable(
            "preview.start candidate output is unavailable".to_string(),
            "preview.candidate_manifest_missing",
            json!({ "candidateOutputPath": display_workspace_path(&candidate_output, ctx) }),
        ));
    }
    check_existing_path(&candidate_output, &ctx.workspace_root).map_err(|error| {
        typed_recoverable(
            format!("preview.start candidate output is unavailable: {error:?}"),
            "preview.candidate_manifest_missing",
            json!({ "candidateOutputPath": display_workspace_path(&candidate_output, ctx) }),
        )
    })?;
    stop_preview_pid(ctx);
    let log_dir = ctx.workspace_root.join("outputs/preview");
    fs::create_dir_all(&log_dir).map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let stdout = fs::File::create(log_dir.join("preview.stdout.log"))
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let stderr = fs::File::create(log_dir.join("preview.stderr.log"))
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let installed_script = PathBuf::from("/opt/anydesign/bootstrap/static-preview-server.js");
    let repository_script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/static-preview-server.js");
    let script = if installed_script.is_file() {
        installed_script
    } else {
        repository_script
    };
    let mut command = TokioCommand::new("node");
    command
        .arg(script)
        .env("WORKSPACE_ROOT", &ctx.workspace_root)
        .env("CANDIDATE_BUILD_ID", build_id)
        .env("CANDIDATE_PREVIEW_HOST", "127.0.0.1")
        .env("CANDIDATE_PREVIEW_PORT", port.to_string())
        .current_dir(&ctx.workspace_root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command.spawn().map_err(|error| {
        ToolError::Recoverable(format!("preview.start failed to spawn: {error}"))
    })?;
    let pid = child.id().unwrap_or_default();
    std::mem::drop(child);
    write_preview_pid(ctx, pid).map_err(|error| ToolError::Recoverable(error.to_string()))?;
    Ok(candidate_output)
}

fn claim_serving_restart(state: &mut Value) -> Result<u32, &'static str> {
    if state.get("managed").and_then(Value::as_bool) != Some(true) {
        return Err("preview is not Runtime-managed");
    }
    let attempts = state
        .get("servingRestartAttempts")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if attempts >= 1 {
        return Err("serving restart budget exhausted");
    }
    state["servingRestartAttempts"] = json!(attempts + 1);
    Ok((attempts + 1) as u32)
}

pub(super) async fn restart_static_candidate_preview(
    workspace: &dyn WorkspaceBackend,
    command: Option<&dyn SandboxCommandBackend>,
    ctx: &ToolContext,
    latest_build: &Value,
    preview_url: &str,
) -> Result<Value, ToolError> {
    let mut state = read_workspace_json(workspace, ctx, "state/preview.json")
        .await
        .ok_or_else(|| {
            typed_recoverable(
                "serving restart requires Runtime preview state".to_string(),
                "preview.serving_restart_unavailable",
                json!({ "repairAllowed": false }),
            )
        })?;
    let attempt = claim_serving_restart(&mut state).map_err(|reason| {
        typed_recoverable(
            reason.to_string(),
            "preview.serving_restart_exhausted",
            json!({
                "maxAttempts": 1,
                "repairAllowed": false,
            }),
        )
    })?;
    state["servingRestart"] = json!({
        "attempt": attempt,
        "status": "restarting",
        "owner": "serving",
    });
    state["accessible"] = json!(false);
    write_workspace_json(workspace, ctx, "state/preview.json", &state).await?;

    let restart_result = if ctx.remote_workspace {
        let command = command.ok_or_else(|| {
            typed_recoverable(
                "remote serving restart requires a command backend".to_string(),
                "preview.serving_restart_unavailable",
                json!({ "repairAllowed": false }),
            )
        });
        match command {
            Ok(command) => {
                let lease_id = state
                    .get("leaseId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        typed_recoverable(
                            "remote serving restart requires a preview lease".to_string(),
                            "preview.lease_missing",
                            json!({ "repairAllowed": false }),
                        )
                    });
                match lease_id {
                    Ok(lease_id) => {
                        command.stop_process(ctx, &lease_id).await.ok();
                        match command
                            .start_process(
                                ctx,
                                &lease_id,
                                &[
                                    "node".to_string(),
                                    "/opt/anydesign/bootstrap/static-preview-server.js"
                                        .to_string(),
                                ],
                                &ctx.workspace_root,
                            )
                            .await
                        {
                            Ok(process) => match command
                                .run(
                                    ctx,
                                    &[
                                        "node".to_string(),
                                        "-e".to_string(),
                                        "const url='http://127.0.0.1:4321/healthz';const deadline=Date.now()+10000;const probe=()=>fetch(url).then(r=>{if(!r.ok)throw new Error(`HTTP ${r.status}`)}).then(()=>process.exit(0)).catch(error=>{if(Date.now()>=deadline){console.error(error.message);process.exit(1)}setTimeout(probe,100)});probe();".to_string(),
                                    ],
                                    &ctx.workspace_root,
                                    12_000,
                                )
                                .await
                            {
                                Ok(readiness) if readiness.success => {
                                    state["pid"] = json!(process.pid);
                                    state["processStatus"] = json!(process.status);
                                    Ok(())
                                }
                                Ok(readiness) => Err(typed_recoverable(
                                    "restarted preview process did not become ready".to_string(),
                                    "preview.process_not_ready",
                                    json!({
                                        "stderr": readiness.stderr.chars().take(512).collect::<String>(),
                                        "repairAllowed": false,
                                    }),
                                )),
                                Err(error) => Err(typed_recoverable(
                                    format!("preview readiness probe failed: {error}"),
                                    "preview.process_not_ready",
                                    json!({ "repairAllowed": false }),
                                )),
                            },
                            Err(error) => Err(typed_recoverable(
                                format!("preview process restart failed: {error}"),
                                "preview.process_failed",
                                json!({ "repairAllowed": false }),
                            )),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    } else {
        let port = state
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .or_else(|| url_port(preview_url))
            .ok_or_else(|| {
                typed_recoverable(
                    "local serving restart requires a preview port".to_string(),
                    "preview.serving_restart_unavailable",
                    json!({ "repairAllowed": false }),
                )
            });
        match port {
            Ok(port) => match start_static_preview_server(
                ctx,
                &default_project_dir(ctx),
                latest_build,
                port,
            )
            .await
            {
                Ok(_) => {
                    let health_url = format!("{}/healthz", preview_url.trim_end_matches('/'));
                    match wait_for_preview_accessible(&health_url, Duration::from_secs(10)).await {
                        Ok(()) => {
                            state["pid"] = json!(read_preview_pid(ctx));
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    };

    match restart_result {
        Ok(()) => {
            let evidence = json!({
                "attempt": attempt,
                "status": "ready",
                "owner": "serving",
            });
            state["status"] = json!("running");
            state["accessible"] = json!(true);
            state["servingRestart"] = evidence.clone();
            write_workspace_json(workspace, ctx, "state/preview.json", &state).await?;
            Ok(evidence)
        }
        Err(error) => {
            state["status"] = json!("failed");
            state["accessible"] = json!(false);
            state["servingRestart"] = json!({
                "attempt": attempt,
                "status": "failed",
                "owner": "serving",
                "diagnostic": format!("{error:?}").chars().take(512).collect::<String>(),
            });
            write_workspace_json(workspace, ctx, "state/preview.json", &state).await?;
            Err(error)
        }
    }
}

async fn wait_for_preview_accessible(url: &str, timeout: Duration) -> Result<(), ToolError> {
    let started = Instant::now();
    loop {
        match verify_preview_accessible(url).await {
            Ok(()) => return Ok(()),
            Err(error) if started.elapsed() < timeout => {
                time::sleep(Duration::from_millis(200)).await;
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
}

fn preview_pid_path(ctx: &ToolContext) -> PathBuf {
    ctx.workspace_root.join("state/preview.pid")
}

fn write_preview_pid(ctx: &ToolContext, pid: u32) -> io::Result<()> {
    let path = preview_pid_path(ctx);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, pid.to_string())
}

fn read_preview_pid(ctx: &ToolContext) -> Option<u32> {
    fs::read_to_string(preview_pid_path(ctx))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

pub(super) fn stop_preview_pid(ctx: &ToolContext) {
    let Some(pid) = read_preview_pid(ctx) else {
        return;
    };
    if pid > 0 {
        #[cfg(unix)]
        {
            let _ = StdCommand::new("kill").arg(pid.to_string()).status();
        }
        #[cfg(windows)]
        {
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status();
        }
    }
    let _ = fs::remove_file(preview_pid_path(ctx));
}
// remote-fs-boundary: allow-end local-preview-process

struct PreviewStatusTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewStatusTool {
    fn name(&self) -> &'static str {
        "preview.status"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview status allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let mut state = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| {
                json!({
                    "status": "stopped",
                    "accessible": false,
                    "url": Value::Null,
                })
            });
        if ctx.remote_workspace {
            if let Some(lease_id) = state.get("leaseId").and_then(Value::as_str) {
                let process = self
                    .command
                    .process_status(&ctx, lease_id)
                    .await
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?;
                state["processStatus"] = json!(process.status);
                state["pid"] = json!(process.pid);
                if process.status != "running" {
                    state["status"] = json!("stopped");
                    state["accessible"] = json!(false);
                }
            }
        }
        Ok(ToolResult::ok(state))
    }
}

struct PreviewStopTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewStopTool {
    fn name(&self) -> &'static str {
        "preview.stop"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview stop allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let mut state = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| json!({ "url": Value::Null }));
        if let Some(lease_id) = state.get("leaseId").and_then(Value::as_str) {
            if ctx.remote_workspace {
                self.command
                    .stop_process(&ctx, lease_id)
                    .await
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?;
            }
            ctx.store
                .stop_preview_lease(lease_id)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        } else {
            stop_preview_pid(&ctx);
        }
        state["status"] = json!("stopped");
        state["accessible"] = json!(false);
        state["pid"] = Value::Null;
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serving_restart_budget_is_durable_and_limited_to_one_attempt() {
        let mut state = json!({ "managed": true });

        assert_eq!(claim_serving_restart(&mut state), Ok(1));
        assert_eq!(state["servingRestartAttempts"], json!(1));
        assert_eq!(
            claim_serving_restart(&mut state),
            Err("serving restart budget exhausted")
        );
        assert_eq!(state["servingRestartAttempts"], json!(1));
    }

    #[test]
    fn serving_restart_refuses_unmanaged_preview() {
        let mut state = json!({ "managed": false });

        assert_eq!(
            claim_serving_restart(&mut state),
            Err("preview is not Runtime-managed")
        );
        assert!(state.get("servingRestartAttempts").is_none());
    }
}
