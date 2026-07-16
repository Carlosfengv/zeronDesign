use super::*;

pub(super) fn diagnostics_build_log_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(DiagnosticsBuildLogTool { workspace })
}

pub(super) fn diagnostics_typescript_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(DiagnosticsTypescriptTool { workspace })
}

pub(super) fn diagnostics_accessibility_tool(
    workspace: Arc<dyn WorkspaceBackend>,
) -> Arc<dyn Tool> {
    Arc::new(DesignContextFidelityDiagnosticsTool {
        workspace,
        kind: "a11y",
        name: "diagnostics.accessibility",
    })
}

pub(super) fn preview_audit_responsive_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(DesignContextFidelityDiagnosticsTool {
        workspace,
        kind: "viewport",
        name: "preview.audit_responsive",
    })
}

struct DiagnosticsBuildLogTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for DiagnosticsBuildLogTool {
    fn name(&self) -> &'static str {
        "diagnostics.build_log"
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
        allow_with_input(input, "build log diagnostics allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = ctx.workspace_root.join("outputs/build/build.log");
        let text = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .unwrap_or_default();
        let has_terminal_error = has_terminal_error(&text);
        Ok(ToolResult::ok(json!({
            "path": "/workspace/outputs/build/build.log",
            "text": text,
            "hasTerminalError": has_terminal_error,
        })))
    }
}

struct DiagnosticsTypescriptTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

struct DesignContextFidelityDiagnosticsTool {
    workspace: Arc<dyn WorkspaceBackend>,
    kind: &'static str,
    name: &'static str,
}

#[async_trait]
impl Tool for DesignContextFidelityDiagnosticsTool {
    fn name(&self) -> &'static str {
        self.name
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
        allow_with_input(input, "Design Context fidelity diagnostics allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let report = read_workspace_json(
            &*self.workspace,
            &ctx,
            "state/design-profile-fidelity.json",
        )
        .await
        .ok_or_else(|| {
            typed_recoverable(
                format!(
                    "{} requires a completed preview.publish fidelity report",
                    self.name
                ),
                "design_context.fidelity_report_missing",
                json!({
                    "suggestedAction": "Call preview.publish to collect Runtime-owned verification evidence first."
                }),
            )
        })?;
        let findings = report
            .get("assertions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|assertion| assertion.get("kind").and_then(Value::as_str) == Some(self.kind))
            .collect::<Vec<_>>();
        Ok(ToolResult::ok(json!({
            "reportVersion": report.get("version").cloned(),
            "status": report.get("status").cloned(),
            "kind": self.kind,
            "findings": findings,
        })))
    }
}

#[async_trait]
impl Tool for DiagnosticsTypescriptTool {
    fn name(&self) -> &'static str {
        "diagnostics.typescript"
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
        allow_with_input(input, "typescript diagnostics allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(
            read_workspace_json(&*self.workspace, &ctx, "outputs/reports/typescript.json")
                .await
                .unwrap_or_else(|| {
                    json!({
                        "ok": true,
                        "diagnostics": [],
                    })
                }),
        ))
    }
}
