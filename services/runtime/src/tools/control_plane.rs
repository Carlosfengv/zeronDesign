use crate::{
    config::{RuntimeConfig, SandboxBackendMode},
    conversation::RuntimeStore,
    permission::{PermissionReason, PermissionResult, RuleSource},
    repair_loop::{
        normalize_error_key, record_repair_attempt, RepairActionSignature, RepairLoopDecision,
        RepairLoopStopReason,
    },
    sandbox_adapter::{
        sandbox_channel_from_binding, sandbox_claim_name, warm_pool_name, workspace_pvc_name,
        KubectlSandboxClient, SandboxAdapter, SandboxAdapterConfig, SandboxKubeClient,
    },
    tools::{
        brief, content, mcp, run,
        runtime::{
            ProgressSink, Tool, ToolContext, ToolError, ToolExecutor, ToolResult, ValidationError,
        },
        sandbox::{self, LocalWorkspaceBackend, WorkspaceBackend},
        schema::{object_schema, string_schema},
        user,
    },
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, Brief, ProjectVersionStatus, ReviewFindingCategory,
        ReviewFindingEvidence, ReviewFindingSeverity, SandboxBinding, SandboxBindingStatus,
        SandboxChannelProtocol,
    },
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub fn control_plane_executor() -> ToolExecutor {
    control_plane_executor_with_sandbox_backend(Arc::new(PhaseAContractSandboxBackend::default()))
}

pub fn control_plane_executor_for_config(config: &RuntimeConfig) -> ToolExecutor {
    let executor = match config.sandbox_backend_mode {
        SandboxBackendMode::Kubernetes => control_plane_executor_with_backends(
            sandbox_backend_for_config(config),
            Arc::new(sandbox::SandboxChannelWorkspaceBackend::new()),
            Arc::new(sandbox::SandboxChannelCommandBackend::new()),
        ),
        SandboxBackendMode::PhaseAContract => {
            control_plane_executor_with_sandbox_backend(sandbox_backend_for_config(config))
        }
    };
    executor
        .with_policy_profile_and_registry(config.policy_profile, config.npm_registry.clone())
        .with_workspace_root(&config.workspace_root)
}

pub fn control_plane_executor_with_sandbox_backend(
    sandbox_backend: Arc<dyn SandboxBackend>,
) -> ToolExecutor {
    control_plane_executor_with_backends(
        sandbox_backend,
        Arc::new(LocalWorkspaceBackend),
        Arc::new(sandbox::LocalCommandBackend),
    )
}

pub fn control_plane_executor_with_backends(
    sandbox_backend: Arc<dyn SandboxBackend>,
    workspace_backend: Arc<dyn WorkspaceBackend>,
    command_backend: Arc<dyn sandbox::SandboxCommandBackend>,
) -> ToolExecutor {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ContentListSourcesTool),
        Arc::new(ContentReadSourceTool),
        Arc::new(BriefWriteDraftTool),
        Arc::new(BriefRequestConfirmationTool),
        Arc::new(RunReportProgressTool),
        Arc::new(RunCompleteTool),
        Arc::new(ReviewReportFindingTool),
        Arc::new(RepairReportAttemptTool),
        Arc::new(UserAskTool),
        Arc::new(SandboxClaimTool {
            backend: sandbox_backend.clone(),
        }),
        Arc::new(SandboxGetStatusTool {
            backend: sandbox_backend.clone(),
        }),
        Arc::new(SandboxWaitReadyTool {
            backend: sandbox_backend.clone(),
        }),
        Arc::new(SandboxOpenChannelTool),
        Arc::new(SandboxReleaseTool {
            backend: sandbox_backend,
        }),
    ];
    tools.extend(sandbox::sandbox_tools_with_backends(
        workspace_backend,
        command_backend,
    ));
    tools.extend(mcp::mcp_stub_tools());
    ToolExecutor::new(tools, Default::default())
}

pub fn sandbox_backend_for_config(config: &RuntimeConfig) -> Arc<dyn SandboxBackend> {
    match config.sandbox_backend_mode {
        SandboxBackendMode::Kubernetes => Arc::new(KubernetesSandboxBackend::new(
            KubectlSandboxClient::new(),
            SandboxAdapterConfig {
                namespace: config.k8s_namespace.clone(),
                ..Default::default()
            },
        )),
        SandboxBackendMode::PhaseAContract => Arc::new(PhaseAContractSandboxBackend::default()),
    }
}

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    fn mode(&self) -> &'static str;
    async fn claim(
        &self,
        store: &RuntimeStore,
        project_id: &str,
        template_key: &str,
    ) -> Result<SandboxBinding>;
    async fn wait_ready(
        &self,
        store: &RuntimeStore,
        binding_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<SandboxBinding>;
    async fn release(&self, store: &RuntimeStore, binding_id: &str) -> Result<SandboxBinding>;
}

#[derive(Debug, Clone)]
pub struct PhaseAContractSandboxBackend {
    namespace: String,
    channel_protocol: SandboxChannelProtocol,
}

impl Default for PhaseAContractSandboxBackend {
    fn default() -> Self {
        Self {
            namespace: "anydesign-sandboxes".to_string(),
            channel_protocol: SandboxChannelProtocol::Websocket,
        }
    }
}

#[async_trait]
impl SandboxBackend for PhaseAContractSandboxBackend {
    fn mode(&self) -> &'static str {
        "phase_a_contract"
    }

    async fn claim(
        &self,
        store: &RuntimeStore,
        project_id: &str,
        template_key: &str,
    ) -> Result<SandboxBinding> {
        let short_id = store.next_id("sandbox");
        let claim_name = sandbox_claim_name(project_id, &short_id);
        store
            .create_sandbox_binding(
                project_id,
                claim_name.clone(),
                claim_name.clone(),
                workspace_pvc_name(&claim_name),
                warm_pool_name(template_key),
                self.namespace.clone(),
                self.channel_protocol,
            )
            .await
    }

    async fn wait_ready(
        &self,
        store: &RuntimeStore,
        binding_id: &str,
        _timeout_ms: Option<u64>,
    ) -> Result<SandboxBinding> {
        let binding = store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("sandbox binding not found: {binding_id}"))?;
        if matches!(
            binding.status,
            SandboxBindingStatus::Failed | SandboxBindingStatus::Deleted
        ) {
            return Err(anyhow::anyhow!(
                "sandbox claim entered terminal status: {:?}",
                binding.status
            ));
        }

        if binding.status == SandboxBindingStatus::Ready {
            return Ok(binding);
        }

        store
            .update_sandbox_binding_status(binding_id, SandboxBindingStatus::Ready)
            .await
    }

    async fn release(&self, store: &RuntimeStore, binding_id: &str) -> Result<SandboxBinding> {
        let binding = store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("sandbox binding not found: {binding_id}"))?;
        if binding.status == SandboxBindingStatus::Deleted {
            return Ok(binding);
        }

        store
            .update_sandbox_binding_status(binding_id, SandboxBindingStatus::Deleted)
            .await
    }
}

#[derive(Clone)]
pub struct KubernetesSandboxBackend {
    client: Arc<dyn SandboxKubeClient>,
    config: SandboxAdapterConfig,
}

impl KubernetesSandboxBackend {
    pub fn new<C>(client: C, config: SandboxAdapterConfig) -> Self
    where
        C: SandboxKubeClient + 'static,
    {
        Self {
            client: Arc::new(client),
            config,
        }
    }

    fn adapter(
        &self,
        store: RuntimeStore,
        timeout_ms: Option<u64>,
    ) -> SandboxAdapter<Arc<dyn SandboxKubeClient>> {
        let mut config = self.config.clone();
        if let Some(timeout_ms) = timeout_ms {
            config.wait_timeout = Duration::from_millis(timeout_ms);
        }
        SandboxAdapter::new(store, self.client.clone(), config)
    }
}

#[async_trait]
impl SandboxBackend for KubernetesSandboxBackend {
    fn mode(&self) -> &'static str {
        "kubernetes"
    }

    async fn claim(
        &self,
        store: &RuntimeStore,
        project_id: &str,
        template_key: &str,
    ) -> Result<SandboxBinding> {
        self.adapter(store.clone(), None)
            .claim(template_key, project_id)
            .await
    }

    async fn wait_ready(
        &self,
        store: &RuntimeStore,
        binding_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<SandboxBinding> {
        self.adapter(store.clone(), timeout_ms)
            .wait_ready(binding_id)
            .await
    }

    async fn release(&self, store: &RuntimeStore, binding_id: &str) -> Result<SandboxBinding> {
        self.adapter(store.clone(), None).release(binding_id).await
    }
}

fn allow_control_plane(input: &Value) -> PermissionResult {
    PermissionResult::Allow {
        updated_input: input.clone(),
        reason: PermissionReason::Other {
            reason: "control-plane tool explicitly allowed".to_string(),
        },
    }
}

fn require_string(input: &Value, key: &str, tool: &str) -> Result<(), ValidationError> {
    if input.get(key).and_then(Value::as_str).is_some() {
        return Ok(());
    }
    Err(ValidationError::new(format!("{tool} requires {key}")))
}

fn sandbox_binding_response(binding: &SandboxBinding, mode: &str) -> Value {
    json!({
        "bindingId": binding.id,
        "projectId": binding.project_id,
        "sandboxName": binding.sandbox_name,
        "sandboxClaimName": binding.sandbox_claim_name,
        "workspacePvcName": binding.workspace_pvc_name,
        "channelServiceName": binding.channel_service_name,
        "warmPoolName": binding.warm_pool_name,
        "namespace": binding.namespace,
        "status": binding.status,
        "channelProtocol": binding.channel_protocol,
        "mode": mode,
    })
}

struct ContentListSourcesTool;

#[async_trait]
impl Tool for ContentListSourcesTool {
    fn name(&self) -> &'static str {
        "content.list_sources"
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
        allow_control_plane(input)
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        content::list_sources(&ctx.store, &ctx.run.id)
            .await
            .map(ToolResult::ok)
            .map_err(|error| ToolError::Recoverable(error.to_string()))
    }
}

struct ContentReadSourceTool;

#[async_trait]
impl Tool for ContentReadSourceTool {
    fn name(&self) -> &'static str {
        "content.read_source"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({ "id": string_schema("Content source id") }), &["id"])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "id", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        content::read_source(&ctx.store, &ctx.run.id, &input)
            .await
            .map(ToolResult::ok)
            .map_err(|error| {
                let id = input.get("id").and_then(Value::as_str).unwrap_or("");
                ToolError::RecoverableWithMetadata {
                    message: error.to_string(),
                    error_kind: "content.source_missing".to_string(),
                    metadata: json!({
                        "sourceId": id,
                        "suggestedAction": "Call content.list_sources and read one of the returned source ids, or use inputs/*.md files that were bootstrapped into the workspace."
                    }),
                }
            })
    }
}

struct BriefWriteDraftTool;

#[async_trait]
impl Tool for BriefWriteDraftTool {
    fn name(&self) -> &'static str {
        "brief.write_draft"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["brief.update"]
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "projectType": string_schema("website or docs"),
                "audience": string_schema("Target audience"),
                "contentHierarchy": { "type": "array", "items": { "type": "string" } },
                "pageStructure": { "type": "array" },
                "visualDirection": string_schema("Visual direction"),
                "recommendedTemplate": {
                    "type": "string",
                    "enum": ["astro-website", "fumadocs-docs"],
                    "description": "Template key. Use astro-website for website projects and fumadocs-docs for docs projects."
                },
                "assumptions": { "type": "array", "items": { "type": "string" } },
                "missingInformation": { "type": "array", "items": { "type": "string" } }
            }),
            &[
                "projectType",
                "audience",
                "contentHierarchy",
                "pageStructure",
                "visualDirection",
                "recommendedTemplate",
                "assumptions",
                "missingInformation",
            ],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({ "briefId": string_schema("Created brief id") }),
            &["briefId"],
        ))
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let input = brief::normalize_draft_input(input);
        let brief: Brief = serde_json::from_value(input.clone()).map_err(|error| {
            ValidationError::new(format!(
                "brief.write_draft received invalid brief JSON: {error}"
            ))
        })?;
        brief.validate_for_runtime().map_err(|error| {
            ValidationError::new(format!("brief.write_draft validation failed: {error}"))
        })?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        brief::write_draft(&ctx.store, &ctx.run.id, input)
            .await
            .map(ToolResult::ok)
            .map_err(|error| ToolError::Recoverable(error.to_string()))
    }
}

struct BriefRequestConfirmationTool;

#[async_trait]
impl Tool for BriefRequestConfirmationTool {
    fn name(&self) -> &'static str {
        "brief.request_confirmation"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "message": string_schema("Confirmation prompt") }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let message = input.get("message").and_then(Value::as_str);
        let value = brief::request_confirmation(&ctx.store, &ctx.run.id, &ctx.project_id, message)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let _ = ctx
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: ctx.run.id.clone(),
                state: "needs_user_input".to_string(),
                timestamp: Utc::now(),
            })
            .await;
        ctx.store
            .update_run_status(&ctx.run.id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        Ok(ToolResult::ok(value))
    }
}

struct RunReportProgressTool;

#[async_trait]
impl Tool for RunReportProgressTool {
    fn name(&self) -> &'static str {
        "run.report_progress"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({ "summary": string_schema("Progress summary") }), &[])
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        run::report_progress(&ctx.store, &ctx.run.id, &ctx.project_id, &input)
            .await
            .map(ToolResult::ok)
            .map_err(|error| ToolError::Recoverable(error.to_string()))
    }
}

struct SandboxClaimTool {
    backend: Arc<dyn SandboxBackend>,
}

#[async_trait]
impl Tool for SandboxClaimTool {
    fn name(&self) -> &'static str {
        "sandbox.claim"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "templateKey": string_schema("Sandbox template key") }),
            &["templateKey"],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "projectId": string_schema("Project id"),
                "sandboxName": string_schema("Sandbox resource name"),
                "sandboxClaimName": string_schema("SandboxClaim resource name"),
                "workspacePvcName": string_schema("PersistentVolumeClaim backing /workspace"),
                "warmPoolName": string_schema("SandboxWarmPool resource name"),
                "namespace": string_schema("Kubernetes namespace"),
                "status": string_schema("Sandbox binding status"),
                "channelProtocol": string_schema("Channel protocol"),
                "mode": string_schema("Runtime adapter mode")
            }),
            &[
                "bindingId",
                "projectId",
                "sandboxName",
                "sandboxClaimName",
                "workspacePvcName",
                "warmPoolName",
                "namespace",
                "status",
                "channelProtocol",
                "mode",
            ],
        ))
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "templateKey", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let template_key = input
            .get("templateKey")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable("sandbox.claim requires templateKey".to_string())
            })?;
        let binding = self
            .backend
            .claim(&ctx.store, &ctx.project_id, template_key)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        ctx.store
            .bind_run_to_sandbox(&ctx.run.id, &binding.id)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let _ = ctx
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: ctx.run.id.clone(),
                state: "sandbox.claiming".to_string(),
                timestamp: Utc::now(),
            })
            .await;

        Ok(ToolResult::ok(sandbox_binding_response(
            &binding,
            self.backend.mode(),
        )))
    }
}

struct SandboxWaitReadyTool {
    backend: Arc<dyn SandboxBackend>,
}

struct SandboxGetStatusTool {
    backend: Arc<dyn SandboxBackend>,
}

#[async_trait]
impl Tool for SandboxGetStatusTool {
    fn name(&self) -> &'static str {
        "sandbox.get_status"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "bindingId": string_schema("SandboxBinding id") }),
            &["bindingId"],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "projectId": string_schema("Project id"),
                "sandboxName": string_schema("Sandbox resource name"),
                "sandboxClaimName": string_schema("SandboxClaim resource name"),
                "workspacePvcName": string_schema("PersistentVolumeClaim backing /workspace"),
                "warmPoolName": string_schema("SandboxWarmPool resource name"),
                "namespace": string_schema("Kubernetes namespace"),
                "status": string_schema("Sandbox binding status"),
                "channelProtocol": string_schema("Channel protocol"),
                "mode": string_schema("Runtime adapter mode")
            }),
            &[
                "bindingId",
                "projectId",
                "sandboxName",
                "sandboxClaimName",
                "workspacePvcName",
                "warmPoolName",
                "namespace",
                "status",
                "channelProtocol",
                "mode",
            ],
        ))
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "bindingId", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let binding_id = input
            .get("bindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable("sandbox.get_status requires bindingId".to_string())
            })?;
        let binding = ctx
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| {
                ToolError::Recoverable(format!("sandbox binding not found: {binding_id}"))
            })?;

        Ok(ToolResult::ok(sandbox_binding_response(
            &binding,
            self.backend.mode(),
        )))
    }
}

#[async_trait]
impl Tool for SandboxWaitReadyTool {
    fn name(&self) -> &'static str {
        "sandbox.wait_ready"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &["bindingId"],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "projectId": string_schema("Project id"),
                "sandboxName": string_schema("Sandbox resource name"),
                "sandboxClaimName": string_schema("SandboxClaim resource name"),
                "workspacePvcName": string_schema("PersistentVolumeClaim backing /workspace"),
                "warmPoolName": string_schema("SandboxWarmPool resource name"),
                "namespace": string_schema("Kubernetes namespace"),
                "status": string_schema("Sandbox binding status"),
                "channelProtocol": string_schema("Channel protocol"),
                "mode": string_schema("Runtime adapter mode")
            }),
            &[
                "bindingId",
                "projectId",
                "sandboxName",
                "sandboxClaimName",
                "workspacePvcName",
                "warmPoolName",
                "namespace",
                "status",
                "channelProtocol",
                "mode",
            ],
        ))
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "bindingId", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let binding_id = input
            .get("bindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable("sandbox.wait_ready requires bindingId".to_string())
            })?;
        let timeout_ms = input.get("timeoutMs").and_then(Value::as_u64);
        let ready = self
            .backend
            .wait_ready(&ctx.store, binding_id, timeout_ms)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let current_run = ctx
            .store
            .get_run(&ctx.run.id)
            .await
            .ok_or_else(|| ToolError::Recoverable(format!("run not found: {}", ctx.run.id)))?;
        if current_run.sandbox_id.as_deref() != Some(binding_id) {
            return Err(ToolError::Recoverable(format!(
                "run {} is not bound to sandbox binding {binding_id}",
                ctx.run.id
            )));
        }
        let busy = ctx
            .store
            .mark_sandbox_binding_busy(&ready.id)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let _ = ctx
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: ctx.run.id.clone(),
                state: "sandbox.ready".to_string(),
                timestamp: Utc::now(),
            })
            .await;

        Ok(ToolResult::ok(sandbox_binding_response(
            &busy,
            self.backend.mode(),
        )))
    }
}

struct SandboxOpenChannelTool;

#[async_trait]
impl Tool for SandboxOpenChannelTool {
    fn name(&self) -> &'static str {
        "sandbox.open_channel"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "bindingId": string_schema("SandboxBinding id") }),
            &["bindingId"],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "projectId": string_schema("Project id"),
                "sandboxName": string_schema("Sandbox resource name"),
                "workspacePvcName": string_schema("PersistentVolumeClaim backing /workspace"),
                "namespace": string_schema("Kubernetes namespace"),
                "protocol": string_schema("Channel protocol"),
                "endpoint": string_schema("Workspace channel endpoint")
            }),
            &[
                "bindingId",
                "projectId",
                "sandboxName",
                "workspacePvcName",
                "namespace",
                "protocol",
                "endpoint",
            ],
        ))
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "bindingId", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let binding_id = input
            .get("bindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable("sandbox.open_channel requires bindingId".to_string())
            })?;
        let binding = ctx
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| {
                ToolError::Recoverable(format!("sandbox binding not found: {binding_id}"))
            })?;
        let channel = sandbox_channel_from_binding(&binding)
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;

        Ok(ToolResult::ok(json!({
            "bindingId": channel.binding_id,
            "projectId": channel.project_id,
            "sandboxName": channel.sandbox_name,
            "workspacePvcName": channel.workspace_pvc_name,
            "namespace": channel.namespace,
            "protocol": channel.protocol,
            "endpoint": channel.endpoint,
        })))
    }
}

struct SandboxReleaseTool {
    backend: Arc<dyn SandboxBackend>,
}

#[async_trait]
impl Tool for SandboxReleaseTool {
    fn name(&self) -> &'static str {
        "sandbox.release"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["sandbox.delete"]
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "bindingId": string_schema("SandboxBinding id") }),
            &["bindingId"],
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(object_schema(
            json!({
                "bindingId": string_schema("SandboxBinding id"),
                "projectId": string_schema("Project id"),
                "sandboxName": string_schema("Sandbox resource name"),
                "sandboxClaimName": string_schema("SandboxClaim resource name"),
                "workspacePvcName": string_schema("PersistentVolumeClaim backing /workspace"),
                "warmPoolName": string_schema("SandboxWarmPool resource name"),
                "namespace": string_schema("Kubernetes namespace"),
                "status": string_schema("Sandbox binding status"),
                "channelProtocol": string_schema("Channel protocol"),
                "mode": string_schema("Runtime adapter mode")
            }),
            &[
                "bindingId",
                "projectId",
                "sandboxName",
                "sandboxClaimName",
                "workspacePvcName",
                "warmPoolName",
                "namespace",
                "status",
                "channelProtocol",
                "mode",
            ],
        ))
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "bindingId", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let binding_id = input
            .get("bindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable("sandbox.release requires bindingId".to_string())
            })?;
        let deleted = self
            .backend
            .release(&ctx.store, binding_id)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let _ = ctx
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: ctx.run.id.clone(),
                state: "sandbox.released".to_string(),
                timestamp: Utc::now(),
            })
            .await;

        Ok(ToolResult::ok(sandbox_binding_response(
            &deleted,
            self.backend.mode(),
        )))
    }
}

struct RunCompleteTool;

#[async_trait]
impl Tool for RunCompleteTool {
    fn name(&self) -> &'static str {
        "run.complete"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "status": string_schema("completed, partial, blocked, failed, or cancelled"),
                "summary": string_schema("Completion summary")
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        if matches!(ctx.run.phase, AgentPhase::Build | AgentPhase::Edit) {
            let Some(version_id) = ctx.run.output_version_id.as_deref() else {
                return Ok(ToolResult::error(
                    "No output_version_id set. Build/Edit must promote a preview before completing.",
                ));
            };
            let Some(version) = ctx.store.get_project_version(version_id).await else {
                return Ok(ToolResult::error(format!(
                    "Output version not found: {version_id}"
                )));
            };
            if version.status != ProjectVersionStatus::Promoted {
                return Ok(ToolResult::error(
                    "Preview has not been promoted. Emit preview.updated before completing the run.",
                ));
            }
        }
        if ctx.run.phase == AgentPhase::Brief
            && input.get("status").and_then(Value::as_str) == Some("completed")
        {
            let Some(brief_id) = ctx.run.brief_version.as_deref() else {
                return Ok(ToolResult::error(
                    "No brief draft is available. Brief runs must write a draft and request confirmation before completing.",
                ));
            };
            if !ctx.store.is_brief_confirmed(brief_id).await {
                return Ok(ToolResult::error(
                    "Brief is not confirmed. Call brief.request_confirmation and wait for user confirmation before completing.",
                ));
            }
        }
        let value = run::complete(&input)
            .await
            .map_err(|error| ToolError::Terminal(error.to_string()))?;
        Ok(ToolResult::ok(value))
    }
}

struct ReviewReportFindingTool;

#[async_trait]
impl Tool for ReviewReportFindingTool {
    fn name(&self) -> &'static str {
        "review.report_finding"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "versionId": string_schema("Candidate project version id under review"),
                "severity": {
                    "type": "string",
                    "enum": ["info", "warning", "blocking"],
                    "description": "Finding severity"
                },
                "category": {
                    "type": "string",
                    "enum": ["build", "runtime", "visual", "content", "safety"],
                    "description": "Finding category"
                },
                "summary": string_schema("Short actionable finding summary"),
                "repairable": {
                    "type": "boolean",
                    "description": "Whether a Repair run can attempt this finding"
                },
                "evidence": {
                    "type": "object",
                    "properties": {
                        "screenshotId": string_schema("Screenshot artifact id"),
                        "filePath": string_schema("Workspace file path"),
                        "logExcerpt": string_schema("Relevant log excerpt")
                    },
                    "additionalProperties": false
                }
            }),
            &["versionId", "severity", "category", "summary"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "versionId", self.name())?;
        require_string(&input, "severity", self.name())?;
        require_string(&input, "category", self.name())?;
        require_string(&input, "summary", self.name())?;
        parse_review_severity(input["severity"].as_str().unwrap())?;
        parse_review_category(input["category"].as_str().unwrap())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        if ctx.run.phase != AgentPhase::Review {
            return PermissionResult::Deny {
                message: "review.report_finding is only available to Review runs".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "review finding reports require AgentPhase::Review".to_string(),
                },
            };
        }
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let version_id = input["versionId"].as_str().unwrap();
        let severity = parse_review_severity(input["severity"].as_str().unwrap())
            .expect("review.report_finding severity was validated before execution");
        let category = parse_review_category(input["category"].as_str().unwrap())
            .expect("review.report_finding category was validated before execution");
        let summary = input["summary"].as_str().unwrap();
        let repairable = input
            .get("repairable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let evidence = parse_review_evidence(input.get("evidence"));
        let finding = ctx
            .store
            .record_review_finding(
                &ctx.project_id,
                &ctx.run.id,
                version_id,
                severity,
                category,
                summary,
                evidence,
                repairable,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        Ok(ToolResult::ok(json!({
            "findingId": finding.id,
            "versionId": finding.version_id,
            "severity": finding.severity,
            "category": finding.category,
            "status": finding.status,
            "repairable": finding.repairable,
        })))
    }
}

fn parse_review_severity(value: &str) -> Result<ReviewFindingSeverity, ValidationError> {
    match value {
        "info" => Ok(ReviewFindingSeverity::Info),
        "warning" => Ok(ReviewFindingSeverity::Warning),
        "blocking" => Ok(ReviewFindingSeverity::Blocking),
        _ => Err(ValidationError::new(format!(
            "review.report_finding severity must be one of info, warning, blocking; got {value}"
        ))),
    }
}

fn parse_review_category(value: &str) -> Result<ReviewFindingCategory, ValidationError> {
    match value {
        "build" => Ok(ReviewFindingCategory::Build),
        "runtime" => Ok(ReviewFindingCategory::Runtime),
        "visual" => Ok(ReviewFindingCategory::Visual),
        "content" => Ok(ReviewFindingCategory::Content),
        "safety" => Ok(ReviewFindingCategory::Safety),
        _ => Err(ValidationError::new(format!(
            "review.report_finding category must be one of build, runtime, visual, content, safety; got {value}"
        ))),
    }
}

fn parse_review_evidence(value: Option<&Value>) -> Option<ReviewFindingEvidence> {
    value.and_then(|evidence| {
        let screenshot_id = evidence
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let file_path = evidence
            .get("filePath")
            .and_then(Value::as_str)
            .map(str::to_string);
        let log_excerpt = evidence
            .get("logExcerpt")
            .and_then(Value::as_str)
            .map(str::to_string);
        if screenshot_id.is_none() && file_path.is_none() && log_excerpt.is_none() {
            None
        } else {
            Some(ReviewFindingEvidence {
                screenshot_id,
                file_path,
                log_excerpt,
            })
        }
    })
}

struct RepairReportAttemptTool;

#[async_trait]
impl Tool for RepairReportAttemptTool {
    fn name(&self) -> &'static str {
        "repair.report_attempt"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "findingId": string_schema("Review finding id being repaired"),
                "rawError": string_schema("Raw tool/build/browser error observed after the repair attempt"),
                "action": {
                    "type": "object",
                    "properties": {
                        "tool": string_schema("Tool used for the repair action"),
                        "path": string_schema("Workspace path touched or command cwd"),
                        "argv": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Command argv for shell/package attempts, empty for file-only actions"
                        }
                    },
                    "required": ["tool"],
                    "additionalProperties": false
                }
            }),
            &["findingId", "rawError", "action"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "findingId", self.name())?;
        require_string(&input, "rawError", self.name())?;
        let action = input
            .get("action")
            .and_then(Value::as_object)
            .ok_or_else(|| ValidationError::new("repair.report_attempt requires action"))?;
        if action.get("tool").and_then(Value::as_str).is_none() {
            return Err(ValidationError::new(
                "repair.report_attempt requires action.tool",
            ));
        }
        if let Some(argv) = action.get("argv") {
            let Some(values) = argv.as_array() else {
                return Err(ValidationError::new(
                    "repair.report_attempt action.argv must be an array of strings",
                ));
            };
            if values.iter().any(|value| !value.is_string()) {
                return Err(ValidationError::new(
                    "repair.report_attempt action.argv must be an array of strings",
                ));
            }
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        if ctx.run.phase != AgentPhase::Repair {
            return PermissionResult::Deny {
                message: "repair.report_attempt is only available to Repair runs".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "repair attempt reports require AgentPhase::Repair".to_string(),
                },
            };
        }
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let parent_run_id = ctx.run.parent_run_id.as_deref().ok_or_else(|| {
            ToolError::Recoverable("repair.report_attempt requires a parent run".to_string())
        })?;
        let finding_id = input["findingId"].as_str().unwrap();
        if !ctx
            .run
            .finding_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id == finding_id))
        {
            return Err(ToolError::Recoverable(format!(
                "repair.report_attempt finding is not scoped to this repair run: {finding_id}"
            )));
        }
        let raw_error = input["rawError"].as_str().unwrap();
        let action = parse_repair_action(&input["action"]);
        let error_key = normalize_error_key(raw_error);
        let action_key = action.key();
        let decision = record_repair_attempt(
            &ctx.store,
            parent_run_id,
            &ctx.run.id,
            finding_id,
            raw_error,
            action,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;

        Ok(ToolResult::ok(repair_loop_decision_response(
            decision, error_key, action_key,
        )))
    }
}

fn parse_repair_action(input: &Value) -> RepairActionSignature {
    let tool = input["tool"].as_str().unwrap();
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string);
    let argv = input
        .get("argv")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    RepairActionSignature::new(tool, path, argv)
}

fn repair_loop_decision_response(
    decision: RepairLoopDecision,
    error_key: String,
    action_key: String,
) -> Value {
    match decision {
        RepairLoopDecision::Continue {
            error_attempts,
            action_attempts,
        } => json!({
            "decision": "continue",
            "errorKey": error_key,
            "actionKey": action_key,
            "errorAttempts": error_attempts,
            "actionAttempts": action_attempts,
        }),
        RepairLoopDecision::Stop {
            status,
            reason,
            error_attempts,
            action_attempts,
        } => json!({
            "decision": "stop",
            "status": status,
            "reason": repair_loop_stop_reason(&reason),
            "errorKey": error_key,
            "actionKey": action_key,
            "errorAttempts": error_attempts,
            "actionAttempts": action_attempts,
        }),
    }
}

fn repair_loop_stop_reason(reason: &RepairLoopStopReason) -> &'static str {
    match reason {
        RepairLoopStopReason::MaxAttemptsForSameError => "max_attempts_for_same_error",
        RepairLoopStopReason::IdenticalActionDoomLoop => "identical_action_doom_loop",
    }
}

struct UserAskTool;

#[async_trait]
impl Tool for UserAskTool {
    fn name(&self) -> &'static str {
        "user.ask"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "message": string_schema("Question for the user") }),
            &[],
        )
    }

    fn requires_user_interaction(&self) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_control_plane(input)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        user::ask(&ctx.store, &ctx.run.id, &ctx.project_id, &input)
            .await
            .map(ToolResult::ok)
            .map_err(|error| ToolError::Terminal(error.to_string()))
    }
}
