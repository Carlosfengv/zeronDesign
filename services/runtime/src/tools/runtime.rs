use crate::{
    agent_hooks::{PreToolUseHook, PreToolUseObservation},
    config::RuntimePolicyProfile,
    conversation::{RuntimeStore, ToolExecutionCompletion, ToolExecutionReservation},
    design_context::frozen_run_design_context_manifest,
    model_gateway::ModelToolDefinition,
    permission::{PermissionEngine, PermissionReason, PermissionResult, PermissionRules},
    profiles::policy,
    types::{
        canonical_json_hash, AgentEvent, AgentPhase, AgentRun, AgentRunStatus, ObservationOutcome,
        ObservationPurpose, ObservationReceipt, ObservationView, PendingPermission, TranscriptMode,
        OBSERVATION_RECEIPT_SCHEMA,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
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
    RecoverableWithMetadata {
        message: String,
        error_kind: String,
        metadata: Value,
    },
    Terminal(String),
    TerminalWithMetadata {
        message: String,
        error_kind: String,
        metadata: Value,
    },
    PermissionDenied(String),
    Aborted,
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            Self::Recoverable(message)
            | Self::RecoverableWithMetadata { message, .. }
            | Self::Terminal(message)
            | Self::TerminalWithMetadata { message, .. }
            | Self::PermissionDenied(message) => message.clone(),
            Self::Aborted => "tool aborted".to_string(),
        }
    }

    pub fn typed_recoverable(
        message: impl Into<String>,
        error_kind: impl Into<String>,
        metadata: Value,
    ) -> Self {
        Self::RecoverableWithMetadata {
            message: message.into(),
            error_kind: error_kind.into(),
            metadata,
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
    pub runtime_public_base_url: String,
    pub runtime_browser_proxy_base_url: String,
    pub runtime_storage_dir: PathBuf,
    pub allow_runtime_owned_writes: bool,
    pub remote_workspace: bool,
}

impl ToolContext {
    pub fn new(store: RuntimeStore, run: AgentRun, workspace_root: PathBuf) -> Self {
        let should_avoid_permission_prompts =
            run.profile_snapshot.transcript_mode == TranscriptMode::Sidechain;
        let runtime_storage_dir = workspace_root.join(".runtime-storage");
        Self {
            project_id: run.project_id.clone(),
            store,
            run,
            should_avoid_permission_prompts,
            workspace_root,
            policy_profile: RuntimePolicyProfile::Production,
            npm_registry: "https://registry.internal.example/npm/".to_string(),
            runtime_public_base_url: "http://127.0.0.1:8080".to_string(),
            runtime_browser_proxy_base_url: "http://127.0.0.1:8081".to_string(),
            runtime_storage_dir,
            allow_runtime_owned_writes: false,
            remote_workspace: false,
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
        let _ = self
            .store
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
        let _ = self
            .store
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
    runtime_public_base_url: Arc<String>,
    runtime_browser_proxy_base_url: Arc<String>,
    remote_workspace: bool,
    runtime_storage_dir: Arc<PathBuf>,
    runtime_storage_overridden: bool,
    observation_receipts_enabled: bool,
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
        let workspace_root = normalize_workspace_root(workspace_root.into());
        Self {
            tools: Arc::new(map),
            permission_engine: PermissionEngine::new(permission_rules),
            runtime_storage_dir: Arc::new(workspace_root.join(".runtime-storage")),
            runtime_storage_overridden: false,
            observation_receipts_enabled: false,
            workspace_root: Arc::new(workspace_root),
            policy_profile: RuntimePolicyProfile::Production,
            npm_registry: Arc::new("https://registry.internal.example/npm/".to_string()),
            runtime_public_base_url: Arc::new("http://127.0.0.1:8080".to_string()),
            runtime_browser_proxy_base_url: Arc::new("http://127.0.0.1:8081".to_string()),
            remote_workspace: false,
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
            runtime_public_base_url: self.runtime_public_base_url.clone(),
            runtime_browser_proxy_base_url: self.runtime_browser_proxy_base_url.clone(),
            remote_workspace: self.remote_workspace,
            runtime_storage_dir: self.runtime_storage_dir.clone(),
            runtime_storage_overridden: self.runtime_storage_overridden,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_workspace_root(&self, workspace_root: impl AsRef<Path>) -> Self {
        let workspace_root = normalize_workspace_root(workspace_root.as_ref().to_path_buf());
        let runtime_storage_dir = if self.runtime_storage_overridden {
            self.runtime_storage_dir.clone()
        } else {
            Arc::new(workspace_root.join(".runtime-storage"))
        };
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: Arc::new(workspace_root),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
            runtime_public_base_url: self.runtime_public_base_url.clone(),
            runtime_browser_proxy_base_url: self.runtime_browser_proxy_base_url.clone(),
            remote_workspace: self.remote_workspace,
            runtime_storage_dir,
            runtime_storage_overridden: self.runtime_storage_overridden,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_runtime_public_base_url(&self, base_url: impl Into<String>) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: self.workspace_root.clone(),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
            runtime_public_base_url: Arc::new(base_url.into().trim_end_matches('/').to_string()),
            runtime_browser_proxy_base_url: self.runtime_browser_proxy_base_url.clone(),
            remote_workspace: self.remote_workspace,
            runtime_storage_dir: self.runtime_storage_dir.clone(),
            runtime_storage_overridden: self.runtime_storage_overridden,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_runtime_browser_proxy_base_url(&self, base_url: impl Into<String>) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: self.workspace_root.clone(),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
            runtime_public_base_url: self.runtime_public_base_url.clone(),
            runtime_browser_proxy_base_url: Arc::new(
                base_url.into().trim_end_matches('/').to_string(),
            ),
            remote_workspace: self.remote_workspace,
            runtime_storage_dir: self.runtime_storage_dir.clone(),
            runtime_storage_overridden: self.runtime_storage_overridden,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_remote_workspace(&self, remote_workspace: bool) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: self.workspace_root.clone(),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
            runtime_public_base_url: self.runtime_public_base_url.clone(),
            runtime_browser_proxy_base_url: self.runtime_browser_proxy_base_url.clone(),
            remote_workspace,
            runtime_storage_dir: self.runtime_storage_dir.clone(),
            runtime_storage_overridden: self.runtime_storage_overridden,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_runtime_storage_dir(&self, runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            tools: self.tools.clone(),
            permission_engine: self.permission_engine.clone(),
            workspace_root: self.workspace_root.clone(),
            policy_profile: self.policy_profile,
            npm_registry: self.npm_registry.clone(),
            runtime_public_base_url: self.runtime_public_base_url.clone(),
            runtime_browser_proxy_base_url: self.runtime_browser_proxy_base_url.clone(),
            remote_workspace: self.remote_workspace,
            runtime_storage_dir: Arc::new(runtime_storage_dir.into()),
            runtime_storage_overridden: true,
            observation_receipts_enabled: self.observation_receipts_enabled,
        }
    }

    pub fn with_observation_receipts_enabled(&self, enabled: bool) -> Self {
        let mut executor = self.clone();
        executor.observation_receipts_enabled = enabled;
        executor
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn is_remote_workspace(&self) -> bool {
        self.remote_workspace
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
        let mut ctx = self.tool_context(store.clone(), run);
        ctx.allow_runtime_owned_writes = tool_use_id.starts_with("bootstrap:");
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
        if replan_terminal_tool(tool.name()) && run_replan_required(&store, run_id).await {
            return ToolExecution {
                result: ToolResult::typed_error(
                    format!(
                        "{} is blocked because this Run requires a replacement EditImpactPlan",
                        tool.name()
                    ),
                    "edit.replan_required",
                    false,
                    json!({
                        "runId": run_id,
                        "tool": tool.name(),
                        "suggestedAction": "Create a successor Run with a new frozen Plan and Run Context Binding."
                    }),
                ),
            };
        }
        let approved_permission = store
            .approved_permission_for_tool(run_id, tool.name(), tool_use_id, &input)
            .await;
        let input = approved_permission
            .as_ref()
            .and_then(|permission| {
                permission
                    .resolved_input
                    .clone()
                    .or_else(|| permission.requested_input.clone())
            })
            .map(normalize_tool_input)
            .unwrap_or(input);
        if ctx.run.status == AgentRunStatus::Validating
            && candidate_freeze_blocks(tool.name())
            && !ctx.allow_runtime_owned_writes
        {
            let message = format!(
                "{} is blocked because the current build candidate is frozen",
                tool.name()
            );
            store
                .append_audit_record(
                    &ctx.project_id,
                    &ctx.run.id,
                    tool.name(),
                    summarize_input(&input),
                    "deny",
                    "candidate freeze rejected mutation",
                )
                .await;
            return ToolExecution {
                result: ToolResult::typed_error(
                    message,
                    "project.candidate_frozen",
                    true,
                    json!({
                        "runId": ctx.run.id,
                        "tool": tool.name(),
                        "suggestedAction": "Continue preview validation and promotion, or start an Edit/Repair run before mutating source."
                    }),
                ),
            };
        }
        if let Some(result) = reject_duplicate_promotion_after_run_output(&ctx, tool.name()) {
            store
                .append_audit_record(
                    &ctx.project_id,
                    &ctx.run.id,
                    tool.name(),
                    summarize_input(&input),
                    "deny",
                    "pre-tool lifecycle guard rejected duplicate promotion",
                )
                .await;
            return ToolExecution { result };
        }
        if !tool_use_id.starts_with("bootstrap:") {
            if let Some((message, error_kind, metadata)) =
                runtime_attestation_gate(&store, &ctx.run, tool.name(), &input).await
            {
                if matches!(
                    error_kind.as_str(),
                    "design_context.read_required" | "design_context.style_contract_unverified"
                ) {
                    let missing_file_count = metadata
                        .get("missingFiles")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    let missing_section_count = metadata
                        .get("missingSectionIds")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    record_design_context_metric(
                        &store,
                        &ctx.run,
                        "design_context_required_read_block_total",
                        1,
                        json!({
                            "tool": tool.name(),
                            "reason": if error_kind == "design_context.read_required" {
                                "read_required"
                            } else {
                                "style_contract_unverified"
                            },
                            "missingFileCount": missing_file_count,
                            "missingSectionCount": missing_section_count,
                        }),
                    )
                    .await;
                }
                store
                    .append_audit_record(
                        &ctx.project_id,
                        &ctx.run.id,
                        tool.name(),
                        summarize_input(&input),
                        "deny",
                        format!("design context read gate rejected ({error_kind}): {message}"),
                    )
                    .await;
                return ToolExecution {
                    result: ToolResult::typed_error(message, error_kind, true, metadata),
                };
            }
        }

        let pre_tool_decision = PreToolUseHook.apply(PreToolUseObservation {
            phase: ctx.run.phase,
            tool_name: tool.name().to_string(),
            input,
            default_cwd: Some(default_tool_cwd(&ctx.run)),
        });
        let input = match pre_tool_decision.rejection {
            Some(rejection) => {
                store
                    .append_audit_record(
                        &ctx.project_id,
                        &ctx.run.id,
                        tool.name(),
                        summarize_input(&pre_tool_decision.input),
                        "deny",
                        format!(
                            "pre-tool hook rejected ({error_kind}): {message}",
                            error_kind = rejection.error_kind,
                            message = rejection.message
                        ),
                    )
                    .await;
                return ToolExecution {
                    result: ToolResult::typed_error(
                        rejection.message,
                        rejection.error_kind,
                        rejection.recoverable,
                        rejection.metadata,
                    ),
                };
            }
            None => pre_tool_decision.input,
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
            let _ = ctx
                .store
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
        let audited_permission = match (approved_permission.as_ref(), permission) {
            (Some(permission), PermissionResult::Ask { .. })
            | (Some(permission), PermissionResult::Passthrough { .. }) => PermissionResult::Allow {
                updated_input: validated_input.clone(),
                reason: PermissionReason::Other {
                    reason: format!("approved by permission {}", permission.id),
                },
            },
            (
                _,
                PermissionResult::Allow {
                    updated_input: Value::Null,
                    reason,
                },
            ) => PermissionResult::Allow {
                updated_input: validated_input.clone(),
                reason,
            },
            (_, other) => other,
        };
        let audit_input = match &audited_permission {
            PermissionResult::Allow { updated_input, .. } => updated_input,
            _ => &validated_input,
        };
        self.audit_decision(&store, &ctx, tool.name(), audit_input, &audited_permission)
            .await;

        match audited_permission {
            PermissionResult::Allow { updated_input, .. } => {
                if !ctx.allow_runtime_owned_writes
                    && edit_plan_mutation_tool(tool.name(), &updated_input)
                {
                    let target = mutation_target(&updated_input);
                    match store
                        .preflight_edit_mutation(run_id, target.as_deref())
                        .await
                    {
                        Ok(updated_run) => ctx.run = updated_run,
                        Err(error) => {
                            let message = error.to_string();
                            let error_kind = if message.contains("edit.plan_scope_violation") {
                                "edit.plan_scope_violation"
                            } else {
                                "edit.plan_stale"
                            };
                            let replan_required = error_kind == "edit.plan_stale"
                                || ctx.run.phase == crate::types::AgentPhase::Repair;
                            if replan_required {
                                mark_run_replan_required(
                                    &store,
                                    &ctx.run,
                                    tool.name(),
                                    target.as_deref(),
                                    error_kind,
                                )
                                .await;
                            }
                            return ToolExecution {
                                result: ToolResult::typed_error(
                                    message,
                                    error_kind,
                                    true,
                                    json!({
                                        "runId": run_id,
                                        "target": target,
                                        "replanRequired": replan_required,
                                        "suggestedAction": if replan_required {
                                            "Stop mutation in this Run and create a successor Run bound to a current EditImpactPlan."
                                        } else {
                                            "Use a target declared by the frozen EditImpactPlan."
                                        }
                                    }),
                                ),
                            };
                        }
                    }
                }
                let progress = ProgressSink::new(run_id, tool_use_id, store.clone());
                let tracked_input = updated_input.clone();
                let input_hash = (!tool.is_read_only(&tracked_input)).then(|| {
                    canonical_json_hash(&json!({
                        "tool": tool.name(),
                        "input": tracked_input.clone(),
                    }))
                });
                if let Some(input_hash) = input_hash.as_deref() {
                    match store
                        .reserve_tool_execution(run_id, tool_use_id, tool.name(), input_hash)
                        .await
                    {
                        Ok(ToolExecutionReservation::Reserved) => {}
                        Ok(ToolExecutionReservation::Replay(record)) => {
                            if let Some(error) = consume_permission_after_execution(
                                &store,
                                approved_permission.as_ref(),
                            )
                            .await
                            {
                                return ToolExecution { result: error };
                            }
                            let Some(content) = record.result_content else {
                                ctx.store
                                    .update_run_status(&ctx.run.id, AgentRunStatus::Failed)
                                    .await
                                    .ok();
                                return ToolExecution {
                                    result: ToolResult::typed_error(
                                        "persisted tool result is missing content",
                                        "tool.execution_record_corrupt",
                                        false,
                                        json!({
                                            "tool": tool.name(),
                                            "toolUseId": tool_use_id,
                                        }),
                                    ),
                                };
                            };
                            return ToolExecution {
                                result: ToolResult {
                                    content,
                                    is_error: record.result_is_error,
                                    metadata: record.result_metadata,
                                },
                            };
                        }
                        Ok(ToolExecutionReservation::InDoubt(_)) => {
                            pause_run_for_uncertain_tool_execution(
                                &store,
                                &ctx.run,
                                tool.name(),
                                tool_use_id,
                                "A previous execution started but did not durably record its result",
                            )
                            .await;
                            return ToolExecution {
                                result: uncertain_tool_execution_result(tool.name(), tool_use_id),
                            };
                        }
                        Ok(ToolExecutionReservation::Conflict(_)) => {
                            ctx.store
                                .update_run_status(&ctx.run.id, AgentRunStatus::Failed)
                                .await
                                .ok();
                            return ToolExecution {
                                result: ToolResult::typed_error(
                                    "tool_use_id was reused with a different tool or input",
                                    "tool.execution_identity_conflict",
                                    false,
                                    json!({
                                        "tool": tool.name(),
                                        "toolUseId": tool_use_id,
                                    }),
                                ),
                            };
                        }
                        Err(error) => {
                            pause_run_for_uncertain_tool_execution(
                                &store,
                                &ctx.run,
                                tool.name(),
                                tool_use_id,
                                &format!("Tool execution could not be durably reserved: {error}"),
                            )
                            .await;
                            return ToolExecution {
                                result: uncertain_tool_execution_result(tool.name(), tool_use_id),
                            };
                        }
                    }
                }
                let mut execution = match tool.call(updated_input, ctx.clone(), progress).await {
                    Ok(result) => ToolExecution {
                        result: truncate_large_result_if_needed(
                            result,
                            tool.as_ref(),
                            tool_use_id,
                            &ctx.runtime_storage_dir,
                            &ctx.run.id,
                        ),
                    },
                    Err(ToolError::Recoverable(message)) => ToolExecution {
                        result: ToolResult::error_with_recoverable(message, true),
                    },
                    Err(ToolError::RecoverableWithMetadata {
                        message,
                        error_kind,
                        metadata,
                    }) => ToolExecution {
                        result: ToolResult::typed_error(message, error_kind, true, metadata),
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
                    Err(ToolError::TerminalWithMetadata {
                        message,
                        error_kind,
                        metadata,
                    }) => {
                        ctx.store
                            .update_run_status(&ctx.run.id, AgentRunStatus::Failed)
                            .await
                            .ok();
                        ToolExecution {
                            result: ToolResult::typed_error(message, error_kind, false, metadata),
                        }
                    }
                    Err(ToolError::Aborted) => ToolExecution {
                        result: ToolResult::error("tool aborted"),
                    },
                };
                if execution
                    .result
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("errorKind"))
                    .and_then(Value::as_str)
                    == Some("design_verification_runtime_lost")
                {
                    record_design_context_metric(
                        &store,
                        &ctx.run,
                        "design_context_verifier_unavailable_total",
                        1,
                        json!({
                            "reason": "runtime_lost",
                            "tool": tool.name(),
                        }),
                    )
                    .await;
                }
                if !execution.result.is_error {
                    if self.observation_receipts_enabled {
                        deduplicate_read_delivery(
                            &store,
                            &ctx.run,
                            tool.name(),
                            &mut execution.result,
                        )
                        .await;
                        record_observation_receipt(
                            &store,
                            &ctx.run,
                            tool.name(),
                            &execution.result,
                        )
                        .await;
                    }
                    record_design_context_read(
                        &store,
                        &ctx.run,
                        tool.name(),
                        &tracked_input,
                        &execution.result,
                    )
                    .await;
                }
                let persistence = if let Some(input_hash) = input_hash.as_deref() {
                    store
                        .complete_tool_execution(
                            run_id,
                            tool_use_id,
                            tool.name(),
                            input_hash,
                            ToolExecutionCompletion {
                                content: execution.result.content.clone(),
                                is_error: execution.result.is_error,
                                metadata: execution.result.metadata.clone(),
                            },
                        )
                        .await
                        .map(|_| ())
                } else {
                    Ok(())
                };
                match persistence {
                    Ok(_) => {
                        if let Some(error) =
                            consume_permission_after_execution(&store, approved_permission.as_ref())
                                .await
                        {
                            return ToolExecution { result: error };
                        }
                        execution
                    }
                    Err(error) => {
                        pause_run_for_uncertain_tool_execution(
                            &store,
                            &ctx.run,
                            tool.name(),
                            tool_use_id,
                            &format!("Tool result could not be durably recorded: {error}"),
                        )
                        .await;
                        ToolExecution {
                            result: uncertain_tool_execution_result(tool.name(), tool_use_id),
                        }
                    }
                }
            }
            PermissionResult::Ask {
                message, reason, ..
            } => {
                let permission_message = format!("Permission required for {}", tool.name());
                let permission = ctx
                    .store
                    .create_tool_permission_request(
                        &ctx.project_id,
                        run_id,
                        tool.name(),
                        Some(tool_use_id),
                        Some(validated_input.clone()),
                    )
                    .await;
                let _ = ctx
                    .store
                    .append_event(AgentEvent::PermissionRequested {
                        run_id: run_id.to_string(),
                        permission_id: permission.id.clone(),
                        tool: tool.name().to_string(),
                        reason: reason.summary(),
                        timestamp: Utc::now(),
                    })
                    .await;
                let _ = ctx
                    .store
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
                let _ = ctx
                    .store
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
                let reason_summary = reason.summary();
                let _ = ctx
                    .store
                    .append_event(AgentEvent::PermissionDenied {
                        run_id: run_id.to_string(),
                        tool: tool.name().to_string(),
                        reason: reason_summary.clone(),
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
                            "reason": reason_summary,
                        })),
                    )
                    .await;
                if let Some((error_kind, metadata)) =
                    typed_permission_denial_metadata(tool.name(), &message, &validated_input)
                {
                    return ToolExecution {
                        result: ToolResult::typed_error(message, error_kind, true, metadata),
                    };
                }
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
        ctx.runtime_public_base_url = (*self.runtime_public_base_url).clone();
        ctx.runtime_browser_proxy_base_url = (*self.runtime_browser_proxy_base_url).clone();
        ctx.remote_workspace = self.remote_workspace;
        ctx.runtime_storage_dir = (*self.runtime_storage_dir).clone();
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

const OBSERVATION_DELIVERY_MAX_ENTRIES: usize = 100;
const OBSERVATION_DELIVERY_MAX_BYTES: u64 = 25 * 1024 * 1024;

async fn deduplicate_read_delivery(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    result: &mut ToolResult,
) {
    if tool_name != "fs.read" {
        return;
    }
    let Some(path) = result.content.get("path").and_then(Value::as_str) else {
        return;
    };
    let Some(text) = result.content.get("text").and_then(Value::as_str) else {
        return;
    };
    let normalized_path = normalize_design_context_path(path);
    let content_sha256 = crate::types::sha256_hex(text.as_bytes());
    let events = store.events(&run.id).await;
    let epoch = current_context_window_epoch(&events);
    let Some(previous) =
        visible_full_observation(&events, &normalized_path, &content_sha256, epoch)
    else {
        return;
    };
    result.content = json!({
        "path": path,
        "contentSha256": content_sha256,
        "contextWindowEpoch": epoch,
        "unchanged": true,
        "alreadyObservedAtTurn": previous.first_read_turn,
        "suggestedAction": "Use the previously observed content or mutate the file."
    });
}

fn current_context_window_epoch(events: &[AgentEvent]) -> u64 {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MetricRecorded { name, value, .. }
                if name == "context_window_epoch_advanced" =>
            {
                Some(*value)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn visible_full_observation<'a>(
    events: &'a [AgentEvent],
    path: &str,
    content_sha256: &str,
    epoch: u64,
) -> Option<&'a ObservationReceipt> {
    let mut entries = 0usize;
    let mut bytes = 0u64;
    for event in events.iter().rev() {
        let AgentEvent::ObservationReceipt { receipt, .. } = event else {
            continue;
        };
        if receipt.context_window_epoch != epoch
            || receipt.view != ObservationView::Full
            || receipt.last_outcome != ObservationOutcome::ContentReturned
        {
            continue;
        }
        entries = entries.saturating_add(1);
        bytes = bytes.saturating_add(receipt.delivered_bytes);
        if entries > OBSERVATION_DELIVERY_MAX_ENTRIES || bytes > OBSERVATION_DELIVERY_MAX_BYTES {
            break;
        }
        if receipt.normalized_path == path && receipt.content_sha256 == content_sha256 {
            return Some(receipt);
        }
    }
    None
}

async fn record_observation_receipt(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    result: &ToolResult,
) {
    let deliveries = match tool_name {
        "fs.read" => result
            .content
            .get("path")
            .and_then(Value::as_str)
            .map(|path| {
                vec![(
                    path.to_string(),
                    result
                        .content
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    result
                        .content
                        .get("contentSha256")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    result
                        .content
                        .get("unchanged")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )]
            })
            .unwrap_or_default(),
        "project.init" => result
            .content
            .get("sourceObservations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|observation| {
                Some((
                    observation.get("path")?.as_str()?.to_string(),
                    observation
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    observation
                        .get("contentSha256")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    false,
                ))
            })
            .collect(),
        _ => Vec::new(),
    };
    if deliveries.is_empty() {
        return;
    }
    let events = store.events(&run.id).await;
    let turn = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelTurnStarted { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let context_window_epoch = current_context_window_epoch(&events);
    for (path, text, declared_sha256, unchanged) in deliveries {
        let normalized_path = normalize_design_context_path(&path);
        let computed_sha256 = text
            .as_deref()
            .map(|text| crate::types::sha256_hex(text.as_bytes()));
        if computed_sha256.is_some()
            && declared_sha256.is_some()
            && computed_sha256 != declared_sha256
        {
            continue;
        }
        let Some(content_sha256) = computed_sha256.or(declared_sha256) else {
            continue;
        };
        let previous = events.iter().rev().find_map(|event| match event {
            AgentEvent::ObservationReceipt { receipt, .. }
                if receipt.normalized_path == normalized_path
                    && receipt.content_sha256 == content_sha256
                    && receipt.context_window_epoch == context_window_epoch
                    && receipt.view == ObservationView::Full =>
            {
                Some(receipt)
            }
            _ => None,
        });
        let delivered_bytes = text.as_deref().map_or(0, |text| text.len() as u64);
        let estimated_tokens = text
            .as_deref()
            .map_or(0, |text| (text.chars().count() as u64).div_ceil(4));
        let receipt = ObservationReceipt {
            schema_version: OBSERVATION_RECEIPT_SCHEMA.to_string(),
            run_id: run.id.clone(),
            normalized_path: normalized_path.clone(),
            content_sha256,
            context_window_epoch,
            view: ObservationView::Full,
            last_outcome: if unchanged {
                ObservationOutcome::Unchanged
            } else {
                ObservationOutcome::ContentReturned
            },
            first_read_turn: previous
                .map(|receipt| receipt.first_read_turn)
                .unwrap_or(turn),
            last_read_turn: turn,
            read_count: previous
                .map(|receipt| receipt.read_count.saturating_add(1))
                .unwrap_or(1),
            purpose: observation_purpose(&normalized_path),
            delivered_bytes,
            estimated_tokens,
            duplicate_delivery: unchanged,
        };
        let _ = store
            .append_event(AgentEvent::ObservationReceipt {
                run_id: run.id.clone(),
                receipt,
                timestamp: Utc::now(),
            })
            .await;
    }
}

fn observation_purpose(path: &str) -> ObservationPurpose {
    if path.starts_with("inputs/")
        || matches!(
            path,
            "state/style-contract.json" | "state/context.md" | "state/project.json"
        )
    {
        ObservationPurpose::Context
    } else if path == "state/read-tracking.json" {
        ObservationPurpose::RuntimeInternal
    } else if path.contains("diagnostic")
        || path.contains("build-log")
        || path.ends_with("build.log")
        || path.starts_with("state/logs/")
    {
        ObservationPurpose::Diagnostic
    } else {
        ObservationPurpose::Source
    }
}

fn replan_terminal_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "fs.write"
            | "fs.write_chunk"
            | "fs.commit_chunks"
            | "fs.patch"
            | "fs.multi_patch"
            | "fs.delete"
            | "project.init"
            | "project.write_page"
            | "project.build"
            | "project.ensure_dependencies"
            | "style.update_tokens"
            | "component.apply"
            | "package.install"
            | "shell.run"
            | "preview.dev_start"
            | "preview.publish"
            | "draft.snapshot_create"
            | "run.complete"
    )
}

async fn run_replan_required(store: &RuntimeStore, run_id: &str) -> bool {
    store.events(run_id).await.iter().rev().any(|event| {
        matches!(
            event,
            AgentEvent::RunWorkflowProgress { stage, .. } if stage == "replan_required"
        )
    })
}

async fn mark_run_replan_required(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    target: Option<&str>,
    error_kind: &str,
) {
    let events = store.events(&run.id).await;
    let turn = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelTurnStarted { turn, .. }
            | AgentEvent::RunWorkflowProgress { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let mut completed_steps = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::RunWorkflowProgress {
                completed_steps, ..
            } => Some(completed_steps.clone()),
            _ => None,
        })
        .unwrap_or_default();
    if !completed_steps.iter().any(|step| step == "replan_required") {
        completed_steps.push("replan_required".to_string());
    }
    let _ = store
        .append_event(AgentEvent::MetricRecorded {
            run_id: run.id.clone(),
            name: "edit_plan.replacement_required".to_string(),
            value: 1,
            metadata: Some(json!({
                "tool": tool_name,
                "target": target,
                "errorKind": error_kind,
                "blocking": true,
            })),
            timestamp: Utc::now(),
        })
        .await;
    let _ = store
        .append_event(AgentEvent::RunWorkflowProgress {
            run_id: run.id.clone(),
            turn,
            stage: "replan_required".to_string(),
            completed_steps,
            next_action: json!({
                "tool": "orchestrator.create_successor_run",
                "reason": "the frozen Plan cannot authorize the required target"
            }),
            budgets: json!({}),
            timestamp: Utc::now(),
        })
        .await;
    let _ = store
        .append_event(AgentEvent::StateChanged {
            run_id: run.id.clone(),
            state: "partial:replan_required".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    let _ = store
        .update_run_status(&run.id, AgentRunStatus::Partial)
        .await;
}

async fn consume_permission_after_execution(
    store: &RuntimeStore,
    permission: Option<&PendingPermission>,
) -> Option<ToolResult> {
    let permission = permission?;
    match store.consume_approved_permission(&permission.id).await {
        Ok(true) => None,
        Ok(false) => Some(ToolResult::error_with_recoverable(
            "permission approval was already consumed",
            true,
        )),
        Err(error) => Some(ToolResult::error_with_recoverable(
            format!("failed to consume permission approval: {error}"),
            true,
        )),
    }
}

async fn pause_run_for_uncertain_tool_execution(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    tool_use_id: &str,
    reason: &str,
) {
    if !run.status.is_terminal() {
        store
            .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
            .await
            .ok();
    }
    let _ = store
        .append_event(AgentEvent::StateChanged {
            run_id: run.id.clone(),
            state: "needs_user_input:tool_execution_in_doubt".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    store
        .append_conversation_item(
            &run.project_id,
            Some(&run.id),
            "tool_execution_in_doubt",
            Some("system"),
            format!(
                "Execution outcome for {tool_name} is uncertain and requires reconciliation before retrying."
            ),
            Some(json!({
                "tool": tool_name,
                "toolUseId": tool_use_id,
                "reason": reason,
            })),
        )
        .await;
}

fn uncertain_tool_execution_result(tool_name: &str, tool_use_id: &str) -> ToolResult {
    ToolResult::typed_error(
        format!(
            "Execution outcome for {tool_name} is uncertain; the Runtime will not replay this tool call automatically"
        ),
        "tool.execution_in_doubt",
        false,
        json!({
            "tool": tool_name,
            "toolUseId": tool_use_id,
            "requiresReconciliation": true,
        }),
    )
}

fn candidate_freeze_blocks(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "fs.write"
            | "fs.write_chunk"
            | "fs.commit_chunks"
            | "fs.patch"
            | "fs.multi_patch"
            | "fs.delete"
            | "project.init"
            | "project.write_page"
            | "project.ensure_dependencies"
            | "project.build"
            | "style.update_tokens"
            | "package.install"
            | "shell.run"
    )
}

fn reject_duplicate_promotion_after_run_output(
    ctx: &ToolContext,
    tool_name: &str,
) -> Option<ToolResult> {
    if tool_name != "preview.report_candidate" {
        return None;
    }
    let version_id = ctx.run.output_version_id.as_ref()?;
    Some(ToolResult::typed_error(
        format!(
            "{tool_name} cannot create another promoted candidate after this run already promoted {version_id}"
        ),
        "preview.already_promoted",
        true,
        json!({
            "phase": format!("{:?}", ctx.run.phase),
            "tool": tool_name,
            "versionId": version_id,
            "suggestedAction": "Do not manually report another candidate after this run already promoted. If the promoted artifact satisfies the user request, call run.complete. If it does not, edit the source first, then use preview.publish to rebuild, screenshot, and promote the new source snapshot."
        }),
    ))
}

fn typed_permission_denial_metadata(
    tool_name: &str,
    message: &str,
    input: &Value,
) -> Option<(String, Value)> {
    if tool_name == "preview.report_candidate" && message.contains("retired outside local E2E") {
        return Some((
            "preview.manual_candidate_retired".to_string(),
            json!({
                "tool": tool_name,
                "suggestedAction": "Call preview.publish without URL, port, command, or mode arguments. After it returns candidate_ready, call run.complete so promotion and completion commit atomically.",
            }),
        ));
    }

    if tool_name == "shell.run" {
        return Some((
            "shell.command_denied".to_string(),
            json!({
                "tool": tool_name,
                "argv": input.get("argv").cloned().unwrap_or(Value::Null),
                "suggestedAction": "Use the dedicated runtime tool for this operation instead of shell.run.",
            }),
        ));
    }

    if message.contains("SecretPath") {
        return Some((
            "path.secret".to_string(),
            json!({
                "tool": tool_name,
                "receivedPath": input.get("path").and_then(Value::as_str).unwrap_or(""),
                "suggestedAction": "Choose a non-secret project source path.",
            }),
        ));
    }

    if message.contains("nested package root denied") {
        return Some((
            "path.nested_package_root".to_string(),
            json!({
                "tool": tool_name,
                "receivedPath": input.get("path").and_then(Value::as_str).unwrap_or(""),
                "suggestedAction": "Use the app root package.json instead of creating or editing nested package.json files.",
            }),
        ));
    }

    if message.contains("runtime-owned path cannot be mutated") {
        return Some((
            "path.runtime_owned".to_string(),
            json!({
                "tool": tool_name,
                "receivedPath": input.get("path").and_then(Value::as_str).unwrap_or(""),
                "suggestedAction": "Use the dedicated Runtime tool that owns this state."
            }),
        ));
    }

    None
}

fn default_tool_cwd(run: &AgentRun) -> String {
    run.project_state_snapshot
        .as_ref()
        .map(|state| state.app_root.clone())
        .and_then(|app_root| {
            let app_root = app_root
                .strip_prefix("/workspace/")
                .unwrap_or(app_root.as_str())
                .trim_start_matches('/');
            if app_root.is_empty() || app_root.contains("..") {
                None
            } else {
                Some(app_root.to_string())
            }
        })
        .unwrap_or_else(|| "project".to_string())
}

fn normalize_tool_input(mut input: Value) -> Value {
    for _ in 0..3 {
        if input.as_object().is_none_or(|object| object.len() != 1) {
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

fn design_context_read_gate(
    run: &AgentRun,
    tool_name: &str,
    input: &Value,
) -> Option<(String, String, Value)> {
    run.design_profile_id.as_ref()?;
    let dcp = match frozen_run_design_context_manifest(run) {
        Ok(manifest) => manifest,
        Err(message) => {
            return Some((
                message,
                "design_context.integrity_failed".to_string(),
                json!({ "runId": run.id }),
            ));
        }
    };
    if tool_name == "fs.read" {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .map(normalize_design_context_path)
            .unwrap_or_default();
        if path == "inputs/design-source.md" {
            if run.design_fidelity_mode.as_deref() != Some("source_fallback") {
                return Some((
                    "raw design source is not exposed in profile_only mode".to_string(),
                    "design_source.mode_forbidden".to_string(),
                    json!({ "requiredMode": "source_fallback" }),
                ));
            }
            if run.design_source_size_bytes.unwrap_or(0) > 32 * 1024 {
                return Some((
                    "large design source must be read through its index and design_source.read_sections"
                        .to_string(),
                    "design_source.index_required".to_string(),
                    json!({ "indexPath": "inputs/design-source-index.json" }),
                ));
            }
        }
    }
    if !design_context_gate_mutation(tool_name, input) {
        return None;
    }

    if let Some(manifest) = dcp.as_ref() {
        if tool_name == "project.init" {
            let requested_path = input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(manifest.payload.expected_app_root.as_str());
            if requested_path != manifest.payload.expected_app_root {
                return Some((
                    format!(
                        "project.init path must match the frozen DCP app root {}",
                        manifest.payload.expected_app_root
                    ),
                    "project.app_root_mismatch".to_string(),
                    json!({
                        "expectedAppRoot": manifest.payload.expected_app_root,
                        "receivedPath": requested_path,
                    }),
                ));
            }
            let requested_template = input.get("template").and_then(Value::as_str);
            if requested_template != Some(manifest.payload.template.as_str()) {
                return Some((
                    format!(
                        "project.init template must match the frozen DCP template {}",
                        manifest.payload.template
                    ),
                    "project.template_mismatch".to_string(),
                    json!({
                        "expectedTemplate": manifest.payload.template,
                        "receivedTemplate": requested_template,
                    }),
                ));
            }
        }
    }

    let mut required_files = match dcp.as_ref() {
        Some(manifest) => manifest
            .payload
            .required_reads
            .iter()
            .filter(|requirement| requirement.phases.contains(&run.phase))
            .map(|requirement| requirement.path.clone())
            .collect::<Vec<_>>(),
        None => match run.phase {
            AgentPhase::Build => vec![
                "inputs/brief.md".to_string(),
                "inputs/design.md".to_string(),
                "inputs/design-profile.json".to_string(),
            ],
            AgentPhase::Edit => vec!["inputs/design.md".to_string()],
            AgentPhase::Repair => vec!["inputs/design.md".to_string()],
            _ => Vec::new(),
        },
    };
    if dcp.is_some()
        && tool_name != "project.init"
        && matches!(
            run.phase,
            AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair
        )
    {
        if run.design_context_style_contract_verified != Some(true) {
            return Some((
                "state/style-contract.json must be verified against the frozen Design Context Package before Build/Edit/Repair mutations or publish"
                    .to_string(),
                "design_context.style_contract_unverified".to_string(),
                json!({
                    "styleContractPath": "state/style-contract.json",
                    "verified": run.design_context_style_contract_verified,
                }),
            ));
        }
        required_files.push("state/style-contract.json".to_string());
    }
    if run.design_fidelity_mode.as_deref() == Some("source_fallback") {
        if run.phase != AgentPhase::Edit {
            required_files.push("inputs/design-profile.json".to_string());
        }
        if run.design_source_size_bytes.unwrap_or(0) <= 32 * 1024 {
            required_files.push("inputs/design-source.md".to_string());
        } else {
            required_files.push("inputs/design-source-index.json".to_string());
        }
    }
    required_files.sort();
    required_files.dedup();
    let missing_files = required_files
        .into_iter()
        .filter(|path| {
            !run.design_context_read_files
                .iter()
                .any(|read| read == path)
        })
        .collect::<Vec<_>>();
    let missing_sections = if run.design_fidelity_mode.as_deref() == Some("source_fallback")
        && run.design_source_size_bytes.unwrap_or(0) > 32 * 1024
    {
        run.design_source_required_section_ids
            .iter()
            .filter(|section_id| {
                run.design_source_sections
                    .iter()
                    .find(|section| &section.id == *section_id)
                    .is_none_or(|section| {
                        !run.design_source_read_section_hashes
                            .iter()
                            .any(|hash| hash == &section.sha256)
                    })
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if missing_files.is_empty() && missing_sections.is_empty() {
        return None;
    }
    Some((
        format!(
            "read required design context before calling {tool_name}: missing files [{}], missing source sections [{}]",
            missing_files.join(", "),
            missing_sections.join(", ")
        ),
        "design_context.read_required".to_string(),
        json!({
            "missingFiles": missing_files,
            "missingSectionIds": missing_sections,
            "fidelityMode": run.design_fidelity_mode.as_deref(),
        }),
    ))
}

async fn runtime_attestation_gate(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    input: &Value,
) -> Option<(String, String, Value)> {
    if run.generation_context_binding_hash.is_none()
        || run.generation_context_runtime_mode.as_deref() != Some("enabled")
    {
        return design_context_read_gate(run, tool_name, input);
    }

    if let Some(blocked) = design_source_access_gate(run, tool_name, input) {
        return Some(blocked);
    }
    if !runtime_attestation_operation(run, tool_name, input) {
        return None;
    }

    let frozen_dcp = match frozen_run_design_context_manifest(run) {
        Ok(manifest) => manifest,
        Err(message) => {
            return Some((
                message,
                "runtime_attestation.frozen_resource_integrity_failed".to_string(),
                json!({ "runId": run.id }),
            ));
        }
    };
    let context = match run
        .generation_context
        .as_ref()
        .cloned()
        .ok_or_else(|| "bound GenerationContext payload is missing".to_string())
        .and_then(|value| {
            serde_json::from_value::<crate::generation_context::GenerationContext>(value)
                .map_err(|error| format!("bound GenerationContext is invalid: {error}"))
        })
        .and_then(|context| {
            crate::generation_context::validate_generation_context_binding(&context)
                .map_err(|error| error.to_string())?;
            Ok(context)
        }) {
        Ok(context) => context,
        Err(message) => {
            return Some((
                message,
                "runtime_attestation.run_start_invalid".to_string(),
                json!({ "runId": run.id }),
            ));
        }
    };
    if context.run_binding.run_id != run.id
        || context.run_binding.project_id != run.project_id
        || run.generation_context_content_hash.as_deref()
            != Some(context.context_content_hash.as_str())
        || run.generation_context_binding_hash.as_deref()
            != Some(context.run_context_binding_hash.as_str())
    {
        return Some((
            "RunStartAttestation does not match the persisted Run binding".to_string(),
            "runtime_attestation.run_binding_mismatch".to_string(),
            json!({ "runId": run.id }),
        ));
    }
    use crate::generation_context::AttestationState;
    let attestation = &context.attestation;
    if attestation.artifact_state == AttestationState::Failed
        || attestation.materialization_state == AttestationState::Failed
        || attestation.template_identity_state != AttestationState::Verified
        || attestation.app_root_declaration_state != AttestationState::Verified
        || attestation.content_approval_state != AttestationState::Verified
        || attestation.visual_bindings_state == AttestationState::Failed
    {
        return Some((
            "RunStartAttestation contains an unverified required Runtime requirement".to_string(),
            "runtime_attestation.run_start_unverified".to_string(),
            json!({
                "runId": run.id,
                "runtimeAttestationHash": attestation.runtime_attestation_hash,
            }),
        ));
    }
    if let Some(manifest) = frozen_dcp.as_ref() {
        let expected_hash = manifest.payload.artifact_manifest_hash.as_str();
        if run.design_context_materialization_hash.as_deref() != Some(expected_hash) {
            return Some((
                "the frozen Design Context package has not been materialized and verified"
                    .to_string(),
                "runtime_attestation.materialization_unverified".to_string(),
                json!({
                    "runId": run.id,
                    "expectedMaterializationHash": expected_hash,
                    "actualMaterializationHash": run.design_context_materialization_hash,
                    "attestationState": attestation.materialization_state,
                }),
            ));
        }
    }
    let approval = store.content_plan_approval_store().verify_exact(
        &run.project_id,
        &context.payload.content_plan.plan_id,
        context.payload.content_plan.revision,
        &context.payload.content_plan.content_hash,
    );
    let approval_verified = approval.as_ref().is_ok_and(|verification| {
        verification.state
            == crate::content_plan_approval::ContentPlanApprovalVerificationState::Verified
            && verification
                .approval
                .as_ref()
                .map(|approval| approval.approval_id.as_str())
                == Some(context.payload.content_plan.approval.approval_id.as_str())
    });
    if !approval_verified {
        return Some((
            "the exact Content Plan approval bound to this Run is no longer authoritative"
                .to_string(),
            "content_plan.approval_rejected".to_string(),
            json!({
                "planId": context.payload.content_plan.plan_id,
                "revision": context.payload.content_plan.revision,
                "contentHash": context.payload.content_plan.content_hash,
                "approvalId": context.payload.content_plan.approval.approval_id,
                "suggestedAction": "Create a new Run bound to the current approved Content Plan revision."
            }),
        ));
    }

    if tool_name == "project.init" {
        let requested_path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(context.payload.identity.app_root.as_str());
        let requested_template = input.get("template").and_then(Value::as_str);
        if requested_path != context.payload.identity.app_root {
            return Some((
                "project.init path does not match RunStartAttestation".to_string(),
                "project.app_root_mismatch".to_string(),
                json!({
                    "expectedAppRoot": context.payload.identity.app_root,
                    "receivedPath": requested_path,
                }),
            ));
        }
        if requested_template != Some(context.payload.identity.template_id.as_str()) {
            return Some((
                "project.init template does not match RunStartAttestation".to_string(),
                "project.template_mismatch".to_string(),
                json!({
                    "expectedTemplate": context.payload.identity.template_id,
                    "receivedTemplate": requested_template,
                }),
            ));
        }
    } else {
        let Some(project) = run.project_state_snapshot.as_ref() else {
            return Some((
                format!("{tool_name} requires ProjectRuntimeAttestation from project.init"),
                "project.runtime_attestation_missing".to_string(),
                json!({ "suggestedAction": "Run project.init before this operation." }),
            ));
        };
        if project.app_root != context.payload.identity.app_root
            || project.template_key != context.payload.identity.template_id
            || project.template_version != context.payload.identity.template_version
            || project.template_manifest_sha256.as_deref()
                != Some(context.payload.identity.template_manifest_hash.as_str())
        {
            return Some((
                "ProjectRuntimeAttestation does not match the frozen GenerationContext identity"
                    .to_string(),
                "project.runtime_attestation_mismatch".to_string(),
                json!({
                    "runId": run.id,
                    "sourceRevision": project.revision,
                    "expectedAppRoot": context.payload.identity.app_root,
                    "actualAppRoot": project.app_root,
                    "expectedTemplateManifestSha256": context.payload.identity.template_manifest_hash,
                    "actualTemplateManifestSha256": project.template_manifest_sha256,
                }),
            ));
        }
    }

    source_fallback_read_gate(run, tool_name)
}

fn design_source_access_gate(
    run: &AgentRun,
    tool_name: &str,
    input: &Value,
) -> Option<(String, String, Value)> {
    if tool_name != "fs.read" {
        return None;
    }
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .map(normalize_design_context_path)
        .unwrap_or_default();
    if path != "inputs/design-source.md" {
        return None;
    }
    if run.design_fidelity_mode.as_deref() != Some("source_fallback") {
        return Some((
            "raw design source is not exposed in profile_only mode".to_string(),
            "design_source.mode_forbidden".to_string(),
            json!({ "requiredMode": "source_fallback" }),
        ));
    }
    if run.design_source_size_bytes.unwrap_or(0) > 32 * 1024 {
        return Some((
            "large design source must be read through its index and design_source.read_sections"
                .to_string(),
            "design_source.index_required".to_string(),
            json!({ "indexPath": "inputs/design-source-index.json" }),
        ));
    }
    None
}

fn runtime_attestation_operation(run: &AgentRun, tool_name: &str, input: &Value) -> bool {
    match tool_name {
        "project.init"
        | "project.write_page"
        | "style.update_tokens"
        | "project.ensure_dependencies"
        | "project.build"
        | "preview.publish"
        | "preview.start"
        | "preview.dev_start"
        | "draft.snapshot_create"
        | "shell.run" => true,
        "fs.write" | "fs.write_chunk" | "fs.commit_chunks" | "fs.patch" | "fs.multi_patch"
        | "fs.delete" => input
            .get("path")
            .or_else(|| input.get("targetPath"))
            .and_then(Value::as_str)
            .map(normalize_design_context_path)
            .is_some_and(|path| {
                path == context_app_root(run)
                    || path.starts_with(&format!("{}/", context_app_root(run)))
                    || path == "state/style-contract.json"
            }),
        _ => false,
    }
}

fn edit_plan_mutation_tool(tool_name: &str, input: &Value) -> bool {
    matches!(
        tool_name,
        "fs.write"
            | "fs.commit_chunks"
            | "fs.patch"
            | "fs.multi_patch"
            | "fs.delete"
            | "project.write_page"
            | "style.update_tokens"
    ) || (tool_name == "project.ensure_dependencies"
        && input.get("mode").and_then(Value::as_str) == Some("add"))
}

fn mutation_target(input: &Value) -> Option<String> {
    input
        .get("path")
        .or_else(|| input.get("targetPath"))
        .and_then(Value::as_str)
        .map(normalize_design_context_path)
}

fn context_app_root(run: &AgentRun) -> &str {
    run.generation_context
        .as_ref()
        .and_then(|context| context.pointer("/payload/identity/appRoot"))
        .and_then(Value::as_str)
        .unwrap_or("project")
}

fn source_fallback_read_gate(run: &AgentRun, tool_name: &str) -> Option<(String, String, Value)> {
    if run.design_fidelity_mode.as_deref() != Some("source_fallback") {
        return None;
    }
    let mut missing_files = Vec::new();
    if run.design_source_size_bytes.unwrap_or(0) <= 32 * 1024 {
        if !run
            .design_context_read_files
            .iter()
            .any(|path| path == "inputs/design-source.md")
        {
            missing_files.push("inputs/design-source.md".to_string());
        }
    } else if !run
        .design_context_read_files
        .iter()
        .any(|path| path == "inputs/design-source-index.json")
    {
        missing_files.push("inputs/design-source-index.json".to_string());
    }
    let missing_sections = if run.design_source_size_bytes.unwrap_or(0) > 32 * 1024 {
        run.design_source_required_section_ids
            .iter()
            .filter(|section_id| {
                run.design_source_sections
                    .iter()
                    .find(|section| &section.id == *section_id)
                    .is_none_or(|section| {
                        !run.design_source_read_section_hashes
                            .iter()
                            .any(|hash| hash == &section.sha256)
                    })
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if missing_files.is_empty() && missing_sections.is_empty() {
        return None;
    }
    Some((
        format!(
            "read the exact Source Fallback material before calling {tool_name}: missing files [{}], missing sections [{}]",
            missing_files.join(", "),
            missing_sections.join(", ")
        ),
        "design_context.read_required".to_string(),
        json!({
            "scope": "source_fallback",
            "missingFiles": missing_files,
            "missingSectionIds": missing_sections,
        }),
    ))
}

fn design_context_gate_mutation(tool_name: &str, input: &Value) -> bool {
    match tool_name {
        "project.init"
        | "style.update_tokens"
        | "project.ensure_dependencies"
        | "project.build"
        | "preview.publish"
        | "shell.run" => true,
        "fs.write" | "fs.write_chunk" | "fs.commit_chunks" | "fs.patch" | "fs.multi_patch"
        | "fs.delete" => input
            .get("path")
            .or_else(|| input.get("targetPath"))
            .and_then(Value::as_str)
            .map(normalize_design_context_path)
            .is_some_and(|path| {
                path == "state/style-contract.json"
                    || path == "project"
                    || path.starts_with("project/")
            }),
        _ => false,
    }
}

async fn record_design_context_read(
    store: &RuntimeStore,
    run: &AgentRun,
    tool_name: &str,
    _input: &Value,
    result: &ToolResult,
) {
    if tool_name != "fs.read" || run.design_profile_id.is_none() {
        return;
    }
    if run.design_context_manifest.is_some() && run.design_context_materialization_hash.is_none() {
        return;
    }
    let Some(path) = result.content.get("path").and_then(Value::as_str) else {
        return;
    };
    let path = normalize_design_context_path(path);
    if !matches!(
        path.as_str(),
        "inputs/brief.md"
            | "inputs/design.md"
            | "inputs/design-profile.json"
            | "inputs/design-profile-usage.md"
            | "inputs/component-recipes.json"
            | "inputs/template-style-contract.json"
            | "state/style-contract.json"
            | "inputs/design-source.md"
            | "inputs/design-source-index.json"
    ) {
        return;
    }
    if let Err(error) = store.record_design_context_file_read(&run.id, &path).await {
        eprintln!(
            "failed to record design context read for {}: {error}",
            run.id
        );
        return;
    }
    if path == "state/style-contract.json" {
        let verified = frozen_style_contract_read_is_verified(run, result);
        if let Err(error) = store
            .set_run_design_context_style_contract_verified(&run.id, verified)
            .await
        {
            eprintln!(
                "failed to record style contract verification for {}: {error}",
                run.id
            );
        }
    }
    if path == "inputs/design-source.md"
        && run.design_fidelity_mode.as_deref() == Some("source_fallback")
    {
        let hashes = run
            .design_source_sections
            .iter()
            .map(|section| section.sha256.clone())
            .collect::<Vec<_>>();
        if let Err(error) = store
            .record_design_source_sections_read(
                &run.id,
                &hashes,
                run.design_source_size_bytes.unwrap_or(0),
            )
            .await
        {
            eprintln!(
                "failed to record full design source read for {}: {error}",
                run.id
            );
        } else {
            record_design_context_metric(
                store,
                run,
                "design_context_source_sections_read",
                hashes.len() as u64,
                json!({
                    "accessMode": "raw",
                    "bytesRead": run.design_source_size_bytes.unwrap_or(0),
                }),
            )
            .await;
        }
    }
}

pub(crate) async fn record_design_context_metric(
    store: &RuntimeStore,
    run: &AgentRun,
    name: &str,
    value: u64,
    mut metadata: Value,
) {
    if run.design_context_manifest.is_none() {
        return;
    }
    let mode = match run.design_context_effective_compatibility_mode.as_deref() {
        Some("enforced") => "enforced",
        _ => "observe",
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("mode".to_string(), json!(mode));
        object.insert("surface".to_string(), json!("website"));
        object.insert(
            "phase".to_string(),
            serde_json::to_value(run.phase).unwrap_or_else(|_| json!("unknown")),
        );
    }
    let _ = store
        .append_event(AgentEvent::MetricRecorded {
            run_id: run.id.clone(),
            name: name.to_string(),
            value,
            metadata: Some(metadata),
            timestamp: Utc::now(),
        })
        .await;
}

fn frozen_style_contract_read_is_verified(run: &AgentRun, result: &ToolResult) -> bool {
    let Some(expected) = run
        .design_context_artifacts
        .get("inputs/template-style-contract.json")
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return false;
    };
    let Some(actual) = result
        .content
        .get("text")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return false;
    };
    crate::style_contract::style_contract_identity(&expected)
        == crate::style_contract::style_contract_identity(&actual)
}

fn normalize_design_context_path(path: &str) -> String {
    path.trim_start_matches("/workspace/")
        .trim_start_matches("./")
        .to_string()
}

#[allow(clippy::let_and_return)]
fn normalize_workspace_root(workspace_root: PathBuf) -> PathBuf {
    // remote-fs-boundary: allow-begin local-path-normalization
    let mut normalized = PathBuf::new();
    for component in workspace_root.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    let normalized = fs::canonicalize(&normalized).unwrap_or(normalized);
    // remote-fs-boundary: allow-end local-path-normalization
    normalized
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
    runtime_storage_dir: &Path,
    run_id: &str,
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

    let safe_run_id = run_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    // remote-fs-boundary: allow-begin runtime-storage-tool-result
    let artifact_dir = runtime_storage_dir.join("tool-results").join(&safe_run_id);
    let artifact_name = format!("{tool_use_id}.json");
    let artifact_path = artifact_dir.join(&artifact_name);
    let artifact_uri = format!("runtime://tool-results/{safe_run_id}/{artifact_name}");
    if let Err(error) = fs::create_dir_all(&artifact_dir)
        .and_then(|_| fs::write(&artifact_path, serialized.as_bytes()))
    {
        return ToolResult::error(format!("failed to persist oversized tool result: {error}"));
    }

    // remote-fs-boundary: allow-end runtime-storage-tool-result
    let preview = serialized.chars().take(2000).collect::<String>();
    ToolResult {
        content: json!({
            "truncated": true,
            "uri": artifact_uri,
            "preview": preview,
            "originalChars": serialized.chars().count(),
            "limitChars": limit,
        }),
        is_error: false,
        metadata: Some(json!({
            "truncated": true,
            "fullResultUri": artifact_uri,
        })),
    }
}

#[cfg(test)]
mod design_context_gate_tests {
    use super::*;
    use crate::{
        content_plan_approval::{RecordContentPlanApproval, RecordContentPlanChange},
        design_context::{
            DesignContextArtifactManifest, DesignContextManifest, DesignContextPackagePayload,
            DesignContextReadRequirement, ProfileCompatibilityMode, ProfileEnforcementMode,
            VerificationPolicySnapshot,
        },
        generation_context::compile_generation_context,
        types::{AgentPhase, Brief, ContentSource},
    };

    fn manifest() -> DesignContextManifest {
        let profile = json!({
            "id": "profile-1",
            "version": 1,
            "scope": { "projectId": "project-1" },
        });
        let profile_text = String::from_utf8(crate::types::canonical_json_bytes(&profile)).unwrap();
        let artifact_manifest = DesignContextArtifactManifest {
            schema_version: "design-context-artifacts@1".to_string(),
            artifacts: vec![crate::design_context::DesignContextArtifact {
                path: "inputs/design-profile.json".to_string(),
                kind: "profile".to_string(),
                bytes: profile_text.len() as u64,
                sha256: crate::types::sha256_hex(profile_text.as_bytes()),
                required_before_mutation: true,
            }],
        };
        let artifact_manifest_hash =
            crate::types::canonical_json_hash(&serde_json::to_value(&artifact_manifest).unwrap());
        let payload = DesignContextPackagePayload {
            schema_version: "design-context@1".to_string(),
            design_profile_id: "profile-1".to_string(),
            design_profile_version: 1,
            base_profile_hash: "base-hash".to_string(),
            effective_profile_hash: crate::types::canonical_json_hash(&profile),
            brief_hash: "brief-hash".to_string(),
            brief_schema_version: "brief@1".to_string(),
            surface: "website".to_string(),
            template: "next-app".to_string(),
            template_manifest_sha256: "template-hash".to_string(),
            expected_app_root: "project".to_string(),
            compiler_version: "design-context-compiler@1".to_string(),
            declared_enforcement_mode: ProfileEnforcementMode::Observe,
            effective_compatibility_mode: ProfileCompatibilityMode::Observe,
            verification_policy: VerificationPolicySnapshot {
                policy_id: "website-verification@1".to_string(),
                a11y_ruleset_version: "a11y@1".to_string(),
                viewport_matrix_id: "viewport@1".to_string(),
                required_verifier_kinds: Vec::new(),
            },
            artifact_manifest_hash,
            resolved_runtime_tokens: BTreeMap::new(),
            resolved_token_snapshot_hash: "tokens-hash".to_string(),
            required_reads: vec![
                read("inputs/brief.md"),
                read("inputs/design-profile.json"),
                read("inputs/design-profile-usage.md"),
                read("inputs/component-recipes.json"),
                read("inputs/template-style-contract.json"),
            ],
            craft_packs: Vec::new(),
            layout_guidance: Vec::new(),
            warnings: Vec::new(),
        };
        DesignContextManifest {
            schema_version: "design-context-manifest@1".to_string(),
            content_hash: crate::types::canonical_json_hash(
                &serde_json::to_value(&payload).unwrap(),
            ),
            artifact_manifest,
            payload,
        }
    }

    fn read(path: &str) -> DesignContextReadRequirement {
        DesignContextReadRequirement {
            path: path.to_string(),
            reason: "test".to_string(),
            phases: vec![AgentPhase::Build],
        }
    }

    async fn frozen_build_run() -> AgentRun {
        let store = RuntimeStore::new();
        let mut run = store
            .create_run(
                "project-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "test".to_string(),
                Vec::new(),
            )
            .await;
        let manifest = manifest();
        run.design_profile_id = Some(manifest.payload.design_profile_id.clone());
        run.design_profile_version = Some(manifest.payload.design_profile_version);
        run.design_profile_hash = Some(manifest.payload.base_profile_hash.clone());
        run.design_profile_effective_hash = Some(manifest.payload.effective_profile_hash.clone());
        run.design_profile_surface = Some(manifest.payload.surface.clone());
        run.design_profile_template = Some(manifest.payload.template.clone());
        run.design_context_package_version = Some(manifest.payload.schema_version.clone());
        run.design_context_content_hash = Some(manifest.content_hash.clone());
        run.design_context_artifact_manifest_hash =
            Some(manifest.payload.artifact_manifest_hash.clone());
        run.design_context_materialization_hash =
            Some(manifest.payload.artifact_manifest_hash.clone());
        run.design_context_compiler_version = Some(manifest.payload.compiler_version.clone());
        run.design_context_brief_hash = Some(manifest.payload.brief_hash.clone());
        run.design_context_verification_policy_id =
            Some(manifest.payload.verification_policy.policy_id.clone());
        run.design_context_expected_app_root = Some(manifest.payload.expected_app_root.clone());
        run.design_context_declared_enforcement_mode = Some("observe".to_string());
        run.design_context_effective_compatibility_mode = Some("observe".to_string());
        run.design_context_warnings = manifest.payload.warnings.clone();
        let profile = json!({
            "id": "profile-1",
            "version": 1,
            "scope": { "projectId": "project-1" },
        });
        run.design_context_artifacts.insert(
            "inputs/design-profile.json".to_string(),
            String::from_utf8(crate::types::canonical_json_bytes(&profile)).unwrap(),
        );
        run.design_context_manifest = Some(serde_json::to_value(manifest).unwrap());
        run
    }

    #[tokio::test]
    async fn frozen_dcp_requires_bootstrap_reads_and_post_init_style_contract() {
        let mut run = frozen_build_run().await;
        let blocked =
            design_context_read_gate(&run, "project.init", &json!({ "template": "next-app" }))
                .expect("bootstrap reads must be required");
        assert_eq!(blocked.1, "design_context.read_required");
        assert!(blocked.2["missingFiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "inputs/template-style-contract.json"));

        run.design_context_read_files = manifest()
            .payload
            .required_reads
            .into_iter()
            .map(|requirement| requirement.path)
            .collect();
        assert!(
            design_context_read_gate(&run, "project.init", &json!({ "template": "next-app" }),)
                .is_none()
        );

        let blocked =
            design_context_read_gate(&run, "fs.patch", &json!({ "path": "project/app/page.tsx" }))
                .expect("post-init style contract must be verified first");
        assert_eq!(blocked.1, "design_context.style_contract_unverified");

        run.design_context_style_contract_verified = Some(true);
        let blocked =
            design_context_read_gate(&run, "fs.patch", &json!({ "path": "project/app/page.tsx" }))
                .expect("post-init style contract read must be required");
        assert_eq!(blocked.1, "design_context.read_required");
        assert_eq!(
            blocked.2["missingFiles"],
            json!(["state/style-contract.json"])
        );
    }

    #[tokio::test]
    async fn frozen_dcp_artifact_tamper_blocks_mutation_before_read_gate() {
        let mut run = frozen_build_run().await;
        run.design_context_artifacts.insert(
            "inputs/design-profile.json".to_string(),
            "{\"id\":\"tampered\",\"version\":1}".to_string(),
        );

        let blocked =
            design_context_read_gate(&run, "fs.patch", &json!({ "path": "project/app/page.tsx" }))
                .expect("tampered frozen DCP must fail closed before mutation");
        assert_eq!(blocked.1, "design_context.integrity_failed");
        assert_eq!(blocked.2["runId"], run.id);
    }

    #[tokio::test]
    async fn frozen_dcp_rejects_project_init_root_or_template_drift() {
        let mut run = frozen_build_run().await;
        run.design_context_read_files = manifest()
            .payload
            .required_reads
            .into_iter()
            .map(|requirement| requirement.path)
            .collect();
        assert_eq!(
            design_context_read_gate(
                &run,
                "project.init",
                &json!({ "template": "next-app", "path": "site" }),
            )
            .unwrap()
            .1,
            "project.app_root_mismatch"
        );
        assert_eq!(
            design_context_read_gate(
                &run,
                "project.init",
                &json!({ "template": "fumadocs-docs" }),
            )
            .unwrap()
            .1,
            "project.template_mismatch"
        );
    }

    #[tokio::test]
    async fn edit_requires_a_verified_style_contract_before_mutation() {
        let mut run = frozen_build_run().await;
        run.phase = AgentPhase::Edit;
        run.design_context_read_files = manifest()
            .payload
            .required_reads
            .into_iter()
            .map(|requirement| requirement.path)
            .chain(std::iter::once("state/style-contract.json".to_string()))
            .collect();

        let blocked =
            design_context_read_gate(&run, "fs.patch", &json!({ "path": "project/app/page.tsx" }))
                .expect("Edit mutations must fail closed before contract verification");
        assert_eq!(blocked.1, "design_context.style_contract_unverified");

        run.design_context_style_contract_verified = Some(true);
        assert!(design_context_read_gate(
            &run,
            "fs.patch",
            &json!({ "path": "project/app/page.tsx" }),
        )
        .is_none());
    }

    #[tokio::test]
    async fn successful_style_contract_read_matches_frozen_artifact() {
        let mut run = frozen_build_run().await;
        let expected = json!({
            "version": 1,
            "template": "next-app",
            "appRoot": "project",
            "tokens": { "color.primary": "--color-primary" },
        });
        run.design_context_artifacts.insert(
            "inputs/template-style-contract.json".to_string(),
            serde_json::to_string(&expected).unwrap(),
        );
        let result = ToolResult::ok(json!({
            "path": "/workspace/state/style-contract.json",
            "text": serde_json::to_string(&expected).unwrap(),
        }));
        assert!(frozen_style_contract_read_is_verified(&run, &result));
    }

    #[tokio::test]
    async fn style_contract_read_rejects_identity_drift() {
        let mut run = frozen_build_run().await;
        run.design_context_artifacts.insert(
            "inputs/template-style-contract.json".to_string(),
            json!({
                "version": 1,
                "template": "next-app",
                "appRoot": "project",
                "tokens": { "color.primary": "--color-primary" },
            })
            .to_string(),
        );
        let result = ToolResult::ok(json!({
            "path": "/workspace/state/style-contract.json",
            "text": json!({
                "version": 1,
                "template": "next-app",
                "appRoot": "project",
                "tokens": { "color.primary": "--wrong" },
            }).to_string(),
        }));
        assert!(!frozen_style_contract_read_is_verified(&run, &result));
    }

    #[tokio::test]
    async fn enabled_generation_context_uses_attestation_without_model_dcp_reads() {
        let store = RuntimeStore::new();
        let project_id = "project-runtime-attestation";
        store
            .upsert_project_access(
                project_id,
                "principal-runtime-attestation".to_string(),
                "ws-runtime-attestation".to_string(),
            )
            .await
            .unwrap();
        let sources = vec![ContentSource::readable(
            "source-runtime-attestation",
            "user",
            "Authoritative homepage copy",
        )];
        let brief_run = store
            .create_run(
                project_id.to_string(),
                AgentPhase::Brief,
                "brief".to_string(),
                "internal-balanced".to_string(),
                sources.clone(),
            )
            .await;
        let brief_id = store
            .write_brief_draft(
                &brief_run.id,
                Brief {
                    project_type: "website".to_string(),
                    audience: "operators".to_string(),
                    content_hierarchy: vec!["hero".to_string()],
                    page_structure: json!([]),
                    visual_direction: "calm".to_string(),
                    recommended_template: "next-app".to_string(),
                    assumptions: Vec::new(),
                    missing_information: Vec::new(),
                },
            )
            .await
            .unwrap();
        store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
        let run = store
            .create_run_with_context(
                project_id.to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "internal-balanced".to_string(),
                sources,
                Some(brief_id),
                None,
            )
            .await;
        let approval = store
            .content_plan_approval_store()
            .approve(RecordContentPlanApproval {
                project_id: project_id.to_string(),
                plan_id: "content-plan-runtime-attestation".to_string(),
                revision: 1,
                content_hash: "a".repeat(64),
                confirmation_event_id: "confirmation-runtime-attestation".to_string(),
            })
            .unwrap();
        let storage = std::env::temp_dir().join(format!(
            "zerondesign-runtime-attestation-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let context = compile_generation_context(&store, &storage, &run, &approval)
            .await
            .unwrap();
        store.bind_run_generation_context(&context).await.unwrap();
        let run = store
            .set_run_generation_context_runtime_mode(&run.id, "enabled")
            .await
            .unwrap();
        assert!(run.design_context_read_files.is_empty());
        assert!(runtime_attestation_gate(
            &store,
            &run,
            "project.init",
            &json!({ "template": "next-app" }),
        )
        .await
        .is_none());

        let identity = &context.payload.identity;
        let project = store
            .upsert_project_runtime_state_with_template_identity(
                project_id,
                identity.app_root.clone(),
                identity.template_id.clone(),
                identity.template_version.clone(),
                Some(identity.template_manifest_hash.clone()),
                "next".to_string(),
                Some("next-app".to_string()),
                Some("0.1.0".to_string()),
                "npm".to_string(),
                "package-lock.json".to_string(),
                "https://registry.internal.example/npm/".to_string(),
            )
            .await
            .unwrap();
        let run = store
            .set_run_project_state_snapshot(&run.id, project)
            .await
            .unwrap();
        assert!(runtime_attestation_gate(
            &store,
            &run,
            "fs.patch",
            &json!({ "path": "project/app/page.tsx" }),
        )
        .await
        .is_none());

        store
            .content_plan_approval_store()
            .record_plan_change(RecordContentPlanChange {
                project_id: project_id.to_string(),
                plan_id: approval.plan_id.clone(),
                revision: 2,
                content_hash: "b".repeat(64),
                change_event_id: "change-runtime-attestation".to_string(),
            })
            .unwrap();
        let blocked = runtime_attestation_gate(
            &store,
            &run,
            "fs.patch",
            &json!({ "path": "project/app/page.tsx" }),
        )
        .await
        .expect("stale approval must fail before mutation");
        assert_eq!(blocked.1, "content_plan.approval_rejected");
    }

    #[tokio::test]
    async fn replan_marker_is_persisted_as_a_terminal_run_fact() {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "project-replan".to_string(),
                AgentPhase::Repair,
                "repair".to_string(),
                "test".to_string(),
                Vec::new(),
            )
            .await;
        store.ensure_initial_checkpoint(&run.id).await.unwrap();

        mark_run_replan_required(
            &store,
            &run,
            "fs.patch",
            Some("project/app/outside.tsx"),
            "edit.plan_scope_violation",
        )
        .await;

        assert!(run_replan_required(&store, &run.id).await);
        assert_eq!(
            store.get_run(&run.id).await.unwrap().status,
            AgentRunStatus::Partial
        );
        assert!(store.events(&run.id).await.iter().any(|event| matches!(
            event,
            AgentEvent::RunWorkflowProgress { stage, .. } if stage == "replan_required"
        )));
    }
}
