use crate::{
    config::RuntimePolicyProfile,
    conversation::RuntimeStore,
    model_gateway::ModelToolDefinition,
    permission::{PermissionEngine, PermissionResult, PermissionRules},
    profiles::policy,
    types::{AgentEvent, AgentRun, AgentRunStatus, TranscriptMode},
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::registry::{McpToolInfo, ToolLoadingPolicy};

pub const DEFAULT_MAX_RESULT_SIZE_CHARS: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptBehavior {
    Block,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchReadKind {
    None,
    Search,
    Read,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    pub error_kind: Option<String>,
    pub metadata: Option<Value>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error_kind: None,
            metadata: None,
        }
    }

    pub fn with_kind(message: impl Into<String>, error_kind: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error_kind: Some(error_kind.into()),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    Recoverable(String),
    Terminal(String),
    PermissionDenied(String),
    Aborted,
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            Self::Recoverable(message)
            | Self::Terminal(message)
            | Self::PermissionDenied(message) => message.clone(),
            Self::Aborted => "tool aborted".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content: Value,
    pub is_error: bool,
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn ok(content: Value) -> Self {
        Self {
            content,
            is_error: false,
            metadata: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: json!({ "error": message.into() }),
            is_error: true,
            metadata: None,
        }
    }

    pub fn error_with_recoverable(message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            content: json!({ "error": message.into() }),
            is_error: true,
            metadata: Some(json!({ "recoverable": recoverable })),
        }
    }

    pub fn typed_error(
        message: impl Into<String>,
        error_kind: impl Into<String>,
        recoverable: bool,
        metadata: Value,
    ) -> Self {
        let mut metadata = metadata;
        let error_kind = error_kind.into();
        if let Some(object) = metadata.as_object_mut() {
            object.insert("recoverable".to_string(), Value::Bool(recoverable));
            object.insert("errorKind".to_string(), Value::String(error_kind.clone()));
        } else {
            metadata = json!({
                "recoverable": recoverable,
                "errorKind": error_kind,
                "details": metadata,
            });
        }
        Self {
            content: json!({ "error": message.into() }),
            is_error: true,
            metadata: Some(metadata),
        }
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub store: RuntimeStore,
    pub run: AgentRun,
    pub project_id: String,
    pub should_avoid_permission_prompts: bool,
    pub workspace_root: PathBuf,
    pub policy_profile: RuntimePolicyProfile,
    pub npm_registry: String,
}

impl ToolContext {
    pub fn new(store: RuntimeStore, run: AgentRun, workspace_root: PathBuf) -> Self {
        let should_avoid_permission_prompts =
            run.profile_snapshot.transcript_mode == TranscriptMode::Sidechain;
        Self {
            project_id: run.project_id.clone(),
            store,
            run,
            should_avoid_permission_prompts,
            workspace_root,
            policy_profile: RuntimePolicyProfile::Production,
            npm_registry: "https://registry.internal.example/npm/".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProgressSink {
    run_id: String,
    tool_use_id: String,
    store: RuntimeStore,
}

impl ProgressSink {
    pub fn new(
        run_id: impl Into<String>,
        tool_use_id: impl Into<String>,
        store: RuntimeStore,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            tool_use_id: tool_use_id.into(),
            store,
        }
    }

    pub async fn emit(&self, summary: impl Into<String>) {
        self.store
            .append_event(AgentEvent::ToolStarted {
                run_id: self.run_id.clone(),
                tool: "progress".to_string(),
                summary: summary.into(),
                tool_use_id: self.tool_use_id.clone(),
                timestamp: Utc::now(),
            })
            .await;
    }

    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    pub async fn emit_tool_output(
        &self,
        tool: impl Into<String>,
        stream: impl Into<String>,
        text: impl Into<String>,
    ) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.store
            .append_event(AgentEvent::ToolOutput {
                run_id: self.run_id.clone(),
                tool: tool.into(),
                tool_use_id: self.tool_use_id.clone(),
                stream: stream.into(),
                text,
                timestamp: Utc::now(),
            })
            .await;
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    fn input_json_schema(&self) -> Option<Value> {
        None
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
    async fn description(&self, _input: Option<&Value>, _ctx: &ToolContext) -> String {
        self.name().to_string()
    }
    fn is_enabled(&self, _ctx: &ToolContext) -> bool {
        true
    }
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn tool_loading(&self) -> ToolLoadingPolicy {
        ToolLoadingPolicy::Eager
    }
    fn mcp_info(&self) -> Option<McpToolInfo> {
        None
    }
    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _input: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }
    fn is_search_or_read(&self, _input: &Value) -> SearchReadKind {
        SearchReadKind::None
    }
    fn requires_user_interaction(&self) -> bool {
        false
    }
    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        Ok(input)
    }
    fn normalize_input_for_model(&self, input: Value, _ctx: &ToolContext) -> Value {
        input
    }
    fn backfill_observable_input(&self, _input: &mut Value) {}
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool {
        a == b
    }
    async fn check_permission(&self, _input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Passthrough {
            message: "tool did not declare permission behavior".to_string(),
        }
    }
    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError>;
    fn max_result_size_chars(&self) -> usize {
        DEFAULT_MAX_RESULT_SIZE_CHARS
    }
}

#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub result: ToolResult,
}

#[derive(Clone)]
pub struct ToolExecutor {
    tools: Arc<BTreeMap<String, Arc<dyn Tool>>>,
    permission_engine: PermissionEngine,
    workspace_root: Arc<PathBuf>,
    policy_profile: RuntimePolicyProfile,
    npm_registry: Arc<String>,
}

impl ToolExecutor {
    pub fn new(tools: Vec<Arc<dyn Tool>>, permission_rules: PermissionRules) -> Self {
        Self::new_with_workspace_root(tools, permission_rules, PathBuf::from("/workspace"))
    }

    pub fn new_with_workspace_root(
        tools: Vec<Arc<dyn Tool>>,
        permission_rules: PermissionRules,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        let mut map = BTreeMap::new();
        for tool in tools {
            map.insert(tool.name().to_string(), tool.clone());
            for alias in tool.aliases() {
                map.insert((*alias).to_string(), tool.clone());
            }
        }
        Self {
            tools: Arc::new(map),
            permission_engine: PermissionEngine::new(permission_rules),
            workspace_root: Arc::new(normalize_workspace_root(workspace_root.into())),
            policy_profile: RuntimePolicyProfile::Production,
            npm_registry: Arc::new("https://registry.internal.example/npm/".to_string()),
        }
    }

    pub fn with_policy_profile_and_registry(
        &self,
        policy_profile: RuntimePolicyProfile,
        npm_registry: impl Into<String>,
    ) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: self.workspace_root.clone(),
            policy_profile,
            npm_registry: Arc::new(npm_registry.into()),
        }
    }

    pub fn with_workspace_root(&self, workspace_root: impl AsRef<Path>) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: Arc::new(normalize_workspace_root(
                workspace_root.as_ref().to_path_buf(),
            )),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn is_concurrency_safe(&self, name: &str, input: &Value) -> bool {
        self.get(name)
            .map(|tool| tool.is_concurrency_safe(input))
            .unwrap_or(false)
    }

    pub fn interrupt_behavior(&self, name: &str) -> InterruptBehavior {
        self.get(name)
            .map(|tool| tool.interrupt_behavior())
            .unwrap_or(InterruptBehavior::Cancel)
    }

    pub async fn model_tool_snapshot(
        &self,
        store: RuntimeStore,
        run_id: &str,
    ) -> (Vec<ModelToolDefinition>, Vec<ModelToolDefinition>) {
        let Some(run) = store.get_run(run_id).await else {
            return (Vec::new(), Vec::new());
        };
        let ctx = self.tool_context(store, run);
        let mut unique_tools = BTreeMap::new();
        for tool in self.tools.values() {
            unique_tools.insert(tool.name(), tool.clone());
        }

        let mut eager_tools = Vec::new();
        let mut deferred_tools = Vec::new();
        for tool in unique_tools.into_values() {
            if !tool.is_enabled(&ctx)
                || !policy::tool_allowed(&ctx.run.profile_snapshot, tool.name())
            {
                continue;
            }
            let definition = ModelToolDefinition {
                name: tool.name().to_string(),
                input_schema: tool.input_schema(),
                input_json_schema: tool.input_json_schema(),
                output_schema: tool.output_schema(),
                loading_policy: tool.tool_loading(),
                mcp_info: tool.mcp_info(),
            };
            match definition.loading_policy {
                ToolLoadingPolicy::Eager | ToolLoadingPolicy::AlwaysLoad => {
                    eager_tools.push(definition);
                }
                ToolLoadingPolicy::Deferred => deferred_tools.push(definition),
            }
        }
        (eager_tools, deferred_tools)
    }

    pub async fn execute(
        &self,
        store: RuntimeStore,
        run_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        input: Value,
    ) -> ToolExecution {
        let input = normalize_tool_input(input);
        let Some(run) = store.get_run(run_id).await else {
            return ToolExecution {
                result: ToolResult::error(format!("run not found: {run_id}")),
            };
        };
        let ctx = self.tool_context(store.clone(), run);
        let Some(tool) = self.get(tool_name) else {
            store
                .append_audit_record(
                    &ctx.project_id,
                    &ctx.run.id,
                    tool_name,
                    summarize_input(&input),
                    "deny",
                    "tool is not registered",
                )
                .await;
            return ToolExecution {
                result: ToolResult::error(format!("No such tool available: {tool_name}")),
            };
        };

        let original_input = input.clone();
        let validated_input = match tool.validate_input(input, &ctx).await {
            Ok(input) => input,
            Err(error) => {
                let error_kind = error
                    .error_kind
                    .clone()
                    .unwrap_or_else(|| "tool.input_schema_invalid".to_string());
                store
                    .append_audit_record(
                        &ctx.project_id,
                        &ctx.run.id,
                        tool.name(),
                        summarize_input(&original_input),
                        "deny",
                        format!("input validation failed ({error_kind}): {}", error.message),
                    )
                    .await;
                let mut metadata = error.metadata.unwrap_or_else(|| json!({}));
                if let Some(object) = metadata.as_object_mut() {
                    object.insert("tool".to_string(), Value::String(tool.name().to_string()));
                }
                return ToolExecution {
                    result: ToolResult::typed_error(error.message, error_kind, true, metadata),
                };
            }
        };

        if !policy::tool_allowed(&ctx.run.profile_snapshot, tool.name()) {
            let reason = policy::denial_reason(&ctx.run.profile_snapshot, tool.name());
            let permission = PermissionResult::Deny {
                message: reason.clone(),
                reason: crate::permission::PermissionReason::Other {
                    reason: reason.clone(),
                },
            };
            self.audit_decision(&store, &ctx, tool.name(), &validated_input, &permission)
                .await;
            ctx.store
                .append_event(AgentEvent::PermissionDenied {
                    run_id: run_id.to_string(),
                    tool: tool.name().to_string(),
                    reason: reason.clone(),
                    timestamp: Utc::now(),
                })
                .await;
            return ToolExecution {
                result: ToolResult::error(reason),
            };
        }

        let permission = self
            .permission_engine
            .decide(tool.as_ref(), &validated_input, &ctx)
            .await;
        let audited_permission = match permission {
            PermissionResult::Allow {
                updated_input: Value::Null,
                reason,
            } => PermissionResult::Allow {
                updated_input: validated_input.clone(),
                reason,
            },
            other => other,
        };
        let audit_input = match &audited_permission {
            PermissionResult::Allow { updated_input, .. } => updated_input,
            _ => &validated_input,
        };
        self.audit_decision(&store, &ctx, tool.name(), audit_input, &audited_permission)
            .await;

        match audited_permission {
            PermissionResult::Allow { updated_input, .. } => {
                let progress = ProgressSink::new(run_id, tool_use_id, store);
                match tool.call(updated_input, ctx.clone(), progress).await {
                    Ok(result) => ToolExecution {
                        result: truncate_large_result_if_needed(
                            result,
                            tool.as_ref(),
                            tool_use_id,
                            &ctx.workspace_root,
                        ),
                    },
                    Err(ToolError::Recoverable(message)) => ToolExecution {
                        result: ToolResult::error_with_recoverable(message, true),
                    },
                    Err(ToolError::PermissionDenied(message)) => ToolExecution {
                        result: ToolResult::error_with_recoverable(message, false),
                    },
                    Err(ToolError::Terminal(message)) => {
                        ctx.store
                            .update_run_status(&ctx.run.id, AgentRunStatus::Failed)
                            .await
                            .ok();
                        ToolExecution {
                            result: ToolResult {
                                content: json!({ "error": message }),
                                is_error: true,
                                metadata: Some(json!({ "recoverable": false })),
                            },
                        }
                    }
                    Err(ToolError::Aborted) => ToolExecution {
                        result: ToolResult::error("tool aborted"),
                    },
                }
            }
            PermissionResult::Ask {
                message, reason, ..
            } => {
                let permission_message = format!("Permission required for {}", tool.name());
                let permission = ctx
                    .store
                    .create_permission_request(&ctx.project_id, run_id, tool.name())
                    .await;
                ctx.store
                    .append_event(AgentEvent::PermissionRequested {
                        run_id: run_id.to_string(),
                        permission_id: permission.id.clone(),
                        tool: tool.name().to_string(),
                        reason: reason.summary(),
                        timestamp: Utc::now(),
                    })
                    .await;
                ctx.store
                    .append_event(AgentEvent::AgentMessage {
                        run_id: run_id.to_string(),
                        text: permission_message.clone(),
                        timestamp: Utc::now(),
                    })
                    .await;
                ctx.store
                    .append_conversation_item(
                        &ctx.project_id,
                        Some(run_id),
                        "permission_requested",
                        Some("system"),
                        permission_message,
                        Some(json!({
                            "permissionId": permission.id,
                            "tool": tool.name(),
                            "reason": reason.summary(),
                        })),
                    )
                    .await;
                ctx.store
                    .append_event(AgentEvent::StateChanged {
                        run_id: run_id.to_string(),
                        state: "needs_user_input".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await;
                ctx.store
                    .update_run_status(run_id, AgentRunStatus::NeedsUserInput)
                    .await
                    .ok();
                ToolExecution {
                    result: ToolResult::error(message),
                }
            }
            PermissionResult::Deny { message, reason } => {
                ctx.store
                    .append_event(AgentEvent::PermissionDenied {
                        run_id: run_id.to_string(),
                        tool: tool.name().to_string(),
                        reason: reason.summary(),
                        timestamp: Utc::now(),
                    })
                    .await;
                ctx.store
                    .append_conversation_item(
                        &ctx.project_id,
                        Some(run_id),
                        "permission_denied",
                        Some("system"),
                        format!("Permission denied for {}: {message}", tool.name()),
                        Some(json!({
                            "tool": tool.name(),
                            "reason": reason.summary(),
                        })),
                    )
                    .await;
                ToolExecution {
                    result: ToolResult::error(message),
                }
            }
            PermissionResult::Passthrough { message } => ToolExecution {
                result: ToolResult::error(message),
            },
        }
    }

    fn tool_context(&self, store: RuntimeStore, run: AgentRun) -> ToolContext {
        let mut ctx = ToolContext::new(store, run, (*self.workspace_root).clone());
        ctx.policy_profile = self.policy_profile;
        ctx.npm_registry = (*self.npm_registry).clone();
        ctx
    }

    async fn audit_decision(
        &self,
        store: &RuntimeStore,
        ctx: &ToolContext,
        tool_name: &str,
        input: &Value,
        permission: &PermissionResult,
    ) {
        let decision = match permission {
            PermissionResult::Passthrough { .. } => "ask",
            _ => permission.decision(),
        };
        store
            .append_audit_record(
                &ctx.project_id,
                &ctx.run.id,
                tool_name,
                summarize_input(input),
                decision,
                permission.reason_summary(),
            )
            .await;
    }
}

fn normalize_tool_input(mut input: Value) -> Value {
    for _ in 0..3 {
        if !input.as_object().is_some_and(|object| object.len() == 1) {
            return input;
        };
        let Some(arguments) = input.get("arguments") else {
            return input;
        };
        match arguments {
            Value::String(arguments) => match serde_json::from_str::<Value>(arguments) {
                Ok(parsed) => input = parsed,
                Err(_) => return input,
            },
            Value::Object(_) => input = arguments.clone(),
            _ => return input,
        };
    }
    input
}

fn normalize_workspace_root(workspace_root: PathBuf) -> PathBuf {
    fs::canonicalize(&workspace_root).unwrap_or(workspace_root)
}

fn summarize_input(input: &Value) -> String {
    match input {
        Value::Object(map) => {
            let keys = map.keys().cloned().collect::<Vec<_>>().join(",");
            let mut summary = format!("object keys=[{keys}]");
            for key in ["path", "cwd", "registry"] {
                if let Some(value) = map.get(key).and_then(Value::as_str) {
                    summary.push_str(&format!(" {key}={}", truncate_summary(value)));
                }
            }
            if let Some(values) = map.get("argv").and_then(Value::as_array) {
                let argv = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(truncate_summary)
                    .collect::<Vec<_>>()
                    .join(" ");
                summary.push_str(&format!(" argv=[{argv}]"));
            }
            if let Some(values) = map.get("packages").and_then(Value::as_array) {
                let packages = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(truncate_summary)
                    .collect::<Vec<_>>()
                    .join(",");
                summary.push_str(&format!(" packages=[{packages}]"));
            }
            summary
        }
        Value::Array(values) => format!("array len={}", values.len()),
        Value::String(value) => format!("string len={}", value.len()),
        Value::Null => "null".to_string(),
        Value::Bool(_) | Value::Number(_) => input.to_string(),
    }
}

fn truncate_summary(value: &str) -> String {
    const MAX: usize = 120;
    if value.len() <= MAX {
        return value.to_string();
    }
    format!("{}...", &value[..MAX])
}

fn truncate_large_result_if_needed(
    result: ToolResult,
    tool: &dyn Tool,
    tool_use_id: &str,
    workspace_root: &Path,
) -> ToolResult {
    if result.is_error {
        return result;
    }

    let serialized = serde_json::to_string_pretty(&result.content)
        .unwrap_or_else(|_| result.content.to_string());
    let limit = tool.max_result_size_chars();
    if serialized.chars().count() <= limit {
        return result;
    }

    let artifact_dir = workspace_root.join("outputs/tool-results");
    let artifact_name = format!("{tool_use_id}.json");
    let artifact_path = artifact_dir.join(&artifact_name);
    let workspace_path = format!("/workspace/outputs/tool-results/{artifact_name}");
    if let Err(error) = fs::create_dir_all(&artifact_dir)
        .and_then(|_| fs::write(&artifact_path, serialized.as_bytes()))
    {
        return ToolResult::error(format!("failed to persist oversized tool result: {error}"));
    }

    let preview = serialized.chars().take(2000).collect::<String>();
    ToolResult {
        content: json!({
            "truncated": true,
            "path": workspace_path,
            "preview": preview,
            "originalChars": serialized.chars().count(),
            "limitChars": limit,
        }),
        is_error: false,
        metadata: Some(json!({
            "truncated": true,
            "fullResultPath": artifact_path.display().to_string(),
        })),
    }
}
