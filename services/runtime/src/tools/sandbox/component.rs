use super::*;
use crate::{component_registry, types::sha256_hex};

pub(super) fn component_search_tool() -> Arc<dyn Tool> {
    Arc::new(ComponentSearchTool)
}

pub(super) fn component_inspect_tool() -> Arc<dyn Tool> {
    Arc::new(ComponentInspectTool)
}

pub(super) fn component_install_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(ComponentInstallTool { workspace })
}

struct ComponentSearchTool;
struct ComponentInspectTool;
struct ComponentInstallTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ComponentSearchTool {
    fn name(&self) -> &'static str {
        "component.search"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "query": string_schema("Name, description, or capability query") }),
            &[],
        )
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "frozen internal component registry search allowed")
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let query = input.get("query").and_then(Value::as_str).unwrap_or("");
        let items = component_registry::search(query)
            .into_iter()
            .map(|item| {
                json!({
                    "name": item.name,
                    "title": item.title,
                    "description": item.description,
                    "registryVersion": item.registry_version,
                    "contentHash": item.content_hash,
                    "license": item.license,
                    "compatibleTemplates": item.compatible_templates,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::ok(json!({
            "registryVersion": component_registry::COMPONENT_REGISTRY_VERSION,
            "items": items,
        })))
    }
}

#[async_trait]
impl Tool for ComponentInspectTool {
    fn name(&self) -> &'static str {
        "component.inspect"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "name": string_schema("Registry item name") }),
            &["name"],
        )
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "frozen registry item inspection allowed")
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let name = required_str(&input, "name")?;
        let item = component_registry::get(name).ok_or_else(|| {
            typed_recoverable(
                format!("component registry item not found: {name}"),
                "component.not_found",
                json!({ "blocking": false }),
            )
        })?;
        Ok(ToolResult::ok(json!({ "item": item })))
    }
}

#[async_trait]
impl Tool for ComponentInstallTool {
    fn name(&self) -> &'static str {
        "component.install"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "name": string_schema("Registry item name"),
                "expectedContentHash": string_schema("Hash returned by component.inspect"),
                "overwrite": { "type": "boolean", "default": false }
            }),
            &["name", "expectedContentHash"],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(
            input,
            "hash-pinned internal shadcn registry install allowed",
        )
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        ensure_next_app(&ctx)?;
        preview_dev::validate_dev_mutation(&ctx)?;
        let name = required_str(&input, "name")?;
        let expected_hash = required_str(&input, "expectedContentHash")?;
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let item = component_registry::get(name).ok_or_else(|| {
            typed_recoverable(
                format!("component registry item not found: {name}"),
                "component.not_found",
                json!({ "blocking": false }),
            )
        })?;
        if item.content_hash != expected_hash {
            return Err(typed_recoverable(
                "component registry item changed after inspection",
                "component.registry_stale",
                json!({
                    "expectedContentHash": expected_hash,
                    "currentContentHash": item.content_hash,
                    "blocking": true,
                }),
            ));
        }
        let mut diff = Vec::new();
        for file in &item.files {
            if !file.target.starts_with("components/ui/") {
                return Err(ToolError::Terminal(
                    "component registry target escaped components/ui".to_string(),
                ));
            }
            let path = default_project_dir(&ctx).join(file.target);
            let before = match self.workspace.read_bytes(&ctx, &path).await {
                Ok(bytes) => Some(sha256_hex(&bytes)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(ToolError::Recoverable(error.to_string())),
            };
            if before.as_deref() == Some(file.sha256.as_str()) {
                diff.push(json!({
                    "path": file.target,
                    "action": "unchanged",
                    "beforeHash": before,
                    "afterHash": file.sha256,
                }));
                continue;
            }
            if before.is_some() && !overwrite {
                return Err(typed_recoverable(
                    format!("component target already differs: {}", file.target),
                    "component.overwrite_confirmation_required",
                    json!({
                        "blocking": true,
                        "diff": [{
                            "path": file.target,
                            "action": "overwrite",
                            "beforeHash": before,
                            "afterHash": file.sha256,
                        }]
                    }),
                ));
            }
            self.workspace
                .write_string(&ctx, &path, file.content)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?;
            diff.push(json!({
                "path": file.target,
                "action": if before.is_some() { "overwrite" } else { "create" },
                "beforeHash": before,
                "afterHash": file.sha256,
            }));
        }
        let changed = diff.iter().any(|entry| entry["action"] != "unchanged");
        let draft_preview = if changed {
            preview_dev::record_dev_mutation(&*self.workspace, &ctx).await
        } else {
            None
        };
        Ok(ToolResult::ok(json!({
            "installed": item.name,
            "registryVersion": item.registry_version,
            "contentHash": item.content_hash,
            "license": item.license,
            "source": item.source,
            "securityScan": item.security_scan,
            "diff": diff,
            "dependencyDiff": [],
            "sourceContract": "pass",
            "dependencyPolicy": "pass",
            "draftPreview": draft_preview,
        })))
    }
}

fn ensure_next_app(ctx: &ToolContext) -> Result<(), ToolError> {
    let supported = ctx
        .run
        .project_state_snapshot
        .as_ref()
        .is_some_and(|state| {
            state.template_key == "next-app" && state.template_version == "next-app@1"
        });
    if supported {
        Ok(())
    } else {
        Err(typed_recoverable(
            "component registry item is incompatible with this template",
            "component.template_incompatible",
            json!({ "blocking": false, "compatibleTemplates": ["next-app@1"] }),
        ))
    }
}
