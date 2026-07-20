use super::*;

pub(super) fn fs_write_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsWriteTool { workspace })
}

pub(super) fn fs_write_chunk_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsWriteChunkTool { workspace })
}

pub(super) fn fs_commit_chunks_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsCommitChunksTool { workspace })
}

pub(super) fn fs_patch_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsPatchTool { workspace })
}

pub(super) fn fs_multi_patch_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsMultiPatchTool { workspace })
}

struct FsWriteTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsWriteTool {
    fn name(&self) -> &'static str {
        "fs.write"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "text": string_schema("File contents. Max 48000 chars and max 96000 serialized argument bytes. For larger files use fs.write_chunk then fs.commit_chunks.")
            }),
            &["path", "text"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "text", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        validate_project_mutation_write_path(&input, ctx, self.name())?;
        validate_write_payload_budget(&input, "fs.write")?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_write_path(&input, &ctx)?;
        let text = required_str(&input, "text")?;
        ensure_project_mutation_content(&path, text, &ctx)?;
        preview_dev::validate_dev_file_mutation(&ctx, &path)?;
        self.workspace
            .write_string(&ctx, &path, text)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!("failed to write {}: {error}", path.display()))
            })?;
        let draft_preview =
            preview_dev::record_dev_file_mutation(&*self.workspace, &ctx, &path).await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "bytes": text.len(),
            "draftPreview": draft_preview,
        })))
    }
}

struct FsWriteChunkTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsWriteChunkTool {
    fn name(&self) -> &'static str {
        "fs.write_chunk"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Final workspace path"),
                "sessionId": string_schema("Chunked write session id"),
                "index": { "type": "integer", "minimum": 0, "description": "Zero-based chunk index" },
                "total": { "type": "integer", "minimum": 1, "maximum": MAX_CHUNKS_PER_WRITE, "description": "Total chunk count" },
                "text": string_schema("Chunk contents. Max 24000 chars and max 48000 serialized argument bytes.")
            }),
            &["path", "sessionId", "index", "total", "text"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "sessionId", self.name())?;
        require_string(&input, "text", self.name())?;
        validate_project_mutation_write_path(&input, ctx, self.name())?;
        let index = required_u64_validation(&input, "index", self.name())?;
        let total = required_u64_validation(&input, "total", self.name())?;
        validate_chunk_bounds(index, total)?;
        validate_chunk_payload_budget(&input)?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let final_path = checked_write_path(&input, &ctx)?;
        let session_id = safe_session_id(required_str(&input, "sessionId")?)?;
        let index = required_u64(&input, "index")?;
        let total = required_u64(&input, "total")?;
        validate_chunk_bounds_tool(index, total)?;
        let text = required_str(&input, "text")?;
        cleanup_expired_staged_writes(&*self.workspace, &ctx).await?;
        update_chunk_manifest(
            &*self.workspace,
            &ctx,
            &session_id,
            &final_path,
            index,
            total,
        )
        .await?;
        let chunk_path = staged_chunk_path(&ctx, &session_id, index);
        self.workspace
            .write_string(&ctx, &chunk_path, text)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!(
                    "failed to stage chunk {index} for {}: {error}",
                    final_path.display()
                ))
            })?;
        let display_path = display_workspace_path(&final_path, &ctx);
        let _ = ctx
            .store
            .append_event(AgentEvent::ChunkReceived {
                run_id: ctx.run.id.clone(),
                path: display_path.clone(),
                session_id: session_id.clone(),
                index,
                total,
                bytes: text.len(),
                chars: text.chars().count(),
                timestamp: Utc::now(),
            })
            .await;
        let _ = ctx
            .store
            .append_event(AgentEvent::MetricRecorded {
                run_id: ctx.run.id.clone(),
                name: "tool_chunk_write_started".to_string(),
                value: 1,
                metadata: Some(json!({
                    "tool": self.name(),
                    "path": display_path,
                    "sessionId": session_id.clone(),
                    "index": index,
                    "total": total,
                    "bytes": text.len(),
                    "chars": text.chars().count(),
                })),
                timestamp: Utc::now(),
            })
            .await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&final_path, &ctx),
            "sessionId": session_id,
            "index": index,
            "total": total,
            "chunkPath": display_workspace_path(&chunk_path, &ctx),
            "chars": text.chars().count(),
        })))
    }
}

struct FsCommitChunksTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsCommitChunksTool {
    fn name(&self) -> &'static str {
        "fs.commit_chunks"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Final workspace path"),
                "sessionId": string_schema("Chunked write session id"),
                "total": { "type": "integer", "minimum": 1, "maximum": MAX_CHUNKS_PER_WRITE, "description": "Total chunk count" },
                "mode": string_schema("Commit mode: create, overwrite, or append. Defaults to overwrite."),
                "sha256": string_schema("Optional expected final sha256")
            }),
            &["path", "sessionId", "total"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "sessionId", self.name())?;
        validate_project_mutation_write_path(&input, ctx, self.name())?;
        if input.get("sha256").is_some() {
            require_string(&input, "sha256", self.name())?;
        }
        if let Some(mode) = input.get("mode") {
            let Some(mode) = mode.as_str() else {
                return Err(ValidationError::with_kind(
                    "fs.commit_chunks requires string mode",
                    "tool.input_schema_invalid",
                ));
            };
            if !matches!(mode, "create" | "overwrite" | "append") {
                return Err(ValidationError::with_kind(
                    "fs.commit_chunks mode must be create, overwrite, or append",
                    "tool.input_schema_invalid",
                ));
            }
        }
        let total = required_u64_validation(&input, "total", self.name())?;
        validate_chunk_bounds(0, total)?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let final_path = checked_write_path(&input, &ctx)?;
        let session_id = safe_session_id(required_str(&input, "sessionId")?)?;
        let total = required_u64(&input, "total")?;
        validate_chunk_bounds_tool(0, total)?;
        let mode = input
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("overwrite");
        let manifest = read_chunk_manifest(&*self.workspace, &ctx, &session_id)
            .await
            .ok_or_else(|| {
                ToolError::Recoverable(format!(
                    "missing chunk session manifest for session {session_id}"
                ))
            })?;
        validate_chunk_manifest_for_commit(&manifest, &ctx, &final_path, total)?;
        let mut content = String::new();
        for index in 0..total {
            let chunk_path = staged_chunk_path(&ctx, &session_id, index);
            let chunk = self
                .workspace
                .read_to_string(&ctx, &chunk_path)
                .await
                .map_err(|error| {
                    ToolError::Recoverable(format!(
                        "missing or unreadable chunk {index}/{total} for session {session_id}: {error}"
                    ))
                })?;
            content.push_str(&chunk);
        }
        let existing_content = match mode {
            "create" => match self.workspace.read_to_string(&ctx, &final_path).await {
                Ok(_) => {
                    return Err(ToolError::Recoverable(format!(
                        "fs.commit_chunks mode=create refused to overwrite existing {}",
                        display_workspace_path(&final_path, &ctx)
                    )));
                }
                Err(_) => String::new(),
            },
            "append" => self
                .workspace
                .read_to_string(&ctx, &final_path)
                .await
                .unwrap_or_default(),
            _ => String::new(),
        };
        let final_content = if mode == "append" {
            format!("{existing_content}{content}")
        } else {
            content
        };
        ensure_project_mutation_content(&final_path, &final_content, &ctx)?;
        preview_dev::validate_dev_file_mutation(&ctx, &final_path)?;
        let actual_sha256 = sha256_hex(final_content.as_bytes());
        if let Some(expected) = input.get("sha256").and_then(Value::as_str) {
            if expected != actual_sha256 {
                return Err(ToolError::Recoverable(format!(
                    "chunk commit sha256 mismatch: expected {expected}, got {actual_sha256}"
                )));
            }
        }
        let tmp_path = final_path.with_extension("tmp-anydesign-chunks");
        self.workspace
            .write_string(&ctx, &tmp_path, &final_content)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!("failed to write temp file: {error}"))
            })?;
        self.workspace
            .rename(&ctx, &tmp_path, &final_path)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!(
                    "failed to commit chunks to {}: {error}",
                    final_path.display()
                ))
            })?;
        let _ = self
            .workspace
            .remove_dir_all(&ctx, &staged_session_dir(&ctx, &session_id))
            .await;
        let display_path = display_workspace_path(&final_path, &ctx);
        let _ = ctx
            .store
            .append_event(AgentEvent::ChunkCommitted {
                run_id: ctx.run.id.clone(),
                path: display_path.clone(),
                session_id: session_id.clone(),
                total,
                bytes: final_content.len(),
                chars: final_content.chars().count(),
                sha256: actual_sha256.clone(),
                timestamp: Utc::now(),
            })
            .await;
        let _ = ctx
            .store
            .append_event(AgentEvent::MetricRecorded {
                run_id: ctx.run.id.clone(),
                name: "tool_chunk_write_committed".to_string(),
                value: 1,
                metadata: Some(json!({
                    "tool": self.name(),
                    "path": display_path.clone(),
                    "sessionId": session_id.clone(),
                    "total": total,
                    "mode": mode,
                    "bytes": final_content.len(),
                    "chars": final_content.chars().count(),
                    "sha256": actual_sha256.clone(),
                })),
                timestamp: Utc::now(),
            })
            .await;
        record_chunk_write_health(
            &*self.workspace,
            &ctx,
            json!({
                "status": "committed",
                "path": display_workspace_path(&final_path, &ctx),
                "sessionId": session_id.clone(),
                "total": total,
                "mode": mode,
                "bytes": final_content.len(),
                "chars": final_content.chars().count(),
                "sha256": actual_sha256.clone(),
            }),
        )
        .await?;
        let draft_preview =
            preview_dev::record_dev_file_mutation(&*self.workspace, &ctx, &final_path).await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&final_path, &ctx),
            "sessionId": session_id,
            "total": total,
            "mode": mode,
            "bytes": final_content.len(),
            "chars": final_content.chars().count(),
            "sha256": actual_sha256,
            "draftPreview": draft_preview,
        })))
    }
}

struct FsPatchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsPatchTool {
    fn name(&self) -> &'static str {
        "fs.patch"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "oldStr": string_schema("Existing exact text"),
                "newStr": string_schema("Replacement text"),
                "replaceAll": { "type": "boolean" }
            }),
            &["path", "oldStr", "newStr"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "oldStr", self.name())?;
        require_string(&input, "newStr", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        validate_project_mutation_write_path(&input, ctx, self.name())?;
        if input.get("replaceAll").is_some()
            && !input.get("replaceAll").is_some_and(Value::is_boolean)
        {
            return Err(ValidationError::new("fs.patch replaceAll must be boolean"));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        ensure_not_nested_package_root(&path, &ctx)?;
        ensure_project_mutation_write_path(&path, &ctx)?;
        let old_str = required_str(&input, "oldStr")?;
        let new_str = required_str(&input, "newStr")?;
        let replace_all = input
            .get("replaceAll")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let read_entry = read_tracking_entry(&*self.workspace, &ctx, &path).await;
        if read_entry.is_none() {
            return Err(typed_recoverable(
                "fs.patch requires reading the target file first. Call fs.read on the path, then retry with a small unique oldStr of roughly 2-6 lines; do not paste the whole file.".to_string(),
                "patch.read_required",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "suggestedAction": "Call fs.read on this path before fs.patch."
                }),
            ));
        }
        let content = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let current_hash = sha256_hex(content.as_bytes());
        let read_hash = read_entry
            .as_ref()
            .and_then(|entry| entry.get("contentHash").and_then(Value::as_str));
        if read_hash != Some(current_hash.as_str()) {
            return Err(typed_recoverable(
                "file has been modified since fs.read; read it again before attempting fs.patch"
                    .to_string(),
                "patch.stale_read",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "currentHash": current_hash,
                    "readHash": read_hash,
                    "suggestedAction": "Call fs.read again and patch against current contents."
                }),
            ));
        }
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err(typed_recoverable(
                "oldStr not found in file".to_string(),
                "patch.old_str_missing",
                patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content)),
            ));
        }
        if count > 1 && !replace_all {
            return Err(typed_recoverable(
                "oldStr found multiple times, provide more context or set replaceAll=true to replace every occurrence".to_string(),
                "patch.old_str_ambiguous",
                patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content)),
            ));
        }
        let new_content = if replace_all {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };
        ensure_project_mutation_content(&path, &new_content, &ctx)?;
        preview_dev::validate_dev_file_mutation(&ctx, &path)?;
        self.workspace
            .write_string(&ctx, &path, &new_content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&*self.workspace, &ctx, &path, &new_content).await?;
        let draft_preview =
            preview_dev::record_dev_file_mutation(&*self.workspace, &ctx, &path).await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "patched": true,
            "replaceAll": replace_all,
            "replacements": if replace_all { count } else { 1 },
            "draftPreview": draft_preview,
        })))
    }
}

struct FsMultiPatchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsMultiPatchTool {
    fn name(&self) -> &'static str {
        "fs.multi_patch"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldStr": string_schema("Existing exact text"),
                            "newStr": string_schema("Replacement text"),
                            "replaceAll": { "type": "boolean" }
                        },
                        "required": ["oldStr", "newStr"]
                    }
                }
            }),
            &["path", "edits"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        validate_project_mutation_write_path(&input, ctx, self.name())?;
        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError::new("fs.multi_patch requires edits array"))?;
        if edits.is_empty() {
            return Err(ValidationError::new(
                "fs.multi_patch requires at least one edit",
            ));
        }
        for edit in edits {
            require_string(edit, "oldStr", self.name())?;
            require_string(edit, "newStr", self.name())?;
            if edit.get("replaceAll").is_some()
                && !edit.get("replaceAll").is_some_and(Value::is_boolean)
            {
                return Err(ValidationError::new(
                    "fs.multi_patch edit replaceAll must be boolean",
                ));
            }
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        ensure_not_nested_package_root(&path, &ctx)?;
        ensure_project_mutation_write_path(&path, &ctx)?;
        let read_entry = read_tracking_entry(&*self.workspace, &ctx, &path).await;
        if read_entry.is_none() {
            return Err(typed_recoverable(
                "fs.multi_patch requires reading the target file first. Call fs.read on the path, then retry with small unique oldStr snippets.".to_string(),
                "patch.read_required",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "suggestedAction": "Call fs.read on this path before fs.multi_patch."
                }),
            ));
        }
        let original_content = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let current_hash = sha256_hex(original_content.as_bytes());
        let read_hash = read_entry
            .as_ref()
            .and_then(|entry| entry.get("contentHash").and_then(Value::as_str));
        if read_hash != Some(current_hash.as_str()) {
            return Err(typed_recoverable(
                "file has been modified since fs.read; read it again before attempting fs.multi_patch"
                    .to_string(),
                "patch.stale_read",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "currentHash": current_hash,
                    "readHash": read_hash,
                    "suggestedAction": "Call fs.read again and patch against current contents."
                }),
            ));
        }

        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::Recoverable("fs.multi_patch requires edits array".to_string())
            })?;
        let mut content = original_content;
        let mut applied = Vec::new();
        let mut total_replacements = 0usize;
        for (index, edit) in edits.iter().enumerate() {
            let old_str = required_str(edit, "oldStr")?;
            let new_str = required_str(edit, "newStr")?;
            let replace_all = edit
                .get("replaceAll")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let count = content.matches(old_str).count();
            if count == 0 {
                let mut metadata =
                    patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content));
                metadata["editIndex"] = json!(index);
                return Err(typed_recoverable(
                    format!("fs.multi_patch edit {index} oldStr not found in file"),
                    "patch.old_str_missing",
                    metadata,
                ));
            }
            if count > 1 && !replace_all {
                let mut metadata =
                    patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content));
                metadata["editIndex"] = json!(index);
                return Err(typed_recoverable(
                    format!("fs.multi_patch edit {index} oldStr found multiple times, provide more context or set replaceAll=true"),
                    "patch.old_str_ambiguous",
                    metadata,
                ));
            }
            content = if replace_all {
                content.replace(old_str, new_str)
            } else {
                content.replacen(old_str, new_str, 1)
            };
            let replacements = if replace_all { count } else { 1 };
            total_replacements += replacements;
            applied.push(json!({
                "index": index,
                "replaceAll": replace_all,
                "replacements": replacements,
            }));
        }
        ensure_project_mutation_content(&path, &content, &ctx)?;
        preview_dev::validate_dev_file_mutation(&ctx, &path)?;
        self.workspace
            .write_string(&ctx, &path, &content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&*self.workspace, &ctx, &path, &content).await?;
        let draft_preview =
            preview_dev::record_dev_file_mutation(&*self.workspace, &ctx, &path).await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "patched": true,
            "edits": applied,
            "replacements": total_replacements,
            "draftPreview": draft_preview,
        })))
    }
}
