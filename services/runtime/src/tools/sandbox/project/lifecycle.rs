use super::project_initializer::{ProjectInitRequest, ProjectInitializer};
use super::*;

const PROJECT_INIT_SOURCE_CONTEXT_MAX_BYTES: usize = 24 * 1024;

pub(super) fn project_init_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(ProjectInitTool {
        initializer: Arc::new(ProjectInitializer::built_in(workspace.clone())),
        workspace,
    })
}

pub(super) fn project_write_page_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(ProjectWritePageTool { workspace })
}

pub(super) fn project_inspect_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(ProjectInspectTool { workspace })
}

struct ProjectInitTool {
    initializer: Arc<ProjectInitializer>,
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectInitTool {
    fn name(&self) -> &'static str {
        "project.init"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "template": string_schema("Template key such as next-app or fumadocs-docs"),
                "path": string_schema("Workspace relative app root")
            }),
            &["template"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let template = input
            .get("template")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError::new("project.init requires template"))?;
        self.initializer
            .resolve_template(template)
            .await
            .map_err(|error| {
                ValidationError::with_kind(
                    format!("project.init unsupported template: {template}"),
                    error.error_kind(),
                )
            })?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project initialization allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let outcome = self
            .initializer
            .initialize(
                &ctx,
                ProjectInitRequest {
                    template: required_str(&input, "template")?.to_string(),
                    path: input
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("project")
                        .to_string(),
                },
            )
            .await
            .map_err(|error| error.into_tool_error())?;
        let mut attestation_ctx = ctx.clone();
        attestation_ctx.run = ctx.store.get_run(&ctx.run.id).await.ok_or_else(|| {
            ToolError::Terminal(
                "project.init completed but its Run snapshot is missing".to_string(),
            )
        })?;
        let runtime_attestation =
            project_runtime_attestation(&*self.workspace, &attestation_ctx).await?;
        require_materialized_project_attestation(&runtime_attestation, "project.init")?;
        require_verified_style_contract(&runtime_attestation, "project.init")?;
        let source_observations = if attestation_ctx
            .run
            .generation_context_runtime_mode
            .as_deref()
            == Some("enabled")
        {
            project_init_source_observations(&*self.workspace, &attestation_ctx).await?
        } else {
            Vec::new()
        };

        Ok(ToolResult::ok(json!({
            "appRoot": outcome.app_root,
            "statePath": "/workspace/state/project.json",
            "template": outcome.template,
            "packageManager": "npm",
            "styleContractPath": "/workspace/state/style-contract.json",
            "runtimeAttestation": runtime_attestation,
            "sourceObservations": source_observations,
            "designProfileTokenChanges": outcome.initial_token_changes,
        })))
    }
}

async fn project_init_source_observations(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
) -> Result<Vec<Value>, ToolError> {
    let app_root = project_app_root_relative(ctx);
    let template = resolve_project_template_spec(ctx)?;
    let surface = template
        .editable_surface_view(&app_root.to_string_lossy().replace('\\', "/"))
        .map_err(|error| {
            ToolError::typed_recoverable(
                error,
                "project.editable_surface_invalid",
                json!({ "blocking": true }),
            )
        })?;
    let mut paths = surface
        .primary_routes
        .into_iter()
        .map(|route| route.source)
        .chain(surface.global_style_files)
        .chain(std::iter::once(surface.token_file))
        .collect::<BTreeSet<_>>();
    let mut delivered_bytes = 0usize;
    let mut observations = Vec::new();
    for relative in std::mem::take(&mut paths) {
        let path = ctx.workspace_root.join(&relative);
        let text = workspace
            .read_to_string(ctx, &path)
            .await
            .map_err(|error| {
                ToolError::typed_recoverable(
                    format!("project.init could not read initialized source {relative}: {error}"),
                    "project.initial_source_unavailable",
                    json!({ "path": relative, "blocking": true }),
                )
            })?;
        if delivered_bytes.saturating_add(text.len()) > PROJECT_INIT_SOURCE_CONTEXT_MAX_BYTES {
            continue;
        }
        delivered_bytes = delivered_bytes.saturating_add(text.len());
        record_read_path(workspace, ctx, &path, &text).await?;
        observations.push(json!({
            "path": display_workspace_path(&path, ctx),
            "text": text,
            "contentSha256": sha256_hex(text.as_bytes()),
            "bytes": text.len(),
            "view": "full",
            "purpose": "source",
        }));
    }
    Ok(observations)
}

struct ProjectWritePageTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectWritePageTool {
    fn name(&self) -> &'static str {
        "project.write_page"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "route": string_schema("Page route such as /, /pricing, or /docs/getting-started"),
                "title": string_schema("Page title"),
                "styleProfile": string_schema("Visual style profile, for example saas"),
                "sections": {
                    "type": "array",
                    "description": "Structured page sections. Each section may include kind, heading, body, and visual.",
                    "items": {
                        "type": "object",
                        "additionalProperties": true,
                        "properties": {
                            "kind": { "type": "string" },
                            "heading": { "type": "string" },
                            "body": { "type": "string" },
                            "visual": { "type": "string" }
                        }
                    }
                }
            }),
            &["route", "title", "sections"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "route", self.name())?;
        require_string(&input, "title", self.name())?;
        let Some(sections) = input.get("sections").and_then(Value::as_array) else {
            return Err(ValidationError::with_kind(
                "project.write_page requires sections array",
                "tool.input_schema_invalid",
            ));
        };
        if sections.is_empty() {
            return Err(ValidationError::with_kind(
                "project.write_page requires at least one section",
                "tool.input_schema_invalid",
            ));
        }
        let template = resolve_project_template_spec(ctx).map_err(|error| {
            ValidationError::with_kind(format!("{error:?}"), "project.state_missing")
        })?;
        template
            .operations
            .render_page(&page_render_request(&input))
            .map_err(|error| ValidationError::with_kind(error.message, error.error_kind))?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        let Ok(template) = resolve_project_template_spec(ctx) else {
            return deny(self.name(), "project runtime state is missing");
        };
        let Ok(rendered) = template.operations.render_page(&page_render_request(input)) else {
            return deny(self.name(), "project.write_page is unsupported or invalid");
        };
        let app_root = project_app_root_relative(ctx);
        for file in rendered {
            let path = app_root.join(file.path);
            let synthetic = json!({ "path": path.to_string_lossy().to_string() });
            match check_write_path_permission(&synthetic, ctx, self.name()) {
                PermissionResult::Allow { .. } => {}
                other => return other,
            }
        }
        allow_with_input(input, "project page render paths allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let route = required_str(&input, "route")?;
        let title = required_str(&input, "title")?;
        let style_profile = input
            .get("styleProfile")
            .and_then(Value::as_str)
            .unwrap_or("saas");
        let sections = input
            .get("sections")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::Recoverable("project.write_page missing sections".to_string())
            })?;
        let app_root = default_project_dir(&ctx);
        let template = resolve_project_template_spec(&ctx)?;
        let rendered = template
            .operations
            .render_page(&RenderPageRequest {
                route: route.to_string(),
                title: title.to_string(),
                style_profile: style_profile.to_string(),
                sections: sections.clone(),
            })
            .map_err(|error| {
                ToolError::typed_recoverable(
                    error.message,
                    error.error_kind,
                    json!({
                        "templateId": template.id.as_str(),
                        "suggestedAction": "Use an operation supported by the active template or select a compatible template."
                    }),
                )
            })?;
        let mut written_paths = Vec::with_capacity(rendered.len());
        let mut bytes = 0usize;
        preview_dev::validate_dev_mutation(&ctx)?;
        for file in &rendered {
            let page_path = check_context_workspace_path(&app_root.join(&file.path), &ctx)
                .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?;
            authorize_existing_file_mutation(&*self.workspace, &ctx, &page_path).await?;
        }
        for file in rendered {
            let page_path = check_context_workspace_path(&app_root.join(&file.path), &ctx)
                .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?;
            ensure_not_nested_package_root(&page_path, &ctx)?;
            ensure_project_mutation_write_path(&page_path, &ctx)?;
            self.workspace
                .write_string(&ctx, &page_path, &file.content)
                .await
                .map_err(|error| {
                    ToolError::Recoverable(format!(
                        "failed to write {}: {error}",
                        page_path.display()
                    ))
                })?;
            advance_mutation_lease(&*self.workspace, &ctx, &page_path, &file.content).await?;
            bytes += file.content.len();
            written_paths.push(display_workspace_path(&page_path, &ctx));
        }
        let primary_path = written_paths.first().cloned().unwrap_or_default();
        let draft_preview = preview_dev::record_dev_mutation(&*self.workspace, &ctx).await;
        Ok(ToolResult::ok(json!({
            "route": route,
            "path": primary_path,
            "paths": written_paths,
            "bytes": bytes,
            "sections": sections.len(),
            "styleProfile": style_profile,
            "draftPreview": draft_preview,
        })))
    }
}

fn page_render_request(input: &Value) -> RenderPageRequest {
    RenderPageRequest {
        route: input
            .get("route")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: input
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        style_profile: input
            .get("styleProfile")
            .and_then(Value::as_str)
            .unwrap_or("saas")
            .to_string(),
        sections: input
            .get("sections")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

struct ProjectInspectTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectInspectTool {
    fn name(&self) -> &'static str {
        "project.inspect"
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
        allow_with_input(input, "project inspection allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let project_hint = read_workspace_json(&*self.workspace, &ctx, "state/project.json").await;
        let project = ctx
            .run
            .project_state_snapshot
            .as_ref()
            .and_then(|state| serde_json::to_value(state).ok());
        let app_root_relative = project_app_root_relative(&ctx);
        let app_root = ctx.workspace_root.join(&app_root_relative);
        let package_manager =
            package_manager_from_project_state_or_lockfiles(&*self.workspace, &ctx, &app_root)
                .await;
        let package_json = self
            .workspace
            .read_to_string(&ctx, &app_root.join("package.json"))
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let style_contract =
            read_workspace_json(&*self.workspace, &ctx, "state/style-contract.json").await;
        let runtime_attestation = if ctx.run.project_state_snapshot.is_some() {
            Some(project_runtime_attestation(&*self.workspace, &ctx).await?)
        } else {
            None
        };
        let latest_build =
            read_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json").await;
        let dependency_state =
            read_workspace_json(&*self.workspace, &ctx, "state/dependency-state.json").await;
        let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json").await;
        let browser = read_workspace_json(&*self.workspace, &ctx, "state/browser.json").await;
        let editable_surface = resolve_project_template_spec(&ctx)
            .and_then(|template| {
                template
                    .editable_surface_view(&app_root_relative.to_string_lossy().replace('\\', "/"))
                    .map_err(|error| {
                        ToolError::typed_recoverable(
                            error,
                            "project.editable_surface_invalid",
                            json!({ "blocking": true }),
                        )
                    })
            })
            .ok()
            .and_then(|view| serde_json::to_value(view).ok());
        let key_source_files =
            project_key_source_files(&*self.workspace, &ctx, &app_root_relative, project.as_ref())
                .await;
        let project_state_conflict = match (&project, &project_hint) {
            (Some(authority), Some(hint)) => {
                ["appRoot", "templateKey", "framework", "packageManager"]
                    .iter()
                    .any(|key| authority.get(key) != hint.get(key))
            }
            (Some(_), None) => true,
            _ => false,
        };
        if project_state_conflict {
            return Err(ToolError::typed_recoverable(
                "state/project.json conflicts with the RuntimeStore project state".to_string(),
                "project.state_conflict",
                json!({
                    "runtimeState": project,
                    "workspaceHint": project_hint,
                    "suggestedAction": "Do not edit state/project.json directly. Re-run project.init through the Runtime or repair the workspace hint from the authoritative RuntimeStore state."
                }),
            ));
        }
        let initialized = runtime_attestation.as_ref().is_some_and(|attestation| {
            attestation.app_root_materialization_state == AppRootMaterializationState::Verified
        });
        let build_status = latest_build
            .as_ref()
            .and_then(|build| build.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("not_started");
        let dev_status = preview
            .as_ref()
            .and_then(|preview| preview.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("not_started");
        let next_action = if !initialized {
            json!({
                "tool": "project.init",
                "reason": "the declared App Root has not been materialized"
            })
        } else if runtime_attestation
            .as_ref()
            .is_some_and(|attestation| !attestation.style_contract_state.is_verified())
        {
            json!({
                "tool": "project.init",
                "reason": "the Runtime-owned Style Contract identity must be restored"
            })
        } else if build_status != "success" {
            json!({
                "tool": "fs.read",
                "reason": "the Editable Surface is ready for the requested source changes"
            })
        } else if dev_status != "ready" {
            json!({
                "tool": "preview.dev_start",
                "reason": "the latest successful source revision is not Ready in Preview"
            })
        } else {
            json!({
                "tool": "draft.snapshot_create",
                "reason": "the current Ready revision must be persisted as a durable DraftSnapshot"
            })
        };

        Ok(ToolResult::ok(json!({
            "appRoot": format!("/workspace/{}", app_root_relative.to_string_lossy().replace('\\', "/")),
            "appRootRelative": app_root_relative.to_string_lossy().replace('\\', "/"),
            "packageManager": package_manager,
            "framework": project.as_ref().and_then(|state| state.get("framework")).cloned().unwrap_or(Value::Null),
            "templateKey": project.as_ref().and_then(|state| state.get("templateKey")).cloned().unwrap_or(Value::Null),
            "project": project,
            "projectHint": project_hint,
            "projectStateConflict": false,
            "package": package_json,
            "keySourceFiles": key_source_files,
            "editableSurface": editable_surface,
            "styleContractPath": if style_contract.is_some() { json!("/workspace/state/style-contract.json") } else { Value::Null },
            "styleContract": style_contract,
            "runtimeAttestation": runtime_attestation,
            "lifecycle": {
                "initialized": initialized,
                "buildStatus": build_status,
                "devStatus": dev_status,
                "sourceRevision": ctx.run.project_state_snapshot.as_ref().map(|state| state.revision),
            },
            "nextAction": next_action,
            "latestBuild": latest_build,
            "dependencyState": dependency_state,
            "preview": preview,
            "browser": browser,
        })))
    }
}
