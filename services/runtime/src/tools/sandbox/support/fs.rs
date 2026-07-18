use super::*;

pub(super) fn path_schema() -> Value {
    object_schema(
        json!({ "path": string_schema("Workspace path") }),
        &["path"],
    )
}

pub(super) fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Recoverable(format!("missing {key}")))
}

pub(super) fn required_u64(input: &Value, key: &str) -> Result<u64, ToolError> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::Recoverable(format!("missing numeric {key}")))
}

pub(super) fn required_u64_validation(
    input: &Value,
    key: &str,
    tool: &str,
) -> Result<u64, ValidationError> {
    input.get(key).and_then(Value::as_u64).ok_or_else(|| {
        ValidationError::with_kind(
            format!("{tool} requires numeric {key}"),
            "tool.input_schema_invalid",
        )
    })
}

pub(super) fn require_string(input: &Value, key: &str, tool: &str) -> Result<(), ValidationError> {
    if input.get(key).and_then(Value::as_str).is_some() {
        return Ok(());
    }
    Err(ValidationError::new(format!("{tool} requires {key}")))
}

pub(super) fn validate_write_payload_budget(
    input: &Value,
    tool: &str,
) -> Result<(), ValidationError> {
    let serialized_bytes = serde_json::to_vec(input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    let text_chars = input
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    if serialized_bytes > MAX_DIRECT_WRITE_ARGUMENT_BYTES
        || text_chars > MAX_DIRECT_WRITE_TEXT_CHARS
    {
        return Err(
            ValidationError::with_kind(LARGE_WRITE_GUIDANCE, "tool.input_too_large").with_metadata(
                json!({
                    "tool": tool,
                    "path": input.get("path").and_then(Value::as_str).unwrap_or("unknown"),
                    "inputChars": text_chars,
                    "serializedBytes": serialized_bytes,
                    "maxInputChars": MAX_DIRECT_WRITE_TEXT_CHARS,
                    "maxSerializedBytes": MAX_DIRECT_WRITE_ARGUMENT_BYTES,
                    "guidance": LARGE_WRITE_GUIDANCE,
                }),
            ),
        );
    }
    Ok(())
}

pub(super) fn validate_chunk_payload_budget(input: &Value) -> Result<(), ValidationError> {
    let serialized_bytes = serde_json::to_vec(input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    let text_chars = input
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    if serialized_bytes > MAX_CHUNK_ARGUMENT_BYTES || text_chars > MAX_CHUNK_TEXT_CHARS {
        return Err(ValidationError::with_kind(
            "fs.write_chunk input too large. Split the file into smaller chunks before retrying.",
            "tool.input_too_large",
        )
        .with_metadata(json!({
            "tool": "fs.write_chunk",
            "path": input.get("path").and_then(Value::as_str).unwrap_or("unknown"),
            "inputChars": text_chars,
            "serializedBytes": serialized_bytes,
            "maxInputChars": MAX_CHUNK_TEXT_CHARS,
            "maxSerializedBytes": MAX_CHUNK_ARGUMENT_BYTES,
            "guidance": "Split the file into smaller chunks before retrying fs.write_chunk.",
        })));
    }
    Ok(())
}

pub(super) fn validate_chunk_bounds(index: u64, total: u64) -> Result<(), ValidationError> {
    if total == 0 || total > MAX_CHUNKS_PER_WRITE || index >= total {
        return Err(ValidationError::with_kind(
            format!(
                "chunk bounds invalid: index={index}, total={total}, max={MAX_CHUNKS_PER_WRITE}"
            ),
            "tool.input_schema_invalid",
        ));
    }
    Ok(())
}

pub(super) fn validate_chunk_bounds_tool(index: u64, total: u64) -> Result<(), ToolError> {
    if total == 0 || total > MAX_CHUNKS_PER_WRITE || index >= total {
        return Err(ToolError::Recoverable(format!(
            "chunk bounds invalid: index={index}, total={total}, max={MAX_CHUNKS_PER_WRITE}"
        )));
    }
    Ok(())
}

pub(super) fn safe_session_id(session_id: &str) -> Result<String, ToolError> {
    let sanitized = session_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    if sanitized.is_empty() {
        return Err(ToolError::Recoverable(
            "sessionId must contain at least one ASCII letter, number, '-' or '_'".to_string(),
        ));
    }
    Ok(sanitized)
}

pub(super) fn staged_session_dir(ctx: &ToolContext, session_id: &str) -> PathBuf {
    ctx.workspace_root
        .join("outputs")
        .join("staged-writes")
        .join(session_id)
}

pub(super) fn staged_manifest_path(ctx: &ToolContext, session_id: &str) -> PathBuf {
    staged_session_dir(ctx, session_id).join("manifest.json")
}

pub(super) fn staged_chunk_path(ctx: &ToolContext, session_id: &str, index: u64) -> PathBuf {
    staged_session_dir(ctx, session_id).join(format!("chunk-{index:05}.txt"))
}

pub(super) async fn update_chunk_manifest(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    session_id: &str,
    final_path: &Path,
    index: u64,
    total: u64,
) -> Result<(), ToolError> {
    let manifest_path = staged_manifest_path(ctx, session_id);
    let display_path = display_workspace_path(final_path, ctx);
    let mut manifest = workspace
        .read_to_string(ctx, &manifest_path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| {
            json!({
                "sessionId": session_id,
                "runId": ctx.run.id,
                "path": display_path,
                "total": total,
                "chunks": [],
                "createdAt": Utc::now(),
                "updatedAt": Utc::now(),
            })
        });
    if manifest.get("runId").and_then(Value::as_str) != Some(ctx.run.id.as_str()) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} belongs to another run"
        )));
    }
    if manifest.get("path").and_then(Value::as_str) != Some(display_path.as_str()) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} targets a different path"
        )));
    }
    if manifest.get("total").and_then(Value::as_u64) != Some(total) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} has different total"
        )));
    }
    let chunks = manifest
        .as_object_mut()
        .and_then(|object| object.get_mut("chunks"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ToolError::Recoverable("chunk manifest is invalid".to_string()))?;
    if chunks.iter().any(|value| value.as_u64() == Some(index)) {
        return Err(ToolError::Recoverable(format!(
            "duplicate chunk {index} for session {session_id}"
        )));
    }
    chunks.push(json!(index));
    chunks.sort_by_key(|value| value.as_u64().unwrap_or(u64::MAX));
    manifest["updatedAt"] = json!(Utc::now());
    write_workspace_json_path(workspace, ctx, &manifest_path, &manifest).await
}

pub(super) async fn read_chunk_manifest(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    session_id: &str,
) -> Option<Value> {
    workspace
        .read_to_string(ctx, &staged_manifest_path(ctx, session_id))
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub(super) fn validate_chunk_manifest_for_commit(
    manifest: &Value,
    ctx: &ToolContext,
    final_path: &Path,
    total: u64,
) -> Result<(), ToolError> {
    let display_path = display_workspace_path(final_path, ctx);
    if manifest.get("runId").and_then(Value::as_str) != Some(ctx.run.id.as_str()) {
        return Err(ToolError::Recoverable(
            "chunk session belongs to another run".to_string(),
        ));
    }
    if manifest.get("path").and_then(Value::as_str) != Some(display_path.as_str()) {
        return Err(ToolError::Recoverable(
            "chunk session targets a different path".to_string(),
        ));
    }
    if manifest.get("total").and_then(Value::as_u64) != Some(total) {
        return Err(ToolError::Recoverable(
            "chunk session total does not match commit total".to_string(),
        ));
    }
    let chunks = manifest
        .get("chunks")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::Recoverable("chunk manifest is invalid".to_string()))?;
    for index in 0..total {
        if !chunks.iter().any(|value| value.as_u64() == Some(index)) {
            return Err(ToolError::Recoverable(format!(
                "missing chunk {index}/{total} in session manifest"
            )));
        }
    }
    Ok(())
}

pub(super) async fn cleanup_expired_staged_writes(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    let root = ctx.workspace_root.join("outputs/staged-writes");
    let Ok(entries) = workspace.list_dir(ctx, &root).await else {
        return Ok(());
    };
    for entry in entries {
        if entry.kind != WorkspaceEntryKind::Dir {
            continue;
        }
        let manifest_path = entry.path.join("manifest.json");
        let Some(manifest) = workspace
            .read_to_string(ctx, &manifest_path)
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let Some(updated_at) = manifest.get("updatedAt").and_then(Value::as_str) else {
            continue;
        };
        let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at) else {
            continue;
        };
        if Utc::now()
            .signed_duration_since(updated_at.with_timezone(&Utc))
            .num_seconds()
            > STAGED_WRITE_TTL_SECS
        {
            let _ = workspace.remove_dir_all(ctx, &entry.path).await;
        }
    }
    Ok(())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub(super) fn resolve_path(path: &str, workspace_root: &Path) -> PathBuf {
    if path == "/workspace" {
        return workspace_root.to_path_buf();
    }
    if let Some(relative) = path.strip_prefix("/workspace/") {
        return workspace_root.join(relative);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

pub(super) fn validate_workspace_path_input(
    input: &Value,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError::new(format!("{tool_name} requires path")))?;
    let resolved = resolve_path(path, &ctx.workspace_root);
    match check_context_workspace_path(&resolved, ctx) {
        Ok(_) => Ok(()),
        Err(PermissionError::SecretPath(_)) => Ok(()),
        Err(error) => Err(path_validation_error(tool_name, path, &resolved, error)),
    }
}

pub(super) fn path_validation_error(
    tool_name: &str,
    received_path: &str,
    resolved: &Path,
    error: PermissionError,
) -> ValidationError {
    let (error_kind, guidance, suggested_path) = match error {
        PermissionError::ExternalDirectory(_) => (
            "path.external_directory",
            "Use workspace-relative paths such as project/src/pages/index.astro.",
            Some("project"),
        ),
        PermissionError::InvalidPathComponent(_) => (
            "path.invalid_component",
            "Remove '..' or other invalid path components and stay inside the workspace.",
            Some("project"),
        ),
        PermissionError::SecretPath(_) => (
            "path.secret",
            "Choose a non-secret project source path.",
            None,
        ),
        PermissionError::CannotResolve(_) => (
            "path.cannot_resolve",
            "Use an existing workspace path or a creatable path under the project app root.",
            Some("project"),
        ),
    };
    ValidationError::with_kind(
        format!("{tool_name} path is not usable: {received_path}"),
        error_kind,
    )
    .with_metadata(json!({
        "tool": tool_name,
        "receivedPath": received_path,
        "resolvedPath": resolved.display().to_string(),
        "suggestedPath": suggested_path,
        "guidance": guidance,
    }))
}

pub(super) fn validate_project_mutation_write_path(
    input: &Value,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return Ok(());
    };
    let resolved = resolve_path(path, &ctx.workspace_root);
    let violation = check_project_write_path(ctx, &resolved).map_err(|violation| {
        ValidationError::with_kind(
            format!(
                "{tool_name} cannot write a path forbidden by template {}: {path}",
                violation.template_id
            ),
            violation.error_kind,
        )
        .with_metadata(json!({
            "tool": tool_name,
            "receivedPath": path,
            "resolvedPath": display_workspace_path(&violation.path, ctx),
            "appRoot": display_workspace_path(&violation.app_root, ctx),
            "forbiddenPaths": violation.forbidden_roots.iter().map(|root| display_workspace_path(root, ctx)).collect::<Vec<_>>(),
            "suggestedAction": violation.guidance,
        }))
    });
    violation
}

pub(super) fn typed_recoverable(
    message: impl Into<String>,
    error_kind: impl Into<String>,
    metadata: Value,
) -> ToolError {
    ToolError::typed_recoverable(message, error_kind, metadata)
}

pub(super) fn style_validation_error(message: impl Into<String>) -> ValidationError {
    ValidationError::with_kind(message, "style.input_invalid").with_metadata(json!({
        "suggestedAction": "Pass a non-empty tokens object using token names declared in state/style-contract.json."
    }))
}

pub(super) fn style_typed_recoverable(
    message: impl Into<String>,
    error_kind: impl Into<String>,
    metadata: Value,
) -> ToolError {
    let mut metadata = metadata;
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "tool".to_string(),
            Value::String("style.update_tokens".to_string()),
        );
    }
    typed_recoverable(message, error_kind, metadata)
}

pub(super) fn patch_recovery_metadata(
    ctx: &ToolContext,
    path: &Path,
    old_str: &str,
    match_count: usize,
    content: Option<&str>,
) -> Value {
    json!({
        "path": display_workspace_path(path, ctx),
        "oldStrPreview": old_str.chars().take(160).collect::<String>(),
        "matchCount": match_count,
        "suggestedAction": if match_count > 1 {
            "Provide a larger unique oldStr or set replaceAll=true when every occurrence should change."
        } else {
            "Read the file again and retry with a small exact snippet from current contents."
        },
        "nearestSnippets": content
            .map(|content| nearest_patch_snippets(content, old_str))
            .unwrap_or_default(),
    })
}

pub(super) fn nearest_patch_snippets(content: &str, old_str: &str) -> Vec<Value> {
    let needle = old_str
        .split_whitespace()
        .max_by_key(|part| part.len())
        .unwrap_or("")
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_');
    if needle.len() < 3 {
        return Vec::new();
    }
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .take(3)
        .map(|(index, line)| {
            json!({
                "line": index + 1,
                "text": line.trim().chars().take(240).collect::<String>(),
            })
        })
        .collect()
}

pub(super) fn checked_existing_path(
    input: &Value,
    ctx: &ToolContext,
) -> Result<PathBuf, ToolError> {
    let path = required_str(input, "path")?;
    check_context_workspace_path(&resolve_path(path, &ctx.workspace_root), ctx)
        .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
}

pub(super) fn checked_write_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let path = required_str(input, "path")?;
    let path = resolve_path(path, &ctx.workspace_root);
    check_context_workspace_path(&path, ctx)
        .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
        .and_then(|path| {
            ensure_runtime_owned_path_not_mutated(&path, ctx)?;
            ensure_not_nested_package_root(&path, ctx)?;
            ensure_project_mutation_write_path(&path, ctx)?;
            Ok(path)
        })
}

pub(super) fn checked_delete_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, String> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "fs.delete requires path".to_string())?;
    let path = check_context_workspace_path(&resolve_path(path, &ctx.workspace_root), ctx)
        .map_err(|error| format!("{error:?}"))?;
    let app_root = if ctx.remote_workspace {
        check_lexical_workspace_path(&default_project_dir(ctx), &ctx.workspace_root)
            .map_err(|error| format!("{error:?}"))?
    } else {
        // remote-fs-boundary: allow-begin local-delete-root-resolution
        fs::canonicalize(default_project_dir(ctx)).map_err(|error| error.to_string())?
        // remote-fs-boundary: allow-end local-delete-root-resolution
    };
    if path == ctx.workspace_root
        || path == app_root
        || path == ctx.workspace_root.join("inputs")
        || path == ctx.workspace_root.join("state")
        || path == ctx.workspace_root.join("outputs")
        || !path.starts_with(&app_root)
    {
        return Err(format!(
            "fs.delete is limited to non-root paths under {}",
            display_workspace_path(&app_root, ctx)
        ));
    }
    Ok(path)
}

pub(super) fn check_existing_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    match input
        .get("path")
        .and_then(Value::as_str)
        .map(|path| check_context_workspace_path(&resolve_path(path, &ctx.workspace_root), ctx))
    {
        Some(Ok(_)) => allow_with_input(input, "workspace path allowed"),
        Some(Err(error)) => deny(tool, format!("{error:?}")),
        None => deny(tool, "missing path"),
    }
}

pub(super) fn check_existing_write_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    match input
        .get("path")
        .and_then(Value::as_str)
        .map(|path| check_context_workspace_path(&resolve_path(path, &ctx.workspace_root), ctx))
    {
        Some(Ok(path)) => {
            if let Err(error) = ensure_runtime_owned_path_not_mutated(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_not_nested_package_root(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_project_mutation_write_path(&path, ctx) {
                deny(tool, error.message())
            } else {
                allow_with_input(input, "workspace edit path allowed")
            }
        }
        Some(Err(error)) => deny(tool, format!("{error:?}")),
        None => deny(tool, "missing path"),
    }
}

pub(super) fn check_write_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return deny(tool, "missing path");
    };
    let path = resolve_path(path, &ctx.workspace_root);
    let result = check_context_workspace_path(&path, ctx);
    match result {
        Ok(path) => {
            if let Err(error) = ensure_runtime_owned_path_not_mutated(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_not_nested_package_root(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_project_mutation_write_path(&path, ctx) {
                deny(tool, error.message())
            } else {
                allow_with_input(input, "workspace write path allowed")
            }
        }
        Err(error) => deny(tool, format!("{error:?}")),
    }
}

pub(super) fn check_context_workspace_path(
    path: &Path,
    ctx: &ToolContext,
) -> Result<PathBuf, PermissionError> {
    if ctx.remote_workspace {
        check_lexical_workspace_path(path, &ctx.workspace_root)
    } else {
        check_workspace_path(path, &ctx.workspace_root)
    }
}

pub(super) fn ensure_runtime_owned_path_not_mutated(
    path: &Path,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    if ctx.allow_runtime_owned_writes {
        return Ok(());
    }
    let relative = path.strip_prefix(&ctx.workspace_root).map_err(|_| {
        ToolError::PermissionDenied("path must stay inside the workspace".to_string())
    })?;
    let runtime_owned = relative.starts_with("state")
        || relative.starts_with("outputs/build")
        || relative.starts_with("outputs/candidates")
        || relative.starts_with("outputs/artifacts")
        || relative.starts_with("outputs/screenshots")
        || relative.starts_with("outputs/tool-results");
    if runtime_owned {
        return Err(typed_recoverable(
            format!(
                "runtime-owned path cannot be mutated with generic fs tools: {}",
                display_workspace_path(path, ctx)
            ),
            "path.runtime_owned",
            json!({
                "path": display_workspace_path(path, ctx),
                "suggestedAction": "Use the dedicated project, style, preview, build, or artifact tool that owns this state."
            }),
        ));
    }
    Ok(())
}

pub(super) fn allow_with_input(input: &Value, reason: impl Into<String>) -> PermissionResult {
    PermissionResult::Allow {
        updated_input: input.clone(),
        reason: PermissionReason::Other {
            reason: reason.into(),
        },
    }
}

pub(super) fn deny(tool: &str, reason: impl Into<String>) -> PermissionResult {
    let reason = reason.into();
    PermissionResult::Deny {
        message: format!("{tool} denied: {reason}"),
        reason: PermissionReason::Rule {
            source: RuleSource::Runtime,
            rule_content: reason,
        },
    }
}

pub(super) async fn collect_search_matches(
    workspace: &dyn WorkspaceBackend,
    path: &Path,
    ctx: &ToolContext,
    query: &str,
    matches: &mut Vec<Value>,
) -> Result<SearchSummary, ToolError> {
    const MAX_SEARCH_FILES: usize = 256;
    const MAX_SEARCH_BYTES: usize = 4 * 1024 * 1024;
    const MAX_SEARCH_MATCHES: usize = 200;
    const MAX_MATCH_TEXT_CHARS: usize = 1_000;

    let mut stack = vec![path.to_path_buf()];
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0usize;
    let mut skipped_paths = 0usize;
    let mut truncated = false;

    while let Some(path) = stack.pop() {
        if search_ignored_path(&path, ctx) {
            skipped_paths += 1;
            continue;
        }
        match workspace
            .path_kind(ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?
        {
            WorkspacePathKind::File => {
                if files_scanned >= MAX_SEARCH_FILES || bytes_scanned >= MAX_SEARCH_BYTES {
                    truncated = true;
                    break;
                }
                let text = workspace
                    .read_to_string(ctx, &path)
                    .await
                    .unwrap_or_default();
                files_scanned += 1;
                bytes_scanned = bytes_scanned.saturating_add(text.len());
                for (index, line) in text.lines().enumerate() {
                    if line.contains(query) {
                        matches.push(json!({
                            "path": display_workspace_path(&path, ctx),
                            "line": index + 1,
                            "text": line.chars().take(MAX_MATCH_TEXT_CHARS).collect::<String>(),
                        }));
                        if matches.len() >= MAX_SEARCH_MATCHES {
                            truncated = true;
                            break;
                        }
                    }
                }
                if truncated || bytes_scanned >= MAX_SEARCH_BYTES {
                    truncated = truncated || !stack.is_empty();
                    break;
                }
            }
            WorkspacePathKind::Dir => {
                for entry in workspace
                    .list_dir(ctx, &path)
                    .await
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?
                {
                    stack.push(entry.path);
                }
            }
        }
    }
    Ok(SearchSummary {
        files_scanned,
        bytes_scanned,
        skipped_paths,
        truncated,
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SearchSummary {
    pub files_scanned: usize,
    pub bytes_scanned: usize,
    pub skipped_paths: usize,
    pub truncated: bool,
}

fn search_ignored_path(path: &Path, ctx: &ToolContext) -> bool {
    let relative = path.strip_prefix(&ctx.workspace_root).unwrap_or(path);
    relative.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(
                ".git"
                    | ".next"
                    | ".cache"
                    | ".turbo"
                    | "node_modules"
                    | "dist"
                    | "build"
                    | "coverage"
                    | "target"
            )
        )
    })
}
