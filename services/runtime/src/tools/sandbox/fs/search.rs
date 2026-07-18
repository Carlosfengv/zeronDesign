use super::*;

pub(super) fn fs_list_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsListTool { workspace })
}

pub(super) fn fs_search_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsSearchTool { workspace })
}

struct FsListTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsListTool {
    fn name(&self) -> &'static str {
        "fs.list"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        let entries = self
            .workspace
            .list_dir(&ctx, &path)
            .await
            .map_err(|error| {
                let display_path = display_workspace_path(&path, &ctx);
                ToolError::typed_recoverable(
                    format!("failed to list {display_path}: {error}"),
                    "fs.list_failed",
                    json!({
                        "path": display_path,
                        "suggestedAction": "Verify the directory exists, or call fs.read if the path is a file."
                    }),
                )
            })?;
        let entries = entries
            .into_iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "kind": match entry.kind {
                        WorkspaceEntryKind::Dir => "dir",
                        WorkspaceEntryKind::File => "file",
                    },
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "entries": entries }),
        ))
    }
}

struct FsSearchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsSearchTool {
    fn name(&self) -> &'static str {
        "fs.search"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "query": string_schema("Text query")
            }),
            &["path", "query"],
        )
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "query", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        let query = required_str(&input, "query")?;
        let mut matches = Vec::new();
        let summary =
            collect_search_matches(&*self.workspace, &path, &ctx, query, &mut matches).await?;
        Ok(ToolResult::ok(json!({
            "matches": matches,
            "filesScanned": summary.files_scanned,
            "bytesScanned": summary.bytes_scanned,
            "skippedPaths": summary.skipped_paths,
            "truncated": summary.truncated,
        })))
    }
}
