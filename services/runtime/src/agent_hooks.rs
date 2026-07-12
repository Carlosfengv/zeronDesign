use crate::types::AgentPhase;
use serde_json::Value;

pub const DEFAULT_RECOVERY_STRONG_GUIDANCE_ATTEMPT: u32 = 2;
pub const DEFAULT_RECOVERY_PARTIAL_ATTEMPT: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableErrorState {
    pub fingerprint: String,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct ToolFailureObservation {
    pub tool_name: String,
    pub is_error: bool,
    pub content: Value,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ToolSuccessObservation {
    pub tool_name: String,
    pub content: Value,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostToolUseSuccessDecision {
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct PostToolUseSuccessHook;

impl PostToolUseSuccessHook {
    pub fn apply(&self, observation: ToolSuccessObservation) -> PostToolUseSuccessDecision {
        let Some(effect) = success_lifecycle_effect(&observation) else {
            return PostToolUseSuccessDecision::default();
        };
        PostToolUseSuccessDecision {
            metadata: Some(effect),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreToolUseObservation {
    pub phase: AgentPhase,
    pub tool_name: String,
    pub input: Value,
    pub default_cwd: Option<String>,
}

fn success_lifecycle_effect(observation: &ToolSuccessObservation) -> Option<Value> {
    let content = &observation.content;
    let effect = match observation.tool_name.as_str() {
        "fs.read" => serde_json::json!({
            "effect": "read_state_updated",
            "path": content.get("path").cloned().unwrap_or(Value::Null),
        }),
        "package.install" | "project.ensure_dependencies" => serde_json::json!({
            "effect": "dependency_state_updated",
            "packageManager": content.get("packageManager").cloned().unwrap_or(Value::Null),
            "mode": content.get("mode").cloned().unwrap_or(Value::Null),
        }),
        "project.build" => serde_json::json!({
            "effect": "build_state_updated",
            "status": content.get("status").cloned().unwrap_or(Value::Null),
            "sourceSnapshotUri": content.get("sourceSnapshotUri").cloned().unwrap_or(Value::Null),
            "packageManager": content.get("packageManager").cloned().unwrap_or(Value::Null),
        }),
        "style.update_tokens" => serde_json::json!({
            "effect": "style_contract_updated",
            "tokenFile": content.get("tokenFile").cloned().unwrap_or(Value::Null),
            "changed": content.get("changed").cloned().unwrap_or(Value::Null),
        }),
        "preview.start" => serde_json::json!({
            "effect": "preview_state_updated",
            "url": content.get("url").cloned().unwrap_or(Value::Null),
            "status": content.get("status").cloned().unwrap_or(Value::Null),
        }),
        "browser.open" => serde_json::json!({
            "effect": "browser_state_updated",
            "url": content.get("url").cloned().unwrap_or(Value::Null),
            "opened": content.get("opened").cloned().unwrap_or(Value::Null),
        }),
        "browser.screenshot" => serde_json::json!({
            "effect": "screenshot_state_updated",
            "screenshotId": content.get("screenshotId").cloned().unwrap_or(Value::Null),
            "blank": content.get("blank").cloned().unwrap_or(Value::Null),
        }),
        "preview.report_candidate" => serde_json::json!({
            "effect": "promotion_state_updated",
            "versionId": content.get("versionId").cloned().unwrap_or(Value::Null),
            "previewUrl": content.get("previewUrl").cloned().unwrap_or(Value::Null),
        }),
        "preview.publish" => {
            let promotion = content.get("promotion").unwrap_or(&Value::Null);
            serde_json::json!({
                "effect": "promotion_state_updated",
                "versionId": promotion.get("versionId").cloned().unwrap_or(Value::Null),
                "previewUrl": promotion.get("previewUrl").cloned().unwrap_or(Value::Null),
            })
        }
        _ => return None,
    };

    Some(serde_json::json!({
        "postToolUseSuccess": effect,
        "previousMetadata": observation.metadata.clone().unwrap_or(Value::Null),
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreToolUseRejection {
    pub message: String,
    pub error_kind: String,
    pub recoverable: bool,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreToolUseDecision {
    pub input: Value,
    pub rejection: Option<PreToolUseRejection>,
}

#[derive(Debug, Clone, Default)]
pub struct PreToolUseHook;

impl PreToolUseHook {
    pub fn apply(&self, observation: PreToolUseObservation) -> PreToolUseDecision {
        if observation.phase == AgentPhase::Brief
            && is_disallowed_brief_phase_tool(&observation.tool_name)
        {
            return PreToolUseDecision {
                input: observation.input,
                rejection: Some(PreToolUseRejection {
                    message: format!(
                        "tool {} is not allowed during Brief phase",
                        observation.tool_name
                    ),
                    error_kind: "tool.phase_forbidden".to_string(),
                    recoverable: true,
                    metadata: phase_rejection_metadata(
                        observation.phase,
                        &observation.tool_name,
                        "Use content.* and brief.* tools during Brief; wait for Build/Edit before filesystem, browser, package, project, preview, or shell tools.",
                    ),
                }),
            };
        }

        if observation.phase != AgentPhase::Brief && is_brief_write_tool(&observation.tool_name) {
            return PreToolUseDecision {
                input: observation.input,
                rejection: Some(PreToolUseRejection {
                    message: format!(
                        "tool {} is only allowed during Brief phase",
                        observation.tool_name
                    ),
                    error_kind: "tool.phase_forbidden".to_string(),
                    recoverable: true,
                    metadata: phase_rejection_metadata(
                        observation.phase,
                        &observation.tool_name,
                        "Use project.inspect, fs.*, style.update_tokens, project.ensure_dependencies, and preview.publish for Build/Edit lifecycle work instead of Brief tools.",
                    ),
                }),
            };
        }

        if matches!(
            observation.phase,
            AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair
        ) && observation.tool_name == "shell.run"
        {
            if let Some(rejection) = dedicated_shell_tool_rejection(&observation) {
                return PreToolUseDecision {
                    input: observation.input,
                    rejection: Some(rejection),
                };
            }
        }

        let mut observation = observation;
        observation.input = normalize_workspace_path_fields(observation.input);
        let input = inject_default_cwd(observation);
        PreToolUseDecision {
            input,
            rejection: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableErrorFingerprint {
    pub key: String,
    pub tool: String,
    pub error_kind: String,
    pub normalized_path: String,
    pub guidance: String,
}

fn is_brief_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "brief.write_draft" | "brief.update" | "brief.request_confirmation"
    )
}

fn is_disallowed_brief_phase_tool(tool_name: &str) -> bool {
    tool_name.starts_with("fs.")
        || tool_name == "shell.run"
        || tool_name.starts_with("package.")
        || tool_name.starts_with("project.")
        || tool_name.starts_with("preview.")
        || tool_name.starts_with("browser.")
        || tool_name.starts_with("diagnostics.")
        || tool_name.starts_with("style.")
        || tool_name.starts_with("sandbox.")
        || tool_name.starts_with("mcp__")
}

fn inject_default_cwd(observation: PreToolUseObservation) -> Value {
    if !matches!(
        observation.phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair
    ) || !tool_accepts_default_cwd(&observation.tool_name)
    {
        return observation.input;
    }

    let Some(default_cwd) = observation.default_cwd else {
        return observation.input;
    };

    let mut input = observation.input;
    if let Some(object) = input.as_object_mut() {
        object
            .entry("cwd".to_string())
            .or_insert_with(|| Value::String(default_cwd));
    }
    input
}

fn tool_accepts_default_cwd(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "shell.run"
            | "project.build"
            | "project.ensure_dependencies"
            | "package.install"
            | "preview.publish"
    )
}

fn normalize_workspace_path_fields(mut input: Value) -> Value {
    let Some(object) = input.as_object_mut() else {
        return input;
    };
    for key in ["cwd", "path"] {
        let Some(path) = object.get(key).and_then(Value::as_str) else {
            continue;
        };
        let Some(normalized) = normalize_workspace_virtual_path(path) else {
            continue;
        };
        object.insert(key.to_string(), Value::String(normalized));
    }
    input
}

fn normalize_workspace_virtual_path(path: &str) -> Option<String> {
    if path == "/workspace" || path == "workspace" {
        return Some(".".to_string());
    }
    path.strip_prefix("/workspace/")
        .or_else(|| path.strip_prefix("workspace/"))
        .map(str::to_string)
}

fn dedicated_shell_tool_rejection(
    observation: &PreToolUseObservation,
) -> Option<PreToolUseRejection> {
    let argv = observation
        .input
        .get("argv")
        .and_then(Value::as_array)?
        .iter()
        .map(|value| value.as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    let redirect = shell_redirect(&argv)?;
    Some(PreToolUseRejection {
        message: format!(
            "shell.run command {} must use {}",
            argv.join(" "),
            redirect.tool_description
        ),
        error_kind: "shell.command_denied".to_string(),
        recoverable: true,
        metadata: serde_json::json!({
            "phase": format!("{:?}", observation.phase),
            "tool": observation.tool_name,
            "argv": argv,
            "suggestedTool": redirect.suggested_tool,
            "suggestedAction": redirect.suggested_action,
        }),
    })
}

struct ShellRedirect {
    suggested_tool: &'static str,
    suggested_action: &'static str,
    tool_description: &'static str,
}

fn shell_redirect(argv: &[String]) -> Option<ShellRedirect> {
    if is_package_manager_install(argv) {
        return Some(ShellRedirect {
            suggested_tool: "project.ensure_dependencies",
            suggested_action: "Use project.ensure_dependencies({\"mode\":\"restore\"}) for package.json installs, or project.ensure_dependencies({\"mode\":\"add\",\"packages\":[...]}) for new dependencies.",
            tool_description: "the runtime dependency tools",
        });
    }

    if is_preview_server_command(argv) {
        return Some(ShellRedirect {
            suggested_tool: "preview.start",
            suggested_action: "Use preview.publish for the normal lifecycle, or preview.start after a successful project.build when debugging preview startup.",
            tool_description: "the runtime preview tools",
        });
    }

    if is_interactive_project_scaffold(argv) {
        return Some(ShellRedirect {
            suggested_tool: "project.init",
            suggested_action: "Use project.init for templates, project.ensure_dependencies for dependencies, and fs.* tools for source edits instead of interactive scaffold commands.",
            tool_description: "the runtime project tools",
        });
    }

    None
}

fn is_package_manager_install(argv: &[String]) -> bool {
    let command = argv.first().map(String::as_str).unwrap_or("");
    let subcommand = argv.get(1).map(String::as_str);
    matches!(
        (command, subcommand),
        ("pnpm", Some("install" | "add"))
            | ("npm", Some("install"))
            | ("yarn", Some("install" | "add"))
            | ("bun", Some("install" | "add"))
    )
}

fn is_preview_server_command(argv: &[String]) -> bool {
    let command = argv.first().map(String::as_str).unwrap_or("");
    match (command, argv.get(1).map(String::as_str)) {
        ("npx", _) => argv
            .iter()
            .skip(1)
            .any(|arg| matches!(arg.as_str(), "serve" | "vite" | "astro")),
        ("npm" | "pnpm" | "yarn", Some("run")) => argv
            .get(2)
            .is_some_and(|script| matches!(script.as_str(), "preview" | "dev" | "start")),
        ("npm", Some("exec")) => argv
            .iter()
            .skip(2)
            .any(|arg| matches!(arg.as_str(), "serve" | "vite" | "astro")),
        ("pnpm" | "yarn", Some("preview" | "dev" | "start")) => true,
        ("astro", Some("preview" | "dev")) | ("vite", Some("--host" | "--port")) => true,
        ("serve", _) => true,
        _ => false,
    }
}

fn is_interactive_project_scaffold(argv: &[String]) -> bool {
    let command = argv.first().map(String::as_str).unwrap_or("");
    if command == "npm" && argv.get(1).map(String::as_str) == Some("create") {
        return true;
    }
    if command == "npx"
        && argv.iter().any(|arg| arg == "astro")
        && argv.iter().any(|arg| arg == "add")
    {
        return true;
    }
    if command == "npm"
        && argv.get(1).map(String::as_str) == Some("exec")
        && argv.iter().any(|arg| arg == "astro")
        && argv.iter().any(|arg| arg == "add")
    {
        return true;
    }
    false
}

fn phase_rejection_metadata(phase: AgentPhase, tool_name: &str, suggested_action: &str) -> Value {
    serde_json::json!({
        "phase": format!("{phase:?}"),
        "tool": tool_name,
        "suggestedAction": suggested_action,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySuggestion {
    pub fingerprint: RecoverableErrorFingerprint,
    pub attempts: u32,
    pub guidance: String,
    pub emit_large_write_metric: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostToolUseFailureDecision {
    pub suggestion: Option<RecoverySuggestion>,
    pub partial_summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PostToolUseFailureHook {
    strong_guidance_attempt: u32,
    partial_attempt: u32,
}

impl Default for PostToolUseFailureHook {
    fn default() -> Self {
        Self {
            strong_guidance_attempt: DEFAULT_RECOVERY_STRONG_GUIDANCE_ATTEMPT,
            partial_attempt: DEFAULT_RECOVERY_PARTIAL_ATTEMPT,
        }
    }
}

impl PostToolUseFailureHook {
    pub fn apply(
        &self,
        phase: AgentPhase,
        observations: &[ToolFailureObservation],
        state: &mut Option<RecoverableErrorState>,
    ) -> PostToolUseFailureDecision {
        let Some(fingerprint) = observations
            .iter()
            .filter_map(|result| recoverable_error_fingerprint(phase, result))
            .next()
        else {
            *state = None;
            return PostToolUseFailureDecision::default();
        };

        let attempts = match state {
            Some(existing) if existing.fingerprint == fingerprint.key => {
                existing.attempts += 1;
                existing.attempts
            }
            _ => {
                *state = Some(RecoverableErrorState {
                    fingerprint: fingerprint.key.clone(),
                    attempts: 1,
                });
                1
            }
        };

        let suggestion =
            (attempts >= self.strong_guidance_attempt).then(|| RecoverySuggestion {
                guidance: format!(
                    "检测到同一类可恢复工具失败已连续出现 {attempts} 次：tool={tool}, errorKind={error_kind}, path={path}。{base_guidance} 请立即切换策略，不要再次提交相同的工具调用。",
                    tool = fingerprint.tool,
                    error_kind = fingerprint.error_kind,
                    path = fingerprint.normalized_path,
                    base_guidance = fingerprint.guidance,
                ),
                emit_large_write_metric: matches!(
                    fingerprint.error_kind.as_str(),
                    "tool.input_json_parse_failed" | "tool.input_too_large"
                ),
                fingerprint: fingerprint.clone(),
                attempts,
            });

        let partial_summary = (attempts >= self.partial_attempt).then(|| {
            format!(
                "已停止自动重试：同一类可恢复工具失败连续出现 {attempts} 次，tool={tool}, errorKind={error_kind}。恢复建议：{guidance} 请根据恢复建议切换策略后继续。当前 run 已以 partial 结束，最近成功的预览会保留。",
                tool = fingerprint.tool,
                error_kind = fingerprint.error_kind,
                guidance = fingerprint.guidance,
            )
        });

        PostToolUseFailureDecision {
            suggestion,
            partial_summary,
        }
    }
}

fn recoverable_error_fingerprint(
    phase: AgentPhase,
    result: &ToolFailureObservation,
) -> Option<RecoverableErrorFingerprint> {
    if !result.is_error {
        return None;
    }
    let metadata = result.metadata.as_ref()?;
    let recoverable = metadata
        .get("recoverable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !recoverable {
        return None;
    }
    let error_kind = metadata
        .get("errorKind")
        .and_then(Value::as_str)
        .unwrap_or("tool.error")
        .to_string();
    if !is_recoverable_error_guard_kind(&error_kind) {
        return None;
    }
    let normalized_path = metadata
        .get("path")
        .or_else(|| metadata.get("normalizedPath"))
        .or_else(|| metadata.get("receivedPath"))
        .or_else(|| metadata.get("suggestedPath"))
        .or_else(|| result.content.get("path"))
        .and_then(Value::as_str)
        .map(normalize_fingerprint_path)
        .unwrap_or_else(|| "unknown".to_string());
    let guidance = metadata
        .get("guidance")
        .or_else(|| metadata.get("suggestedAction"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| recoverable_error_guard_guidance(&error_kind).to_string());
    let key = format!(
        "{phase:?}|{}|{}|{}",
        result.tool_name, error_kind, normalized_path
    );
    Some(RecoverableErrorFingerprint {
        key,
        tool: result.tool_name.clone(),
        error_kind,
        normalized_path,
        guidance,
    })
}

fn is_recoverable_error_guard_kind(error_kind: &str) -> bool {
    matches!(
        error_kind,
        "tool.input_json_parse_failed"
            | "tool.input_too_large"
            | "path.external_directory"
            | "path.invalid_component"
            | "path.secret"
            | "path.cannot_resolve"
            | "path.nested_package_root"
            | "fs.read_failed"
            | "fs.list_failed"
            | "content.source_missing"
            | "style.input_invalid"
            | "style.contract_missing"
            | "style.contract_invalid"
            | "style.token_unknown"
            | "style.token_value_invalid"
            | "style.token_file_unavailable"
            | "style.token_file_invalid"
            | "style.token_variable_missing"
            | "style.token_variable_ambiguous"
            | "dependency.install_timeout"
            | "dependency.install_failed"
            | "docs.routing_root_forbidden"
            | "docs.source_contract_invalid"
            | "patch.read_required"
            | "patch.stale_read"
            | "patch.old_str_missing"
            | "patch.old_str_ambiguous"
            | "build.missing_dependency"
            | "build.timeout"
            | "build.failed"
            | "preview.screenshot_missing"
            | "preview.screenshot_invalid"
            | "preview.screenshot_blank"
            | "preview.build_missing"
            | "preview.build_failed"
            | "preview.source_snapshot_missing"
            | "preview.source_snapshot_mismatch"
            | "preview.already_promoted"
            | "preview.dist_missing"
            | "shell.command_denied"
            | "shell.non_zero_exit"
    )
}

fn recoverable_error_guard_guidance(error_kind: &str) -> &'static str {
    match error_kind {
        "path.external_directory"
        | "path.invalid_component"
        | "path.secret"
        | "path.cannot_resolve" => {
            "Use workspace-relative paths under project, inputs, state, or outputs; do not use host absolute paths."
        }
        "path.nested_package_root" => {
            "Use the existing app root package.json; do not create or edit nested package.json files under source directories."
        }
        "fs.read_failed" => {
            "Use fs.list for directories, then call fs.read with a workspace-relative file path."
        }
        "fs.list_failed" => "Verify the directory exists, or call fs.read if the path is a file.",
        "content.source_missing" => {
            "Call content.list_sources and read one of the returned source ids, or use the bootstrapped inputs/*.md files."
        }
        "style.input_invalid" => {
            "Pass a non-empty tokens object with simple CSS values and token names from state/style-contract.json."
        }
        "style.contract_missing" | "style.contract_invalid" => {
            "Run project.init or repair state/style-contract.json before using style.update_tokens."
        }
        "style.token_unknown" => {
            "Call project.inspect and update only tokens declared in state/style-contract.json."
        }
        "style.token_value_invalid" => {
            "Use a simple CSS token value without semicolons, braces, or newlines."
        }
        "style.token_file_unavailable"
        | "style.token_file_invalid"
        | "style.token_variable_missing"
        | "style.token_variable_ambiguous" => {
            "Repair the runtime token CSS file or regenerate it before retrying style.update_tokens."
        }
        "patch.read_required" => "Call fs.read on the target path before retrying fs.patch or fs.multi_patch.",
        "patch.stale_read" => "Read the file again and patch against the current content hash.",
        "patch.old_str_missing" => {
            "Search or read the current file, then retry with an exact snippet from current contents."
        }
        "patch.old_str_ambiguous" => {
            "Use a larger unique oldStr snippet, fs.multi_patch, or replaceAll=true only when every occurrence should change."
        }
        "build.missing_dependency" => {
            "Use project.ensure_dependencies with mode=restore, then rerun project.build or preview.publish."
        }
        "build.timeout" => {
            "Inspect diagnostics.build_log, increase timeoutMs if the build is legitimately long, then rerun project.build or preview.publish."
        }
        "build.failed" => {
            "Open diagnostics.build_log, fix the source or dependency error, then rerun project.build or preview.publish."
        }
        "dependency.install_timeout" => {
            "Retry project.ensure_dependencies with a larger timeoutMs after checking registry connectivity, then rerun project.build or preview.publish."
        }
        "dependency.install_failed" => {
            "Open the package install log, fix registry or package errors, then rerun project.ensure_dependencies."
        }
        "docs.routing_root_forbidden" => {
            "Keep fumadocs-docs on the Next app router: remove project/pages and write routes under app/."
        }
        "docs.source_contract_invalid" => {
            "Repair the fumadocs-docs scaffold before building: source.config.ts, lib/source.js, app/docs routes, and content/docs must match the runtime contract."
        }
        "preview.screenshot_missing" | "preview.screenshot_invalid" | "preview.screenshot_blank" => {
            "Run preview.publish, or run preview.start and browser.screenshot before reporting a candidate."
        }
        "preview.build_missing" | "preview.build_failed" | "preview.dist_missing" => {
            "Run project.build successfully before preview.start, preview.report_candidate, or preview.publish."
        }
        "preview.source_snapshot_missing" | "preview.source_snapshot_mismatch" => {
            "Use the sourceSnapshotUri from the latest successful project.build result."
        }
        "preview.already_promoted" => {
            "Do not manually report another candidate after promotion; complete the run if the artifact satisfies the request, or edit source and use preview.publish for the new source snapshot."
        }
        "shell.command_denied" => "Use the dedicated runtime tool for this operation instead of shell.run.",
        "shell.non_zero_exit" => {
            "Inspect stdout/stderr, fix the command arguments, or use a dedicated runtime tool when available."
        }
        _ => "Use the structured tool metadata to choose a different recovery strategy.",
    }
}

fn normalize_fingerprint_path(path: &str) -> String {
    let path = path
        .trim()
        .trim_start_matches("/workspace/")
        .trim_start_matches("./")
        .replace('\\', "/");
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        "unknown".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn post_tool_failure_hook_suggests_and_partials_repeated_typed_errors() {
        let hook = PostToolUseFailureHook::default();
        let mut state = None;
        let observations = vec![ToolFailureObservation {
            tool_name: "fs.patch".to_string(),
            is_error: true,
            content: json!({ "error": "read first" }),
            metadata: Some(json!({
                "recoverable": true,
                "errorKind": "patch.read_required",
                "path": "/workspace/project/copy.md",
                "suggestedAction": "Call fs.read on this path before fs.patch."
            })),
        }];

        let first = hook.apply(AgentPhase::Build, &observations, &mut state);
        assert!(first.suggestion.is_none());
        assert!(first.partial_summary.is_none());

        let second = hook.apply(AgentPhase::Build, &observations, &mut state);
        let suggestion = second
            .suggestion
            .expect("second attempt should suggest recovery");
        assert_eq!(suggestion.attempts, 2);
        assert_eq!(suggestion.fingerprint.tool, "fs.patch");
        assert_eq!(suggestion.fingerprint.error_kind, "patch.read_required");
        assert_eq!(suggestion.fingerprint.normalized_path, "project/copy.md");
        assert!(suggestion.guidance.contains("fs.read"));
        assert!(!suggestion.emit_large_write_metric);

        let third = hook.apply(AgentPhase::Build, &observations, &mut state);
        assert!(third.suggestion.is_some());
        assert!(third
            .partial_summary
            .as_deref()
            .is_some_and(|summary| summary.contains("patch.read_required")));
    }

    #[test]
    fn post_tool_failure_hook_guides_repeated_style_token_errors() {
        let hook = PostToolUseFailureHook::default();
        let mut state = None;
        let observations = vec![ToolFailureObservation {
            tool_name: "style.update_tokens".to_string(),
            is_error: true,
            content: json!({ "error": "unknown token" }),
            metadata: Some(json!({
                "recoverable": true,
                "errorKind": "style.token_unknown",
                "token": "color.brand",
                "suggestedAction": "Call project.inspect and update only tokens declared in state/style-contract.json."
            })),
        }];

        let first = hook.apply(AgentPhase::Edit, &observations, &mut state);
        assert!(first.suggestion.is_none());

        let second = hook.apply(AgentPhase::Edit, &observations, &mut state);
        let suggestion = second
            .suggestion
            .expect("second style token failure should suggest recovery");
        assert_eq!(suggestion.attempts, 2);
        assert_eq!(suggestion.fingerprint.tool, "style.update_tokens");
        assert_eq!(suggestion.fingerprint.error_kind, "style.token_unknown");
        assert!(suggestion.guidance.contains("project.inspect"));
        assert!(!suggestion.emit_large_write_metric);

        let third = hook.apply(AgentPhase::Edit, &observations, &mut state);
        assert!(third
            .partial_summary
            .as_deref()
            .is_some_and(|summary| summary.contains("style.token_unknown")));
    }

    #[test]
    fn post_tool_failure_hook_resets_when_no_recoverable_error_is_observed() {
        let hook = PostToolUseFailureHook::default();
        let mut state = Some(RecoverableErrorState {
            fingerprint: "Build|fs.patch|patch.read_required|project/copy.md".to_string(),
            attempts: 2,
        });
        let observations = vec![ToolFailureObservation {
            tool_name: "fs.read".to_string(),
            is_error: false,
            content: json!({ "text": "ok" }),
            metadata: None,
        }];

        let decision = hook.apply(AgentPhase::Build, &observations, &mut state);

        assert!(decision.suggestion.is_none());
        assert!(decision.partial_summary.is_none());
        assert!(state.is_none());
    }

    #[test]
    fn pre_tool_hook_rejects_workspace_tools_during_brief_phase() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Brief,
            tool_name: "fs.read".to_string(),
            input: json!({ "path": "project/src/pages/index.astro" }),
            default_cwd: Some("project".to_string()),
        });

        let rejection = decision
            .rejection
            .expect("brief phase should reject workspace tools");
        assert_eq!(rejection.error_kind, "tool.phase_forbidden");
        assert!(rejection.recoverable);
        assert_eq!(rejection.metadata["tool"], "fs.read");
        assert_eq!(rejection.metadata["phase"], "Brief");
    }

    #[test]
    fn pre_tool_hook_rejects_brief_write_tools_outside_brief_phase() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "brief.write_draft".to_string(),
            input: json!({ "brief": {} }),
            default_cwd: Some("project".to_string()),
        });

        let rejection = decision
            .rejection
            .expect("build phase should reject brief write tools");
        assert_eq!(rejection.error_kind, "tool.phase_forbidden");
        assert_eq!(rejection.metadata["tool"], "brief.write_draft");
        assert_eq!(rejection.metadata["phase"], "Build");
    }

    #[test]
    fn pre_tool_hook_injects_default_cwd_for_build_execution_tools() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "shell.run".to_string(),
            input: json!({ "argv": ["node", "-e", "process.stdout.write('ok')"] }),
            default_cwd: Some("custom-app".to_string()),
        });

        assert!(decision.rejection.is_none());
        assert_eq!(decision.input["cwd"], "custom-app");
    }

    #[test]
    fn pre_tool_hook_preserves_explicit_cwd() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Edit,
            tool_name: "project.build".to_string(),
            input: json!({ "cwd": "project/site" }),
            default_cwd: Some("project".to_string()),
        });

        assert!(decision.rejection.is_none());
        assert_eq!(decision.input["cwd"], "project/site");
    }

    #[test]
    fn pre_tool_hook_normalizes_workspace_virtual_path_fields() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "fs.read".to_string(),
            input: json!({ "path": "/workspace/project/src/pages/index.astro" }),
            default_cwd: Some("project".to_string()),
        });

        assert!(decision.rejection.is_none());
        assert_eq!(decision.input["path"], "project/src/pages/index.astro");

        let cwd_decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "project.build".to_string(),
            input: json!({ "cwd": "/workspace/project/site" }),
            default_cwd: Some("project".to_string()),
        });

        assert!(cwd_decision.rejection.is_none());
        assert_eq!(cwd_decision.input["cwd"], "project/site");
    }

    #[test]
    fn pre_tool_hook_redirects_shell_dependency_installs_to_runtime_tools() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "shell.run".to_string(),
            input: json!({ "argv": ["pnpm", "install"] }),
            default_cwd: Some("project".to_string()),
        });

        let rejection = decision
            .rejection
            .expect("shell dependency installs should use runtime tools");
        assert_eq!(rejection.error_kind, "shell.command_denied");
        assert!(rejection.recoverable);
        assert_eq!(
            rejection.metadata["suggestedTool"],
            "project.ensure_dependencies"
        );
        assert_eq!(rejection.metadata["argv"], json!(["pnpm", "install"]));
    }

    #[test]
    fn pre_tool_hook_redirects_shell_preview_servers_to_preview_tools() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "shell.run".to_string(),
            input: json!({ "argv": ["pnpm", "run", "dev"] }),
            default_cwd: Some("project".to_string()),
        });

        let rejection = decision
            .rejection
            .expect("preview server commands should use preview tools");
        assert_eq!(rejection.error_kind, "shell.command_denied");
        assert_eq!(rejection.metadata["suggestedTool"], "preview.start");
    }

    #[test]
    fn pre_tool_hook_redirects_interactive_scaffolds_to_project_tools() {
        let decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: AgentPhase::Build,
            tool_name: "shell.run".to_string(),
            input: json!({ "argv": ["npm", "create", "astro@latest"] }),
            default_cwd: Some("project".to_string()),
        });

        let rejection = decision
            .rejection
            .expect("interactive scaffold commands should use project tools");
        assert_eq!(rejection.error_kind, "shell.command_denied");
        assert_eq!(rejection.metadata["suggestedTool"], "project.init");
    }

    #[test]
    fn post_tool_success_hook_classifies_build_state_updates() {
        let decision = PostToolUseSuccessHook.apply(ToolSuccessObservation {
            tool_name: "project.build".to_string(),
            content: json!({
                "status": "ok",
                "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-1",
                "packageManager": "pnpm"
            }),
            metadata: None,
        });

        let metadata = decision
            .metadata
            .expect("build should have success metadata");
        assert_eq!(
            metadata["postToolUseSuccess"]["effect"],
            "build_state_updated"
        );
        assert_eq!(metadata["postToolUseSuccess"]["packageManager"], "pnpm");
    }

    #[test]
    fn post_tool_success_hook_classifies_publish_promotion_updates() {
        let decision = PostToolUseSuccessHook.apply(ToolSuccessObservation {
            tool_name: "preview.publish".to_string(),
            content: json!({
                "promotion": {
                    "versionId": "version-1",
                    "previewUrl": "http://127.0.0.1:4321"
                }
            }),
            metadata: None,
        });

        let metadata = decision
            .metadata
            .expect("preview.publish should have success metadata");
        assert_eq!(
            metadata["postToolUseSuccess"]["effect"],
            "promotion_state_updated"
        );
        assert_eq!(metadata["postToolUseSuccess"]["versionId"], "version-1");
    }
}
