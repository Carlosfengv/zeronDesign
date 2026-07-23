use super::*;

pub(super) fn fs_read_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(FsReadTool { workspace })
}

pub(super) fn design_source_read_sections_tool() -> Arc<dyn Tool> {
    Arc::new(DesignSourceReadSectionsTool)
}

struct FsReadTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

struct DesignSourceReadSectionsTool;

#[async_trait]
impl Tool for DesignSourceReadSectionsTool {
    fn name(&self) -> &'static str {
        "design_source.read_sections"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "sourceArtifactId": string_schema("Design source artifact bound to this run"),
                "sectionIds": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "uniqueItems": true
                },
                "expectedSourceHash": string_schema("SHA-256 hash from the run snapshot")
            }),
            &["sourceArtifactId", "sectionIds", "expectedSourceHash"],
        )
    }

    fn is_enabled(&self, ctx: &ToolContext) -> bool {
        ctx.run.design_fidelity_mode.as_deref() == Some("source_fallback")
            && ctx.run.design_source_artifact_id.is_some()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "sourceArtifactId", self.name())?;
        require_string(&input, "expectedSourceHash", self.name())?;
        let artifact_id = input["sourceArtifactId"].as_str().unwrap_or_default();
        let expected_hash = input["expectedSourceHash"].as_str().unwrap_or_default();
        if ctx.run.design_source_artifact_id.as_deref() != Some(artifact_id)
            || ctx.run.design_source_hash.as_deref() != Some(expected_hash)
        {
            return Err(ValidationError::with_kind(
                "design source artifact or hash does not match the current run snapshot",
                "design_source.snapshot_mismatch",
            ));
        }
        let section_ids = input
            .get("sectionIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ValidationError::with_kind(
                    "design_source.read_sections requires sectionIds",
                    "tool.input_schema_invalid",
                )
            })?;
        if section_ids.is_empty() {
            return Err(ValidationError::with_kind(
                "design_source.read_sections requires at least one sectionId",
                "tool.input_schema_invalid",
            ));
        }
        let mut selected_bytes = 0u64;
        let mut seen = std::collections::HashSet::new();
        for section_id in section_ids {
            let section_id = section_id.as_str().ok_or_else(|| {
                ValidationError::with_kind(
                    "sectionIds must contain strings",
                    "tool.input_schema_invalid",
                )
            })?;
            if !seen.insert(section_id) {
                return Err(ValidationError::with_kind(
                    "sectionIds must not contain duplicates",
                    "tool.input_schema_invalid",
                ));
            }
            let section = ctx
                .run
                .design_source_sections
                .iter()
                .find(|section| section.id == section_id)
                .ok_or_else(|| {
                    ValidationError::with_kind(
                        format!("unknown source section for current run: {section_id}"),
                        "design_source.section_not_found",
                    )
                })?;
            selected_bytes += (section.end_byte - section.start_byte) as u64;
        }
        if selected_bytes > 16 * 1024 {
            return Err(ValidationError::with_kind(
                "design_source.read_sections may return at most 16 KiB per call",
                "design_source.read_limit_exceeded",
            ));
        }
        let budget = ctx.run.design_source_budget_bytes.unwrap_or(48 * 1024);
        if ctx
            .run
            .design_source_bytes_read
            .saturating_add(selected_bytes)
            > budget
        {
            ctx.store
                .update_run_status(&ctx.run.id, AgentRunStatus::NeedsUserInput)
                .await
                .ok();
            ctx.store
                .append_event(AgentEvent::StateChanged {
                    run_id: ctx.run.id.clone(),
                    state: "needs_user_input:design_profile_source_budget_exceeded".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .ok();
            return Err(ValidationError::with_kind(
                "design profile source budget exceeded",
                "design_source.budget_exceeded",
            ));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "read-only source sections bound to the current run".to_string(),
            },
        }
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let artifact_id = input["sourceArtifactId"].as_str().unwrap_or_default();
        let source = ctx
            .store
            .read_design_source_artifact_content(artifact_id)
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let section_ids = input["sectionIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let mut sections = Vec::new();
        let mut hashes = Vec::new();
        let mut bytes_read = 0u64;
        for section_id in section_ids {
            let section = ctx
                .run
                .design_source_sections
                .iter()
                .find(|section| section.id == section_id)
                .ok_or_else(|| {
                    ToolError::Terminal(format!("source section disappeared: {section_id}"))
                })?;
            let bytes = source
                .get(section.start_byte..section.end_byte)
                .ok_or_else(|| {
                    ToolError::Terminal(format!("source section range is invalid: {section_id}"))
                })?;
            if crate::types::sha256_hex(bytes) != section.sha256 {
                return Err(ToolError::Terminal(format!(
                    "source section integrity check failed: {section_id}"
                )));
            }
            bytes_read += bytes.len() as u64;
            hashes.push(section.sha256.clone());
            sections.push(json!({
                "id": section.id,
                "heading": section.heading,
                "startByte": section.start_byte,
                "endByte": section.end_byte,
                "sha256": section.sha256,
                "text": std::str::from_utf8(bytes).map_err(|_| ToolError::Terminal("source section is not UTF-8".to_string()))?,
            }));
        }
        ctx.store
            .record_design_source_sections_read(&ctx.run.id, &hashes, bytes_read)
            .await
            .map_err(|error| {
                ToolError::typed_recoverable(
                    error.to_string(),
                    "design_source.budget_exceeded",
                    json!({ "state": "needs_user_input:design_profile_source_budget_exceeded" }),
                )
            })?;
        crate::tools::runtime::record_design_context_metric(
            &ctx.store,
            &ctx.run,
            "design_context_source_sections_read",
            hashes.len() as u64,
            json!({
                "accessMode": "indexed",
                "bytesRead": bytes_read,
            }),
        )
        .await;
        Ok(ToolResult::ok(json!({
            "trustLabel": "untrusted_design_reference",
            "sourceArtifactId": artifact_id,
            "sourceHash": ctx.run.design_source_hash,
            "sections": sections,
            "bytesRead": bytes_read,
            "remainingBudgetBytes": ctx.run.design_source_budget_bytes.unwrap_or(48 * 1024).saturating_sub(ctx.run.design_source_bytes_read + bytes_read),
        })))
    }
}

#[async_trait]
impl Tool for FsReadTool {
    fn name(&self) -> &'static str {
        "fs.read"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // fs.read persists a per-run read lease in state/read-tracking.json.
        // Concurrent read-modify-write updates would lose sibling leases and
        // make a successfully read file fail the later fs.patch gate.
        false
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        let normalized = input["path"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("/workspace/");
        if normalized == "state/validation-report.json"
            || normalized.ends_with("/.anydesign-candidate-manifest.json")
            || normalized.ends_with("/.anydesign-artifact-routes.json")
        {
            return Err(ValidationError::with_kind(
                "full validation and candidate manifests are platform evidence; read state/repair-context.json for bounded source repair guidance",
                "generation.repair_context_required",
            ));
        }
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
        let text = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| {
                let workspace_path = display_workspace_path(&path, &ctx);
                ToolError::RecoverableWithMetadata {
                    message: format!("failed to read {workspace_path}: {error}"),
                    error_kind: "fs.read_failed".to_string(),
                    metadata: json!({
                        "path": workspace_path,
                        "suggestedAction": "If the path is a directory, call fs.list. Otherwise verify the file exists and retry fs.read with a workspace-relative file path."
                    }),
                }
            })?;
        record_read_path(&*self.workspace, &ctx, &path, &text).await?;
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "text": text }),
        ))
    }
}
