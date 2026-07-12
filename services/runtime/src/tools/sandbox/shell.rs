use super::*;

pub(super) fn shell_run_tool(command: Arc<dyn SandboxCommandBackend>) -> Arc<dyn Tool> {
    Arc::new(ShellRunTool { command })
}

struct ShellRunTool {
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ShellRunTool {
    fn name(&self) -> &'static str {
        "shell.run"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "argv": { "type": "array", "items": { "type": "string" } },
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &["argv"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let argv = input
            .get("argv")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError::new("shell.run requires argv"))?;
        if argv.is_empty() || !argv.iter().all(|item| item.as_str().is_some()) {
            return Err(ValidationError::new(
                "shell.run argv must be a non-empty string array",
            ));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let argv = argv_from_input(input).unwrap_or_default();
        check_command_policy(&argv)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let argv = argv_from_input(&input)?;
        let cwd = match input.get("cwd").and_then(Value::as_str) {
            Some(cwd) => {
                check_context_workspace_path(&resolve_path(cwd, &ctx.workspace_root), &ctx)
                    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?
            }
            None => default_project_dir(&ctx),
        };
        let timeout_ms = input
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(60_000);
        let output = self
            .command
            .run_with_output_events(&ctx, &argv, &cwd, timeout_ms, None, self.name())
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    ToolError::Recoverable("shell.run timed out".to_string())
                } else if error.kind() == io::ErrorKind::Interrupted {
                    ToolError::Recoverable(error.to_string())
                } else {
                    ToolError::Recoverable(format!("shell.run failed to start: {error}"))
                }
            })?;
        if !output.success {
            return Err(ToolError::typed_recoverable(
                format!(
                    "shell.run exited with status {:?}\nstdout:\n{}\nstderr:\n{}",
                    output.status, output.stdout, output.stderr
                ),
                "shell.non_zero_exit",
                json!({
                    "status": output.status,
                    "stdout": truncate_for_metadata(&output.stdout),
                    "stderr": truncate_for_metadata(&output.stderr),
                    "suggestedAction": "Inspect stdout/stderr, fix the command arguments, or use a dedicated runtime tool when available."
                }),
            ));
        }
        Ok(ToolResult::ok(json!({
            "status": output.status,
            "success": output.success,
            "stdout": output.stdout,
            "stderr": output.stderr,
        })))
    }
}
