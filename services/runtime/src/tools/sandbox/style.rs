use super::*;

pub(super) fn style_update_tokens_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(StyleUpdateTokensTool { workspace })
}

struct StyleUpdateTokensTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for StyleUpdateTokensTool {
    fn name(&self) -> &'static str {
        "style.update_tokens"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "tokens": {
                    "type": "object",
                    "description": "Patch map of style contract token names to CSS values, for example color.primary -> #f37a0a.",
                    "additionalProperties": { "type": "string" }
                }
            }),
            &["tokens"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let tokens = input
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| style_validation_error("style.update_tokens requires tokens object"))?;
        if tokens.is_empty() {
            return Err(style_validation_error(
                "style.update_tokens requires at least one token",
            ));
        }
        for (name, value) in tokens {
            if name.trim().is_empty() {
                return Err(style_validation_error(
                    "style.update_tokens token names must be non-empty",
                ));
            }
            let Some(value) = value.as_str() else {
                return Err(style_validation_error(
                    "style.update_tokens token values must be strings",
                ));
            };
            validate_style_token_value(value).map_err(|message| {
                style_validation_error(format!("style.update_tokens {message}"))
            })?;
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "style token update allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let contract = read_workspace_json(&*self.workspace, &ctx, "state/style-contract.json")
            .await
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens requires state/style-contract.json; initialize the project first",
                    "style.contract_missing",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "suggestedAction": "Run project.init or project.inspect before style.update_tokens so the runtime style contract exists."
                    }),
                )
            })?;
        let token_file = contract
            .get("tokenFile")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens style contract is missing tokenFile",
                    "style.contract_invalid",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "missingField": "tokenFile",
                        "suggestedAction": "Regenerate the project style contract with project.init or repair state/style-contract.json."
                    }),
                )
            })?;
        let token_path = check_context_workspace_path(
            &resolve_path(token_file, &ctx.workspace_root),
            &ctx,
        )
        .map_err(|error| {
            style_typed_recoverable(
                format!("style.update_tokens tokenFile is not readable: {error:?}"),
                "style.token_file_unavailable",
                json!({
                    "tokenFile": token_file,
                    "suggestedAction": "Ensure the contract tokenFile points to an existing workspace token CSS file."
                }),
            )
        })?;
        let contract_tokens = contract
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens style contract is missing tokens map",
                    "style.contract_invalid",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "missingField": "tokens",
                        "suggestedAction": "Regenerate the project style contract with project.init or repair state/style-contract.json."
                    }),
                )
            })?;
        let requested = input
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens requires tokens object",
                    "style.input_invalid",
                    json!({
                        "suggestedAction": "Pass a tokens object such as {\"color.primary\":\"#f37a0a\"}."
                    }),
                )
            })?;

        let mut content = self
            .workspace
            .read_to_string(&ctx, &token_path)
            .await
            .map_err(|error| {
                style_typed_recoverable(
                    format!("style.update_tokens failed to read token file: {error}"),
                    "style.token_file_unavailable",
                    json!({
                        "tokenFile": display_workspace_path(&token_path, &ctx),
                        "suggestedAction": "Ensure the token file exists and is readable before updating style tokens."
                    }),
                )
            })?;
        let mut changes = Vec::new();
        for (token_name, value) in requested {
            let Some(css_variable) = contract_tokens.get(token_name).and_then(Value::as_str) else {
                return Err(style_typed_recoverable(
                    format!(
                        "style.update_tokens unknown token {token_name}; use one of the tokens declared in state/style-contract.json"
                    ),
                    "style.token_unknown",
                    json!({
                        "token": token_name,
                        "contractPath": "/workspace/state/style-contract.json",
                        "availableTokens": contract_tokens.keys().cloned().collect::<Vec<_>>(),
                        "suggestedAction": "Call project.inspect and update only tokens declared in state/style-contract.json."
                    }),
                ));
            };
            let new_value = value.as_str().ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens token values must be strings",
                    "style.input_invalid",
                    json!({
                        "token": token_name,
                        "suggestedAction": "Pass CSS token values as strings."
                    }),
                )
            })?;
            validate_style_token_value(new_value).map_err(|message| {
                style_typed_recoverable(
                    format!("style.update_tokens {message}"),
                    "style.token_value_invalid",
                    json!({
                        "token": token_name,
                        "value": new_value,
                        "suggestedAction": "Use a simple CSS token value without semicolons, braces, or newlines."
                    }),
                )
            })?;
            let (updated, old_value) =
                replace_css_variable_value(&content, css_variable, new_value, &ctx, &token_path)?;
            content = updated;
            changes.push(json!({
                "token": token_name,
                "cssVariable": css_variable,
                "before": old_value,
                "after": new_value,
            }));
        }

        preview_dev::validate_dev_mutation(&ctx)?;
        self.workspace
            .write_string(&ctx, &token_path, &content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&*self.workspace, &ctx, &token_path, &content).await?;
        let draft_preview = preview_dev::record_dev_mutation(&*self.workspace, &ctx).await;
        Ok(ToolResult::ok(json!({
            "tokenFile": display_workspace_path(&token_path, &ctx),
            "updated": true,
            "changes": changes,
            "draftPreview": draft_preview,
        })))
    }
}
