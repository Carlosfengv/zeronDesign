use super::*;
use crate::{
    project_asset::ProjectAssetStore,
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::{ProjectAssetSource, VisualArtifactOrigin},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

const MAX_ASSET_PROVIDER_RESPONSE_BYTES: usize = 12 * 1024 * 1024;
const ASSET_PROVIDER_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const ASSET_PROVIDER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

pub(super) fn asset_import_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(AssetImportTool { workspace })
}

pub(super) fn asset_list_tool() -> Arc<dyn Tool> {
    Arc::new(AssetListTool)
}

pub(super) fn asset_generate_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(AssetGenerateTool { workspace })
}

struct AssetImportTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

struct AssetListTool;
struct AssetGenerateTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for AssetImportTool {
    fn name(&self) -> &'static str {
        "asset.import"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "artifactId": string_schema("Uploaded or generated VisualArtifact id"),
                "name": string_schema("Stable human-readable asset name"),
                "altText": string_schema("Required accessible alternative text"),
                "license": string_schema("License or user-owned declaration")
            }),
            &["artifactId", "name", "altText", "license"],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "content-addressed project asset import allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        ensure_next_app(&ctx, self.name())?;
        preview_dev::validate_dev_mutation(&ctx)?;
        let artifact_id = required_str(&input, "artifactId")?;
        let name = asset_name(required_str(&input, "name")?)?;
        let alt_text = required_str(&input, "altText")?.trim().to_string();
        let license = required_str(&input, "license")?.trim().to_string();
        if alt_text.is_empty() || license.is_empty() {
            return Err(typed_recoverable(
                "asset.import requires non-empty altText and license",
                "tool.input_schema_invalid",
                json!({}),
            ));
        }
        let visual_store =
            VisualArtifactStore::open(ctx.runtime_storage_dir.join("visual-artifacts"))
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let artifact = visual_store
            .get(artifact_id)
            .map_err(|error| ToolError::Terminal(error.to_string()))?
            .filter(|artifact| artifact.project_id == ctx.project_id)
            .ok_or_else(|| {
                typed_recoverable(
                    format!("VisualArtifact not found: {artifact_id}"),
                    "asset.source_not_found",
                    json!({}),
                )
            })?;
        let source = match artifact.origin {
            VisualArtifactOrigin::Upload => ProjectAssetSource::Upload,
            VisualArtifactOrigin::Generated => ProjectAssetSource::Generated,
            VisualArtifactOrigin::Browser => {
                return Err(typed_recoverable(
                    "browser evidence cannot be imported as a project asset",
                    "asset.source_invalid",
                    json!({}),
                ));
            }
        };
        let bytes = visual_store
            .read_content(&artifact.id)
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let target_path = format!("public/assets/{}-{name}.png", &artifact.sha256[..16]);
        self.workspace
            .write_bytes(&ctx, &default_project_dir(&ctx).join(&target_path), &bytes)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let store = ProjectAssetStore::open(&ctx.runtime_storage_dir)
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let project_asset = store
            .create(
                &ctx.project_id,
                artifact.id.clone(),
                source,
                target_path.clone(),
                artifact.sha256.clone(),
                license,
                json!({
                    "visualArtifactOrigin": artifact.origin,
                    "originMetadata": artifact.origin_metadata,
                    "storage": "runtime-owned-content-addressed",
                }),
                artifact.width,
                artifact.height,
                alt_text,
                Some(ctx.run.id.clone()),
            )
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let draft_preview = preview_dev::record_dev_mutation(&*self.workspace, &ctx).await;
        Ok(ToolResult::ok(json!({
            "asset": project_asset,
            "targetPath": target_path,
            "draftPreview": draft_preview,
        })))
    }
}

#[async_trait]
impl Tool for AssetListTool {
    fn name(&self) -> &'static str {
        "asset.list"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project asset provenance read allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let store = ProjectAssetStore::open(&ctx.runtime_storage_dir)
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        Ok(ToolResult::ok(json!({
            "assets": store.list_project(&ctx.project_id),
        })))
    }
}

#[async_trait]
impl Tool for AssetGenerateTool {
    fn name(&self) -> &'static str {
        "asset.generate"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "prompt": string_schema("Photography, illustration, or texture request"),
                "width": { "type": "integer", "minimum": 64, "maximum": 4096 },
                "height": { "type": "integer", "minimum": 64, "maximum": 4096 },
                "crop": string_schema("Crop behavior"),
                "altText": string_schema("Accessible alternative text")
            }),
            &["prompt", "width", "height", "crop", "altText"],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "optional visual asset generation request allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        ensure_next_app(&ctx, self.name())?;
        preview_dev::validate_dev_mutation(&ctx)?;
        let endpoint = std::env::var("ASSET_GENERATION_PROVIDER_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(asset_provider_unavailable)?;
        let prompt = required_str(&input, "prompt")?;
        let width = input.get("width").and_then(Value::as_u64).ok_or_else(|| {
            typed_recoverable(
                "asset.generate width is required",
                "tool.input_schema_invalid",
                json!({}),
            )
        })?;
        let height = input.get("height").and_then(Value::as_u64).ok_or_else(|| {
            typed_recoverable(
                "asset.generate height is required",
                "tool.input_schema_invalid",
                json!({}),
            )
        })?;
        if !(64..=4096).contains(&width) || !(64..=4096).contains(&height) {
            return Err(typed_recoverable(
                "asset.generate dimensions must be within 64..=4096",
                "tool.input_schema_invalid",
                json!({}),
            ));
        }
        let crop = required_str(&input, "crop")?;
        let alt_text = required_str(&input, "altText")?;
        let client = reqwest::Client::builder()
            .connect_timeout(ASSET_PROVIDER_CONNECT_TIMEOUT)
            .timeout(ASSET_PROVIDER_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                asset_provider_failure(format!(
                    "Asset Generation Provider client setup failed: {error}"
                ))
            })?;
        let mut request = client.post(endpoint).json(&json!({
            "prompt": prompt,
            "width": width,
            "height": height,
            "crop": crop,
            "responseFormat": "base64",
        }));
        if let Ok(token) = std::env::var("ASSET_GENERATION_PROVIDER_AUTH_TOKEN") {
            if !token.trim().is_empty() {
                request = request.bearer_auth(token);
            }
        }
        let response = request.send().await.map_err(|error| {
            asset_provider_failure(format!("Asset Generation Provider request failed: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(asset_provider_failure(format!(
                "Asset Generation Provider returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ASSET_PROVIDER_RESPONSE_BYTES as u64)
        {
            return Err(asset_provider_failure(
                "Asset Generation Provider response exceeded the Runtime limit".to_string(),
            ));
        }
        let response = response.bytes().await.map_err(|error| {
            asset_provider_failure(format!("Asset Generation Provider read failed: {error}"))
        })?;
        if response.len() > MAX_ASSET_PROVIDER_RESPONSE_BYTES {
            return Err(asset_provider_failure(
                "Asset Generation Provider response exceeded the Runtime limit".to_string(),
            ));
        }
        let response: Value = serde_json::from_slice(&response).map_err(|error| {
            asset_provider_failure(format!(
                "Asset Generation Provider returned invalid JSON: {error}"
            ))
        })?;
        let encoded = response
            .get("contentBase64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                asset_provider_failure(
                    "Asset Generation Provider omitted contentBase64".to_string(),
                )
            })?;
        let provider_identity = response
            .get("providerIdentity")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                asset_provider_failure(
                    "Asset Generation Provider omitted providerIdentity".to_string(),
                )
            })?;
        let model_version = response
            .get("modelVersion")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                asset_provider_failure("Asset Generation Provider omitted modelVersion".to_string())
            })?;
        let license = response
            .get("license")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                asset_provider_failure("Asset Generation Provider omitted license".to_string())
            })?;
        let bytes = BASE64_STANDARD.decode(encoded).map_err(|error| {
            asset_provider_failure(format!("generated asset is not valid base64: {error}"))
        })?;
        let visual_store =
            VisualArtifactStore::open(ctx.runtime_storage_dir.join("visual-artifacts"))
                .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let visual = visual_store
            .create_generated(
                &ctx.project_id,
                &bytes,
                std::collections::BTreeMap::from([
                    ("providerIdentity".to_string(), json!(provider_identity)),
                    ("modelVersion".to_string(), json!(model_version)),
                    (
                        "promptHash".to_string(),
                        json!(crate::types::sha256_hex(prompt.as_bytes())),
                    ),
                    ("requestedWidth".to_string(), json!(width)),
                    ("requestedHeight".to_string(), json!(height)),
                    ("crop".to_string(), json!(crop)),
                ]),
            )
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let normalized_bytes = visual_store
            .read_content(&visual.id)
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let target_path = format!("public/assets/{}-generated.png", &visual.sha256[..16]);
        self.workspace
            .write_bytes(
                &ctx,
                &default_project_dir(&ctx).join(&target_path),
                &normalized_bytes,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let store = ProjectAssetStore::open(&ctx.runtime_storage_dir)
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let asset = store
            .create(
                &ctx.project_id,
                visual.id,
                ProjectAssetSource::Generated,
                target_path,
                visual.sha256,
                license.to_string(),
                json!({
                    "providerIdentity": provider_identity,
                    "modelVersion": model_version,
                    "promptHash": crate::types::sha256_hex(prompt.as_bytes()),
                    "responseMode": "inline-base64",
                }),
                visual.width,
                visual.height,
                alt_text.to_string(),
                Some(ctx.run.id.clone()),
            )
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        let draft_preview = preview_dev::record_dev_mutation(&*self.workspace, &ctx).await;
        Ok(ToolResult::ok(json!({
            "asset": asset,
            "partial": false,
            "draftPreview": draft_preview,
        })))
    }
}

fn asset_provider_unavailable() -> ToolError {
    typed_recoverable(
        "no Runtime Asset Generation Provider is configured",
        "asset.provider_unavailable",
        json!({
            "blocking": false,
            "partial": true,
            "fallback": "Continue with uploaded, CSS, or icon assets."
        }),
    )
}

fn asset_provider_failure(message: String) -> ToolError {
    typed_recoverable(
        message,
        "asset.provider_unavailable",
        json!({
            "blocking": false,
            "partial": true,
            "fallback": "Continue without the generated asset or retry the provider later."
        }),
    )
}

fn asset_name(value: &str) -> Result<String, ToolError> {
    let value = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('-');
    if value.is_empty() || value.len() > 64 {
        return Err(typed_recoverable(
            "asset name must contain letters or digits and be at most 64 characters",
            "tool.input_schema_invalid",
            json!({}),
        ));
    }
    Ok(value.to_string())
}

fn ensure_next_app(ctx: &ToolContext, tool: &str) -> Result<(), ToolError> {
    if ctx
        .run
        .project_state_snapshot
        .as_ref()
        .is_some_and(|state| state.template_key == "next-app")
    {
        return Ok(());
    }
    Err(typed_recoverable(
        format!("{tool} is currently available for next-app only"),
        "template.operation_unsupported",
        json!({ "blocking": false }),
    ))
}
