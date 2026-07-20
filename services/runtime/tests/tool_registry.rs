use anydesign_runtime::{
    sandbox_adapter::{
        SandboxAdapterConfig, SandboxClaimManifest, SandboxClaimPhase, SandboxKubeClient,
    },
    tools::{
        control_plane::{
            control_plane_executor, control_plane_executor_with_sandbox_backend,
            KubernetesSandboxBackend,
        },
        registry::{McpToolInfo, ToolDefinition, ToolLoadingPolicy, ToolRegistry},
        schema::{object_schema, string_schema},
    },
    types::{AgentPhase, SandboxBindingStatus, SandboxChannelProtocol},
    RuntimeStore,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Default, Clone)]
struct FakeSandboxClient {
    created: Arc<Mutex<Vec<SandboxClaimManifest>>>,
    phases: Arc<Mutex<VecDeque<SandboxClaimPhase>>>,
    channel_services: Arc<Mutex<VecDeque<Option<String>>>>,
    deleted: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeSandboxClient {
    fn with_phases_and_services(
        phases: Vec<SandboxClaimPhase>,
        channel_services: Vec<Option<String>>,
    ) -> Self {
        Self {
            created: Arc::new(Mutex::new(Vec::new())),
            phases: Arc::new(Mutex::new(VecDeque::from(phases))),
            channel_services: Arc::new(Mutex::new(VecDeque::from(channel_services))),
            deleted: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl SandboxKubeClient for FakeSandboxClient {
    async fn create_claim(&self, manifest: &SandboxClaimManifest) -> Result<()> {
        self.created.lock().unwrap().push(manifest.clone());
        Ok(())
    }

    async fn claim_phase(&self, _namespace: &str, _claim_name: &str) -> Result<SandboxClaimPhase> {
        Ok(self
            .phases
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SandboxClaimPhase::Pending))
    }

    async fn channel_service_name(
        &self,
        _namespace: &str,
        _claim_name: &str,
        _sandbox_name: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .channel_services
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(None))
    }

    async fn delete_claim(&self, namespace: &str, claim_name: &str) -> Result<()> {
        self.deleted
            .lock()
            .unwrap()
            .push((namespace.to_string(), claim_name.to_string()));
        Ok(())
    }
}

#[test]
fn output_schema_is_available() {
    let mut registry = ToolRegistry::new();
    let mut tool = ToolDefinition::eager(
        "brief.write_draft",
        object_schema(
            json!({ "content": string_schema("Brief JSON") }),
            &["content"],
        ),
    );
    tool.output_schema = Some(object_schema(
        json!({ "briefId": string_schema("Created brief id") }),
        &["briefId"],
    ));
    registry.register(tool);

    let registered = registry
        .get("brief.write_draft")
        .expect("tool should exist");
    assert!(registered.output_schema.is_some());
}

#[test]
fn disabled_tools_are_not_sent_to_model() {
    let mut registry = ToolRegistry::new();
    let mut disabled = ToolDefinition::eager("disabled.tool", object_schema(json!({}), &[]));
    disabled.enabled = false;
    registry.register(disabled);

    assert!(registry.model_tool_definitions().is_empty());
}

#[test]
fn deferred_and_mcp_metadata_is_retained_but_not_eager_loaded() {
    let mut registry = ToolRegistry::new();
    let mut mcp_tool = ToolDefinition::deferred_mcp_stub(
        "mcp__example__lookup",
        object_schema(json!({}), &[]),
        McpToolInfo {
            server_name: "example".to_string(),
            tool_name: "lookup".to_string(),
        },
        "example MCP adapter is not configured",
        220,
    );
    mcp_tool.input_json_schema = Some(json!({
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"]
    }));
    registry.register(mcp_tool);

    assert!(registry.model_tool_definitions().is_empty());
    let deferred = registry.deferred_metadata();
    assert_eq!(deferred.len(), 1);
    assert_eq!(
        deferred[0].mcp_info.as_ref().unwrap().server_name,
        "example"
    );
    assert_eq!(
        deferred[0].model_input_schema(),
        deferred[0].input_json_schema.as_ref().unwrap()
    );
    assert_eq!(
        deferred[0].mcp_stub.as_ref().unwrap().reason,
        "example MCP adapter is not configured"
    );
}

#[test]
fn always_load_tools_are_sent_to_model() {
    let mut registry = ToolRegistry::new();
    let mut tool = ToolDefinition::eager("run.complete", object_schema(json!({}), &[]));
    tool.loading_policy = ToolLoadingPolicy::AlwaysLoad;
    registry.register(tool);

    let model_tools = registry.model_tool_definitions();
    assert_eq!(model_tools.len(), 1);
    assert_eq!(model_tools[0].name, "run.complete");
}

#[test]
fn model_and_deferred_tool_catalogs_can_be_limited_by_token_budget() {
    let mut registry = ToolRegistry::new();
    let mut read_tool = ToolDefinition::eager("fs.read", object_schema(json!({}), &[]));
    read_tool.estimated_token_cost = 40;
    registry.register(read_tool);
    let mut shell_tool = ToolDefinition::eager("shell.run", object_schema(json!({}), &[]));
    shell_tool.estimated_token_cost = 80;
    registry.register(shell_tool);
    registry.register(ToolDefinition::deferred_mcp_stub(
        "mcp__example__lookup",
        object_schema(json!({}), &[]),
        McpToolInfo {
            server_name: "example".to_string(),
            tool_name: "lookup".to_string(),
        },
        "example MCP adapter is not configured",
        180,
    ));

    let model_tools = registry.model_tool_definitions_within_budget(90);
    assert_eq!(model_tools.len(), 1);
    assert_eq!(model_tools[0].name, "fs.read");

    assert!(registry.deferred_metadata_within_budget(120).is_empty());
    let deferred = registry.deferred_metadata_within_budget(200);
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0].name, "mcp__example__lookup");
}

#[tokio::test]
async fn sandbox_open_channel_tool_requires_ready_binding() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "project-project-1-sandbox-1".to_string(),
            "project-project-1-sandbox-1".to_string(),
            "workspace-project-project-1-sandbox-1".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    let executor = control_plane_executor();

    let blocked = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-1",
            "sandbox.open_channel",
            json!({ "bindingId": binding.id }),
        )
        .await;
    assert!(blocked.result.is_error);
    assert!(blocked.result.content["error"]
        .as_str()
        .unwrap()
        .contains("wait_ready must complete"));

    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let opened = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-2",
            "sandbox.open_channel",
            json!({ "bindingId": binding.id }),
        )
        .await;

    assert!(!opened.result.is_error);
    assert_eq!(opened.result.content["protocol"], "websocket");
    assert_eq!(
        opened.result.content["endpoint"],
        "ws://project-project-1-sandbox-1.anydesign-sandboxes.svc.cluster.local:3001/workspace"
    );
}

#[tokio::test]
async fn sandbox_claim_wait_ready_open_channel_sequence_uses_binding_contract() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "Project_ABC".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let executor = control_plane_executor();

    let claimed = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-claim",
            "sandbox.claim",
            json!({ "templateKey": "next-app" }),
        )
        .await;
    assert!(!claimed.result.is_error);
    assert_eq!(claimed.result.content["status"], "claiming");
    assert_eq!(
        claimed.result.content["warmPoolName"],
        "anydesign-next-app-pool"
    );
    assert_eq!(claimed.result.content["mode"], "phase_a_contract");

    let binding_id = claimed.result.content["bindingId"].as_str().unwrap();
    assert!(executor.is_concurrency_safe("sandbox.get_status", &json!({ "bindingId": binding_id })));
    let status = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-status",
            "sandbox.get_status",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(!status.result.is_error);
    assert_eq!(status.result.content["status"], "claiming");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().sandbox_id.as_deref(),
        Some(binding_id)
    );
    let child = store
        .create_child_run(
            &run.id,
            AgentPhase::Review,
            "review".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(child.sandbox_id.as_deref(), Some(binding_id));

    let early_open = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-open-early",
            "sandbox.open_channel",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(early_open.result.is_error);
    assert!(early_open.result.content["error"]
        .as_str()
        .unwrap()
        .contains("wait_ready must complete"));

    let ready = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-ready",
            "sandbox.wait_ready",
            json!({ "bindingId": binding_id, "timeoutMs": 120000 }),
        )
        .await;
    assert!(!ready.result.is_error);
    assert_eq!(ready.result.content["status"], "busy");
    assert_eq!(ready.result.content["channelProtocol"], "websocket");
    assert_eq!(
        store.get_sandbox_binding(binding_id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );

    let opened = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-open",
            "sandbox.open_channel",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(!opened.result.is_error);
    assert_eq!(opened.result.content["protocol"], "websocket");
    assert!(opened.result.content["endpoint"]
        .as_str()
        .unwrap()
        .starts_with("ws://project-project-abc-sandbox-"));

    let released = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-release",
            "sandbox.delete",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(!released.result.is_error);
    assert_eq!(released.result.content["status"], "deleted");
    assert_eq!(
        store.get_sandbox_binding(binding_id).await.unwrap().status,
        SandboxBindingStatus::Deleted
    );

    let reopened = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-sandbox-open-deleted",
            "sandbox.open_channel",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(reopened.result.is_error);
    assert!(reopened.result.content["error"]
        .as_str()
        .unwrap()
        .contains("status=Deleted"));

    let states: Vec<_> = store
        .events(&run.id)
        .await
        .into_iter()
        .filter_map(|event| match event {
            anydesign_runtime::types::AgentEvent::StateChanged { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec!["sandbox.claiming", "sandbox.ready", "sandbox.released"]
    );
}

#[tokio::test]
async fn sandbox_tools_can_use_injected_kubernetes_backend() {
    let store = RuntimeStore::new();
    store
        .upsert_project_access(
            "Project_ABC",
            "owner-1".to_string(),
            "ws-kube-sandboxes".to_string(),
        )
        .await
        .unwrap();
    let run = store
        .create_run(
            "Project_ABC".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let client = FakeSandboxClient::with_phases_and_services(
        vec![SandboxClaimPhase::Ready],
        vec![Some("workspace-channel-7f9b".to_string())],
    );
    let executor =
        control_plane_executor_with_sandbox_backend(Arc::new(KubernetesSandboxBackend::new(
            client.clone(),
            SandboxAdapterConfig {
                namespace: "kube-sandboxes".to_string(),
                channel_protocol: SandboxChannelProtocol::Websocket,
                wait_timeout: Duration::from_millis(30),
                poll_interval: Duration::from_millis(1),
            },
        )));

    let claimed = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-kube-claim",
            "sandbox.claim",
            json!({ "templateKey": "next-app" }),
        )
        .await;

    assert!(!claimed.result.is_error);
    assert_eq!(claimed.result.content["mode"], "kubernetes");
    assert_eq!(claimed.result.content["namespace"], "ws-kube-sandboxes");
    assert_eq!(claimed.result.content["status"], "claiming");
    let binding_id = claimed.result.content["bindingId"].as_str().unwrap();
    let workspace_pvc_name = claimed.result.content["workspacePvcName"]
        .as_str()
        .unwrap()
        .to_string();
    let created = client.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].namespace, "ws-kube-sandboxes");
    assert_eq!(created[0].warm_pool_name, "anydesign-next-app-pool");
    assert_eq!(created[0].workspace_pvc_name, workspace_pvc_name);

    let ready = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-kube-ready",
            "sandbox.wait_ready",
            json!({ "bindingId": binding_id, "timeoutMs": 30 }),
        )
        .await;
    assert!(!ready.result.is_error);
    assert_eq!(ready.result.content["mode"], "kubernetes");
    assert_eq!(ready.result.content["status"], "busy");
    assert_eq!(ready.result.content["workspacePvcName"], workspace_pvc_name);
    assert_eq!(
        store.get_sandbox_binding(binding_id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
    assert_eq!(
        ready.result.content["channelServiceName"],
        "workspace-channel-7f9b"
    );

    let opened = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-kube-open",
            "sandbox.open_channel",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(!opened.result.is_error);
    assert_eq!(
        opened.result.content["workspacePvcName"],
        workspace_pvc_name
    );
    assert_eq!(
        opened.result.content["endpoint"],
        "ws://workspace-channel-7f9b.ws-kube-sandboxes.svc.cluster.local:3001/workspace"
    );

    let deleted = executor
        .execute(
            store.clone(),
            &run.id,
            "tool-kube-delete",
            "sandbox.delete",
            json!({ "bindingId": binding_id }),
        )
        .await;
    assert!(!deleted.result.is_error);
    assert_eq!(deleted.result.content["mode"], "kubernetes");
    assert_eq!(deleted.result.content["status"], "deleted");
    assert_eq!(
        deleted.result.content["workspacePvcName"],
        workspace_pvc_name
    );
    assert_eq!(
        client.deleted.lock().unwrap().as_slice(),
        [("ws-kube-sandboxes".to_string(), created[0].name.clone())]
    );
}

#[tokio::test]
async fn run_sandbox_binding_rejects_unknown_binding_id() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;

    let result = store
        .bind_run_to_sandbox(&run.id, "sandbox-binding-missing")
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("sandbox binding not found"));
    assert_eq!(store.get_run(&run.id).await.unwrap().sandbox_id, None);
}

#[tokio::test]
async fn run_sandbox_binding_rejects_cross_project_workspace() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "project-2",
            "project-project-2-sandbox-1".to_string(),
            "project-project-2-sandbox-1".to_string(),
            "workspace-project-project-2-sandbox-1".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();

    let result = store.bind_run_to_sandbox(&run.id, &binding.id).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("sandbox binding project mismatch"));
    assert_eq!(store.get_run(&run.id).await.unwrap().sandbox_id, None);
}

#[tokio::test]
async fn sandbox_binding_rejects_reused_workspace_pvc() {
    let store = RuntimeStore::new();
    store
        .create_sandbox_binding(
            "project-1",
            "project-project-1-sandbox-1".to_string(),
            "project-project-1-sandbox-1".to_string(),
            "workspace-shared".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();

    let result = store
        .create_sandbox_binding(
            "project-2",
            "project-project-2-sandbox-1".to_string(),
            "project-project-2-sandbox-1".to_string(),
            "workspace-shared".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("workspace PVC workspace-shared is already bound"));
}
