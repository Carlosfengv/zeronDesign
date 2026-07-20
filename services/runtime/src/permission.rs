use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use crate::tools::runtime::{Tool, ToolContext};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Org,
    Project,
    Profile,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionReason {
    Rule {
        source: RuleSource,
        rule_content: String,
    },
    SafetyCheck {
        classifier_approvable: bool,
    },
    Mode {
        mode: String,
    },
    Hook {
        hook_name: String,
        reason: Option<String>,
    },
    AsyncAgent {
        reason: String,
    },
    Other {
        reason: String,
    },
}

impl PermissionReason {
    pub fn summary(&self) -> String {
        match self {
            Self::Rule { rule_content, .. } => rule_content.clone(),
            Self::SafetyCheck {
                classifier_approvable,
            } => format!("safety_check approvable={classifier_approvable}"),
            Self::Mode { mode } => format!("permission mode: {mode}"),
            Self::Hook { hook_name, reason } => reason
                .as_ref()
                .map(|reason| format!("{hook_name}: {reason}"))
                .unwrap_or_else(|| hook_name.clone()),
            Self::AsyncAgent { reason } | Self::Other { reason } => reason.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionUpdate {
    pub tool: String,
    pub decision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        updated_input: Value,
        reason: PermissionReason,
    },
    Ask {
        message: String,
        reason: PermissionReason,
        suggestions: Option<Vec<PermissionUpdate>>,
    },
    Deny {
        message: String,
        reason: PermissionReason,
    },
    Passthrough {
        message: String,
    },
}

impl PermissionResult {
    pub fn decision(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "allow",
            Self::Ask { .. } => "ask",
            Self::Deny { .. } => "deny",
            Self::Passthrough { .. } => "passthrough",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Allow { reason, .. } => reason.summary(),
            Self::Ask { message, .. }
            | Self::Deny { message, .. }
            | Self::Passthrough { message } => message.clone(),
        }
    }

    pub fn reason_summary(&self) -> String {
        match self {
            Self::Allow { reason, .. } | Self::Ask { reason, .. } | Self::Deny { reason, .. } => {
                reason.summary()
            }
            Self::Passthrough { message } => message.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PermissionRules {
    deny_tools: BTreeSet<String>,
    ask_tools: BTreeSet<String>,
    always_allowed_tools: BTreeSet<String>,
    pre_tool_use_hooks: BTreeMap<String, PreToolUseHookDecision>,
    permission_request_hooks: BTreeMap<String, PermissionRequestHookDecision>,
    pub bypass_permissions: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreToolUseHookDecision {
    Allow {
        reason: String,
        updated_input: Option<Value>,
    },
    Ask {
        reason: String,
        updated_input: Option<Value>,
    },
    Deny {
        reason: String,
    },
}

impl PreToolUseHookDecision {
    pub fn allow(reason: impl Into<String>, updated_input: Option<Value>) -> Self {
        Self::Allow {
            reason: reason.into(),
            updated_input,
        }
    }

    pub fn ask(reason: impl Into<String>, updated_input: Option<Value>) -> Self {
        Self::Ask {
            reason: reason.into(),
            updated_input,
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionRequestHookDecision {
    Allow {
        reason: String,
        updated_input: Option<Value>,
    },
    Deny {
        reason: String,
    },
}

impl PermissionRequestHookDecision {
    pub fn allow(reason: impl Into<String>, updated_input: Option<Value>) -> Self {
        Self::Allow {
            reason: reason.into(),
            updated_input,
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }
}

impl PermissionRules {
    pub fn deny_tool(mut self, tool: impl Into<String>) -> Self {
        self.deny_tools.insert(tool.into());
        self
    }

    pub fn ask_tool(mut self, tool: impl Into<String>) -> Self {
        self.ask_tools.insert(tool.into());
        self
    }

    pub fn always_allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.always_allowed_tools.insert(tool.into());
        self
    }

    pub fn pre_tool_use_hook(
        mut self,
        tool: impl Into<String>,
        decision: PreToolUseHookDecision,
    ) -> Self {
        self.pre_tool_use_hooks.insert(tool.into(), decision);
        self
    }

    pub fn permission_request_hook(
        mut self,
        tool: impl Into<String>,
        decision: PermissionRequestHookDecision,
    ) -> Self {
        self.permission_request_hooks.insert(tool.into(), decision);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct PermissionEngine {
    rules: PermissionRules,
}

impl PermissionEngine {
    pub fn new(rules: PermissionRules) -> Self {
        Self { rules }
    }

    pub async fn decide(
        &self,
        tool: &dyn Tool,
        input: &Value,
        ctx: &ToolContext,
    ) -> PermissionResult {
        if let Some(hook_decision) = self.rules.pre_tool_use_hooks.get(tool.name()) {
            return self
                .resolve_pre_tool_use_hook(tool, input, ctx, hook_decision)
                .await;
        }
        self.decide_without_pre_tool_use(tool, input, ctx).await
    }

    async fn resolve_pre_tool_use_hook(
        &self,
        tool: &dyn Tool,
        input: &Value,
        ctx: &ToolContext,
        hook_decision: &PreToolUseHookDecision,
    ) -> PermissionResult {
        match hook_decision {
            PreToolUseHookDecision::Deny { reason } => PermissionResult::Deny {
                message: reason.clone(),
                reason: PermissionReason::Hook {
                    hook_name: "PreToolUse".to_string(),
                    reason: Some(reason.clone()),
                },
            },
            PreToolUseHookDecision::Ask {
                reason,
                updated_input,
            } => {
                let hook_input = updated_input.as_ref().unwrap_or(input);
                self.resolve_ask(
                    tool,
                    hook_input,
                    ctx,
                    PermissionResult::Ask {
                        message: reason.clone(),
                        reason: PermissionReason::Hook {
                            hook_name: "PreToolUse".to_string(),
                            reason: Some(reason.clone()),
                        },
                        suggestions: None,
                    },
                )
            }
            PreToolUseHookDecision::Allow {
                reason,
                updated_input,
            } => {
                let hook_input = updated_input.as_ref().unwrap_or(input);
                let decision = self
                    .decide_without_pre_tool_use(tool, hook_input, ctx)
                    .await;
                match decision {
                    PermissionResult::Allow {
                        updated_input: Value::Null,
                        ..
                    } => PermissionResult::Allow {
                        updated_input: hook_input.clone(),
                        reason: PermissionReason::Hook {
                            hook_name: "PreToolUse".to_string(),
                            reason: Some(reason.clone()),
                        },
                    },
                    PermissionResult::Allow { updated_input, .. } => PermissionResult::Allow {
                        updated_input,
                        reason: PermissionReason::Hook {
                            hook_name: "PreToolUse".to_string(),
                            reason: Some(reason.clone()),
                        },
                    },
                    PermissionResult::Ask { .. }
                    | PermissionResult::Deny { .. }
                    | PermissionResult::Passthrough { .. } => decision,
                }
            }
        }
    }

    async fn decide_without_pre_tool_use(
        &self,
        tool: &dyn Tool,
        input: &Value,
        ctx: &ToolContext,
    ) -> PermissionResult {
        if self.rules.deny_tools.contains(tool.name()) {
            return PermissionResult::Deny {
                message: format!("{} denied by runtime rule", tool.name()),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: format!("deny {}", tool.name()),
                },
            };
        }

        if self.rules.ask_tools.contains(tool.name()) {
            return self.resolve_ask(
                tool,
                input,
                ctx,
                PermissionResult::Ask {
                    message: format!("{} requires approval", tool.name()),
                    reason: PermissionReason::Rule {
                        source: RuleSource::Runtime,
                        rule_content: format!("ask {}", tool.name()),
                    },
                    suggestions: None,
                },
            );
        }

        let tool_decision = tool.check_permission(input, ctx).await;
        match tool_decision {
            PermissionResult::Deny { .. } => tool_decision,
            PermissionResult::Ask { .. } => self.resolve_ask(tool, input, ctx, tool_decision),
            PermissionResult::Allow { .. } => tool_decision,
            PermissionResult::Passthrough { .. } => {
                if self.rules.bypass_permissions {
                    return PermissionResult::Allow {
                        updated_input: input.clone(),
                        reason: PermissionReason::Mode {
                            mode: "bypass_permissions".to_string(),
                        },
                    };
                }
                if self.rules.always_allowed_tools.contains(tool.name()) {
                    return PermissionResult::Allow {
                        updated_input: input.clone(),
                        reason: PermissionReason::Rule {
                            source: RuleSource::Runtime,
                            rule_content: format!("always_allow {}", tool.name()),
                        },
                    };
                }
                self.resolve_ask(
                    tool,
                    input,
                    ctx,
                    PermissionResult::Ask {
                        message: format!("{} requires approval", tool.name()),
                        reason: PermissionReason::Other {
                            reason: "tool did not declare an allow rule".to_string(),
                        },
                        suggestions: None,
                    },
                )
            }
        }
    }

    fn resolve_ask(
        &self,
        tool: &dyn Tool,
        input: &Value,
        ctx: &ToolContext,
        ask: PermissionResult,
    ) -> PermissionResult {
        if ctx.should_avoid_permission_prompts {
            if let Some(hook_decision) = self.rules.permission_request_hooks.get(tool.name()) {
                return match hook_decision {
                    PermissionRequestHookDecision::Allow {
                        reason,
                        updated_input,
                    } => PermissionResult::Allow {
                        updated_input: updated_input.clone().unwrap_or_else(|| input.clone()),
                        reason: PermissionReason::Hook {
                            hook_name: "PermissionRequest".to_string(),
                            reason: Some(reason.clone()),
                        },
                    },
                    PermissionRequestHookDecision::Deny { reason } => PermissionResult::Deny {
                        message: reason.clone(),
                        reason: PermissionReason::Hook {
                            hook_name: "PermissionRequest".to_string(),
                            reason: Some(reason.clone()),
                        },
                    },
                };
            }
            return PermissionResult::Deny {
                message: "Permission prompts are not available".to_string(),
                reason: PermissionReason::AsyncAgent {
                    reason: format!("{} asked in headless mode", tool.name()),
                },
            };
        }
        ask
    }
}

pub fn allow_reason(reason: impl Into<String>) -> PermissionResult {
    PermissionResult::Allow {
        updated_input: Value::Null,
        reason: PermissionReason::Other {
            reason: reason.into(),
        },
    }
}

pub fn check_command_policy(argv: &[String]) -> PermissionResult {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    const ALWAYS_DENY: &[&str] = &[
        "sh", "bash", "zsh", "fish", "kubectl", "docker", "ssh", "scp", "sudo",
    ];
    if ALWAYS_DENY.contains(&cmd) {
        return PermissionResult::Deny {
            message: format!("{cmd} is not allowed"),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "always-deny command".to_string(),
            },
        };
    }

    for arg in argv {
        if is_deny_pattern(arg) {
            return PermissionResult::Deny {
                message: format!("argument contains denied pattern: {arg}"),
                reason: PermissionReason::SafetyCheck {
                    classifier_approvable: false,
                },
            };
        }
    }

    if is_interactive_project_scaffold(argv) {
        return PermissionResult::Deny {
            message: "Use project.init/package.install/fs.* instead of interactive framework scaffold commands".to_string(),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "interactive scaffold commands must use project tools".to_string(),
            },
        };
    }

    if is_preview_server_command(argv) {
        return PermissionResult::Deny {
            message:
                "Use preview.start instead of launching long-running preview servers with shell.run"
                    .to_string(),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "preview servers must be managed by preview.start".to_string(),
            },
        };
    }

    match (cmd, argv.get(1).map(String::as_str)) {
        ("pnpm", Some("install" | "add"))
        | ("npm", Some("install"))
        | ("yarn", Some("install" | "add"))
        | ("bun", Some("install" | "add")) => PermissionResult::Deny {
            message: "Dependency installation must use package.install. Use package.install({ \"mode\": \"restore\", \"packageManager\": \"pnpm\" }) for package.json installs or package.install({ \"mode\": \"add\", \"packages\": [\"...\"], \"packageManager\": \"pnpm\" }) for new dependencies.".to_string(),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "dependency installation must use package.install".to_string(),
            },
        },
        ("pnpm", Some("build" | "dev" | "lint" | "test" | "run"))
        | ("npm", Some("create" | "exec"))
        | ("npm", Some("run"))
        | ("npx", _)
        | ("node", _)
        | ("mkdir", _)
        | ("which", _)
        | ("ls", _)
        | ("pwd", _)
        | ("find", _)
        | ("rg", _)
        | ("cat", _)
        | ("sed", _) => PermissionResult::Allow {
            updated_input: Value::Null,
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "command allowlist".to_string(),
            },
        },
        ("git", _) => PermissionResult::Ask {
            message: format!("{cmd} requires platform policy approval"),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "command asklist".to_string(),
            },
            suggestions: None,
        },
        _ => PermissionResult::Deny {
            message: format!("{cmd} not in allowlist"),
            reason: PermissionReason::Rule {
                source: RuleSource::Runtime,
                rule_content: "command not allowed".to_string(),
            },
        },
    }
}

fn is_preview_server_command(argv: &[String]) -> bool {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    match (cmd, argv.get(1).map(String::as_str)) {
        ("npx", _) => argv
            .iter()
            .skip(1)
            .any(|arg| matches!(arg.as_str(), "serve" | "vite" | "next")),
        ("npm" | "pnpm" | "yarn", Some("run")) => argv
            .get(2)
            .is_some_and(|script| matches!(script.as_str(), "preview" | "dev" | "start")),
        ("npm", Some("exec")) => argv
            .iter()
            .skip(2)
            .any(|arg| matches!(arg.as_str(), "serve" | "vite" | "next")),
        ("pnpm" | "yarn", Some("preview" | "dev" | "start")) => true,
        ("next", Some("dev" | "start")) | ("vite", Some("--host" | "--port")) => true,
        ("serve", _) => true,
        _ => false,
    }
}

fn is_interactive_project_scaffold(argv: &[String]) -> bool {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    if cmd == "npm" && argv.get(1).map(String::as_str) == Some("create") {
        return true;
    }
    false
}

fn is_deny_pattern(arg: &str) -> bool {
    [
        "rm -rf",
        "/etc/passwd",
        "id_rsa",
        "id_ed25519",
        "kubeconfig",
    ]
    .iter()
    .any(|pattern| arg.contains(pattern))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    CannotResolve(PathBuf),
    ExternalDirectory(PathBuf),
    SecretPath(PathBuf),
    InvalidPathComponent(PathBuf),
}

// remote-fs-boundary: allow-begin local-workspace-path-policy
pub fn check_existing_path(path: &Path, workspace_root: &Path) -> Result<PathBuf, PermissionError> {
    if !path.exists() {
        return check_create_path(path, workspace_root);
    }
    let real =
        std::fs::canonicalize(path).map_err(|_| PermissionError::CannotResolve(path.to_owned()))?;
    ensure_workspace_path(&real, workspace_root)?;
    ensure_not_secret_path(&real)?;
    Ok(real)
}

pub fn check_workspace_path(
    path: &Path,
    workspace_root: &Path,
) -> Result<PathBuf, PermissionError> {
    if path.exists() {
        check_existing_path(path, workspace_root)
    } else {
        check_create_path(path, workspace_root)
    }
}

pub fn check_lexical_workspace_path(
    path: &Path,
    workspace_root: &Path,
) -> Result<PathBuf, PermissionError> {
    let normalized = normalize_workspace_path(path)?;
    let normalized_root = normalize_workspace_path(workspace_root)?;
    if !normalized.starts_with(&normalized_root) {
        return Err(PermissionError::ExternalDirectory(normalized));
    }
    ensure_not_secret_path(&normalized)?;
    Ok(normalized)
}

pub fn check_create_path(path: &Path, workspace_root: &Path) -> Result<PathBuf, PermissionError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PermissionError::InvalidPathComponent(path.to_owned()));
    }

    if !workspace_root.exists() {
        let normalized = normalize_workspace_path(path)?;
        if !is_lexically_inside_workspace(&normalized, workspace_root) {
            return Err(PermissionError::ExternalDirectory(normalized));
        }
        ensure_not_secret_path(&normalized)?;
        return Ok(normalized);
    }

    let mut existing_ancestor = path;
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or_else(|| PermissionError::CannotResolve(path.to_owned()))?;
    }
    let real_ancestor = std::fs::canonicalize(existing_ancestor)
        .map_err(|_| PermissionError::CannotResolve(existing_ancestor.to_owned()))?;
    ensure_workspace_path(&real_ancestor, workspace_root)?;

    let missing_suffix = path
        .strip_prefix(existing_ancestor)
        .map_err(|_| PermissionError::CannotResolve(path.to_owned()))?;
    if missing_suffix
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(PermissionError::InvalidPathComponent(path.to_owned()));
    }

    let normalized = real_ancestor.join(missing_suffix);
    ensure_workspace_path(&normalized, workspace_root)?;
    ensure_not_secret_path(&normalized)?;
    Ok(normalized)
}

fn is_lexically_inside_workspace(path: &Path, workspace_root: &Path) -> bool {
    normalize_workspace_path(path).ok().is_some_and(|path| {
        path.starts_with(
            normalize_workspace_path(workspace_root)
                .unwrap_or_else(|_| workspace_root.to_path_buf()),
        )
    })
}

fn normalize_workspace_path(path: &Path) -> Result<PathBuf, PermissionError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PermissionError::InvalidPathComponent(path.to_owned()))
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn ensure_workspace_path(real: &Path, workspace_root: &Path) -> Result<(), PermissionError> {
    let workspace_root = std::fs::canonicalize(workspace_root)
        .map_err(|_| PermissionError::CannotResolve(workspace_root.to_owned()))?;
    if !real.starts_with(&workspace_root) {
        return Err(PermissionError::ExternalDirectory(real.to_owned()));
    }
    Ok(())
}
// remote-fs-boundary: allow-end local-workspace-path-policy

fn ensure_not_secret_path(real: &Path) -> Result<(), PermissionError> {
    if is_secret_path(real.to_str().unwrap_or("")) {
        return Err(PermissionError::SecretPath(real.to_owned()));
    }
    Ok(())
}

fn is_secret_path(path: &str) -> bool {
    const PATTERNS: &[&str] = &[
        ".env",
        "kubeconfig",
        ".ssh/",
        "id_rsa",
        "id_ed25519",
        ".token",
        "credentials",
        "private_key",
    ];
    PATTERNS.iter().any(|pattern| path.contains(pattern))
}
