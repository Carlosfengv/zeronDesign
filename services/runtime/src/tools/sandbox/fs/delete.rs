use super::*;

pub(super) fn fs_delete_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsDeleteTool { workspace })
}

struct FsDeleteTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsDeleteTool {
    fn name(&self) -> &'static str {
        "fs.delete"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        match checked_delete_path(input, ctx) {
            Ok(_) => allow_with_input(input, "workspace delete path allowed"),
            Err(message) => deny(self.name(), message),
        }
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_delete_path(&input, &ctx).map_err(ToolError::PermissionDenied)?;
        preview_dev::validate_dev_mutation(&ctx)?;
        let kind = self
            .workspace
            .path_kind(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        authorize_existing_file_mutation(&*self.workspace, &ctx, &path).await?;
        match kind {
            WorkspacePathKind::Dir => self
                .workspace
                .remove_dir_all(&ctx, &path)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
            WorkspacePathKind::File => self
                .workspace
                .remove_file(&ctx, &path)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        }
        invalidate_mutation_lease(&*self.workspace, &ctx, &path).await?;
        let draft_preview = preview_dev::record_dev_mutation(&*self.workspace, &ctx).await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "deleted": true,
            "draftPreview": draft_preview,
        })))
    }
}
