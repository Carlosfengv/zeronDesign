use anydesign_runtime::{
    conversation::RuntimeStore,
    sandbox_adapter::{
        find_sandbox_channel_service, parse_claim_phase, parse_claim_phase_from_json,
        sandbox_channel_endpoint, sandbox_channel_endpoint_with_overrides, sandbox_claim_name,
        warm_pool_name, workspace_pvc_name, CommandOutput, CommandRunner, KubectlSandboxClient,
        SandboxAdapter, SandboxAdapterConfig, SandboxClaimManifest, SandboxClaimPhase,
        SandboxKubeClient, SANDBOX_CLAIM_API_VERSION,
    },
    types::{SandboxBindingStatus, SandboxChannelProtocol},
};
use anyhow::Result;
use async_trait::async_trait;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Default, Clone)]
struct FakeSandboxClient {
    created: Arc<Mutex<Vec<SandboxClaimManifest>>>,
    phases: Arc<Mutex<VecDeque<SandboxClaimPhase>>>,
    sandbox_names: Arc<Mutex<VecDeque<Option<String>>>>,
    channel_services: Arc<Mutex<VecDeque<Option<String>>>>,
    deleted: Arc<Mutex<Vec<(String, String)>>>,
}

#[derive(Debug, Clone)]
struct RecordedCommand {
    program: String,
    args: Vec<String>,
    stdin: Option<String>,
}

#[derive(Debug, Clone)]
struct FakeCommandRunner {
    commands: Arc<Mutex<Vec<RecordedCommand>>>,
    outputs: Arc<Mutex<VecDeque<CommandOutput>>>,
}

impl FakeCommandRunner {
    fn new(outputs: Vec<CommandOutput>) -> Self {
        Self {
            commands: Arc::new(Mutex::new(Vec::new())),
            outputs: Arc::new(Mutex::new(VecDeque::from(outputs))),
        }
    }

    fn commands(&self) -> Vec<RecordedCommand> {
        self.commands.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<String>,
    ) -> Result<CommandOutput> {
        self.commands.lock().unwrap().push(RecordedCommand {
            program: program.to_string(),
            args: args.to_vec(),
            stdin,
        });
        Ok(self.outputs.lock().unwrap().pop_front().unwrap())
    }
}

impl FakeSandboxClient {
    fn with_phases(phases: Vec<SandboxClaimPhase>) -> Self {
        Self {
            created: Arc::new(Mutex::new(Vec::new())),
            phases: Arc::new(Mutex::new(VecDeque::from(phases))),
            sandbox_names: Arc::new(Mutex::new(VecDeque::new())),
            channel_services: Arc::new(Mutex::new(VecDeque::new())),
            deleted: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_phases_services_and_sandbox_names(
        phases: Vec<SandboxClaimPhase>,
        channel_services: Vec<Option<String>>,
        sandbox_names: Vec<Option<String>>,
    ) -> Self {
        Self {
            created: Arc::new(Mutex::new(Vec::new())),
            phases: Arc::new(Mutex::new(VecDeque::from(phases))),
            sandbox_names: Arc::new(Mutex::new(VecDeque::from(sandbox_names))),
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

    async fn claim_sandbox_name(
        &self,
        _namespace: &str,
        _claim_name: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .sandbox_names
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

fn test_config() -> SandboxAdapterConfig {
    SandboxAdapterConfig {
        namespace: "anydesign-sandboxes".to_string(),
        channel_protocol: SandboxChannelProtocol::Websocket,
        wait_timeout: Duration::from_millis(30),
        poll_interval: Duration::from_millis(1),
    }
}

#[test]
fn sandbox_claim_manifest_uses_v1beta1_warm_pool_ref_and_workspace_label() {
    let manifest = SandboxClaimManifest::new(
        "project-demo-abc",
        "anydesign-sandboxes",
        "anydesign-next-app-pool",
    );
    let yaml = manifest.to_yaml();

    assert_eq!(manifest.api_version, SANDBOX_CLAIM_API_VERSION);
    assert!(yaml.contains("apiVersion: extensions.agents.x-k8s.io/v1beta1"));
    assert!(yaml.contains("kind: SandboxClaim"));
    assert!(yaml.contains("warmPoolRef:"));
    assert!(yaml.contains("name: anydesign-next-app-pool"));
    assert!(yaml.contains("anydesign.dev/workspace-pvc: workspace-project-demo-abc"));
    assert!(!yaml.contains("additionalPodMetadata:\n    labels:"));
    assert!(yaml.contains("ttlSecondsAfterFinished: 14400"));
    assert!(!yaml.contains("warmpoolRef:"));
    assert!(!yaml.contains("volumeClaimTemplates:"));
    assert!(!yaml.contains("spec:\n  workspace:"));
}

#[test]
fn sandbox_names_are_k8s_safe_and_template_specific() {
    assert_eq!(warm_pool_name("next-app"), "anydesign-next-app-pool");
    assert!(sandbox_claim_name("Project_ABC", "sandbox-123").starts_with("project-project-abc"));
    assert!(sandbox_claim_name("x", "y").len() <= 63);

    let long_project = "real-20260718142059-zenova-agent-cloud-with-a-long-project-name";
    let first = sandbox_claim_name(long_project, "sandbox-1848");
    let second = sandbox_claim_name(long_project, "sandbox-1849");
    assert_ne!(first, second);
    assert!(first.ends_with("sandbox-1848"));
    assert!(second.ends_with("sandbox-1849"));
    assert!(first.len() <= 63);
    let first_pvc = workspace_pvc_name(&first);
    let second_pvc = workspace_pvc_name(&second);
    assert_ne!(first_pvc, second_pvc);
    assert!(first_pvc.ends_with("sandbox-1848"));
    assert!(second_pvc.ends_with("sandbox-1849"));
    assert!(first_pvc.len() <= 63);
}

#[test]
fn sandbox_claim_phase_parser_maps_agent_sandbox_statuses() {
    assert_eq!(parse_claim_phase("").unwrap(), SandboxClaimPhase::Pending);
    assert_eq!(
        parse_claim_phase("Provisioning").unwrap(),
        SandboxClaimPhase::Starting
    );
    assert_eq!(
        parse_claim_phase("Ready").unwrap(),
        SandboxClaimPhase::Ready
    );
    assert_eq!(
        parse_claim_phase("Failed").unwrap(),
        SandboxClaimPhase::Failed
    );
    assert_eq!(
        parse_claim_phase("Terminating").unwrap(),
        SandboxClaimPhase::Deleted
    );
    assert!(parse_claim_phase("Wat").is_err());
}

#[test]
fn sandbox_channel_endpoint_uses_selected_protocol_and_internal_dns() {
    assert_eq!(
        sandbox_channel_endpoint(
            "project-demo-sandbox",
            "anydesign-sandboxes",
            SandboxChannelProtocol::Websocket
        ),
        "ws://project-demo-sandbox.anydesign-sandboxes.svc.cluster.local:3001/workspace"
    );
    assert_eq!(
        sandbox_channel_endpoint(
            "project-demo-sandbox",
            "anydesign-sandboxes",
            SandboxChannelProtocol::Grpc
        ),
        "grpc://project-demo-sandbox.anydesign-sandboxes.svc.cluster.local:3001/workspace"
    );
}

#[test]
fn sandbox_channel_endpoint_can_target_local_port_forward_for_desktop_runtime() {
    assert_eq!(
        sandbox_channel_endpoint_with_overrides(
            "project-demo-sandbox",
            "anydesign-sandboxes",
            SandboxChannelProtocol::Websocket,
            Some("127.0.0.1".to_string()),
            Some(39001),
        ),
        "ws://127.0.0.1:39001/workspace"
    );
}

#[test]
fn sandbox_channel_service_discovery_prefers_exact_and_channel_ports() {
    let services = r#"{
      "items": [
        {
          "metadata": {
            "name": "project-demo-sidecar",
            "labels": { "agents.x-k8s.io/sandbox": "project-demo-sandbox" }
          },
          "spec": { "ports": [{ "name": "metrics", "port": 9090 }] }
        },
        {
          "metadata": { "name": "project-demo-sandbox" },
          "spec": { "ports": [{ "name": "workspace", "port": 80 }] }
        }
      ]
    }"#;

    assert_eq!(
        find_sandbox_channel_service(services, "project-demo-claim", "project-demo-sandbox")
            .unwrap(),
        Some("project-demo-sandbox".to_string())
    );
}

#[test]
fn sandbox_channel_service_discovery_uses_owner_or_label_when_names_differ() {
    let services = r#"{
      "items": [
        {
          "metadata": {
            "name": "workspace-channel-7f9b",
            "ownerReferences": [{ "name": "project-demo-sandbox" }]
          },
          "spec": { "ports": [{ "name": "workspace-channel", "port": 80 }] }
        },
        {
          "metadata": {
            "name": "other-service",
            "labels": { "agents.x-k8s.io/claim": "different-claim" }
          },
          "spec": { "ports": [{ "name": "http", "port": 80 }] }
        }
      ]
    }"#;

    assert_eq!(
        find_sandbox_channel_service(services, "project-demo-claim", "project-demo-sandbox")
            .unwrap(),
        Some("workspace-channel-7f9b".to_string())
    );
}

#[test]
fn sandbox_channel_service_discovery_returns_none_without_match() {
    let services = r#"{
      "items": [
        {
          "metadata": { "name": "unrelated" },
          "spec": { "ports": [{ "name": "http", "port": 80 }] }
        }
      ]
    }"#;

    assert_eq!(
        find_sandbox_channel_service(services, "project-demo-claim", "project-demo-sandbox")
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn kubectl_client_applies_manifest_and_reads_claim_phase() {
    let runner = FakeCommandRunner::new(vec![
        CommandOutput {
            stdout: "sandboxclaim created".to_string(),
            stderr: String::new(),
            status_success: true,
        },
        CommandOutput {
            stdout: r#"{
              "status": {
                "conditions": [
                  { "type": "Ready", "status": "True", "reason": "Ready" }
                ],
                "sandbox": { "name": "project-demo-sandbox" }
              }
            }"#
            .to_string(),
            stderr: String::new(),
            status_success: true,
        },
        CommandOutput {
            stdout: r#"{
              "status": {
                "sandbox": { "name": "project-demo-sandbox" }
              }
            }"#
            .to_string(),
            stderr: String::new(),
            status_success: true,
        },
        CommandOutput {
            stdout: r#"{
              "items": [
                {
                  "metadata": { "name": "project-demo-sandbox" },
                  "spec": { "ports": [{ "name": "workspace", "port": 80 }] }
                }
              ]
            }"#
            .to_string(),
            stderr: String::new(),
            status_success: true,
        },
        CommandOutput {
            stdout: "sandboxclaim deleted".to_string(),
            stderr: String::new(),
            status_success: true,
        },
    ]);
    let client = KubectlSandboxClient::with_runner(runner.clone()).with_program("kubectl-test");
    let manifest = SandboxClaimManifest::new(
        "project-demo-sandbox",
        "anydesign-sandboxes",
        "anydesign-next-app-pool",
    );

    client.create_claim(&manifest).await.unwrap();
    let phase = client
        .claim_phase("anydesign-sandboxes", "project-demo-sandbox")
        .await
        .unwrap();
    let sandbox_name = client
        .claim_sandbox_name("anydesign-sandboxes", "project-demo-sandbox")
        .await
        .unwrap()
        .unwrap();
    let service_name = client
        .channel_service_name("anydesign-sandboxes", "project-demo-claim", &sandbox_name)
        .await
        .unwrap();

    assert_eq!(phase, SandboxClaimPhase::Ready);
    assert_eq!(sandbox_name, "project-demo-sandbox");
    assert_eq!(service_name, Some("project-demo-sandbox".to_string()));
    client
        .delete_claim("anydesign-sandboxes", "project-demo-sandbox")
        .await
        .unwrap();
    let commands = runner.commands();
    assert_eq!(commands.len(), 5);
    assert_eq!(commands[0].program, "kubectl-test");
    assert_eq!(commands[0].args, vec!["apply", "-f", "-"]);
    assert!(commands[0].stdin.as_ref().unwrap().contains("warmPoolRef:"));
    assert_eq!(
        commands[1].args,
        vec![
            "get",
            "sandboxclaim",
            "project-demo-sandbox",
            "-n",
            "anydesign-sandboxes",
            "-o",
            "json",
        ]
    );
    assert_eq!(
        commands[2].args,
        vec![
            "get",
            "sandboxclaim",
            "project-demo-sandbox",
            "-n",
            "anydesign-sandboxes",
            "-o",
            "json",
        ]
    );
    assert_eq!(
        commands[3].args,
        vec!["get", "services", "-n", "anydesign-sandboxes", "-o", "json",]
    );
    assert_eq!(
        commands[4].args,
        vec![
            "delete",
            "sandboxclaim",
            "project-demo-sandbox",
            "-n",
            "anydesign-sandboxes",
            "--ignore-not-found=true",
            "--wait=false",
        ]
    );
}

#[test]
fn sandbox_claim_status_parser_accepts_v1beta1_ready_condition() {
    let phase = parse_claim_phase_from_json(
        r#"{
          "status": {
            "conditions": [
              { "type": "Ready", "status": "True", "reason": "Ready" }
            ],
            "sandbox": { "name": "sandbox-1" }
          }
        }"#,
    )
    .unwrap();

    assert_eq!(phase, SandboxClaimPhase::Ready);
}

#[tokio::test]
async fn kubectl_client_reports_apply_and_get_failures() {
    let apply_runner = FakeCommandRunner::new(vec![CommandOutput {
        stdout: String::new(),
        stderr: "forbidden".to_string(),
        status_success: false,
    }]);
    let client = KubectlSandboxClient::with_runner(apply_runner);
    let manifest = SandboxClaimManifest::new(
        "project-demo-sandbox",
        "anydesign-sandboxes",
        "anydesign-next-app-pool",
    );
    assert!(client
        .create_claim(&manifest)
        .await
        .unwrap_err()
        .to_string()
        .contains("kubectl apply"));

    let get_runner = FakeCommandRunner::new(vec![CommandOutput {
        stdout: String::new(),
        stderr: "not found".to_string(),
        status_success: false,
    }]);
    let client = KubectlSandboxClient::with_runner(get_runner);
    assert!(client
        .claim_phase("anydesign-sandboxes", "missing")
        .await
        .unwrap_err()
        .to_string()
        .contains("kubectl get"));

    let delete_runner = FakeCommandRunner::new(vec![CommandOutput {
        stdout: String::new(),
        stderr: "forbidden".to_string(),
        status_success: false,
    }]);
    let client = KubectlSandboxClient::with_runner(delete_runner);
    assert!(client
        .delete_claim("anydesign-sandboxes", "missing")
        .await
        .unwrap_err()
        .to_string()
        .contains("kubectl delete"));
}

#[tokio::test]
async fn arc_kube_client_delegates_terminal_absence_verification() {
    let runner = FakeCommandRunner::new(vec![CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        status_success: true,
    }]);
    let client: Arc<dyn SandboxKubeClient> =
        Arc::new(KubectlSandboxClient::with_runner(runner.clone()).with_program("kubectl-test"));

    assert!(client
        .sandbox_resources_absent(
            "anydesign-sandboxes",
            "project-demo-claim",
            "project-demo-sandbox",
            "workspace-project-demo-claim",
        )
        .await
        .unwrap());
    assert_eq!(
        runner.commands()[0].args,
        vec![
            "get",
            "sandboxclaim/project-demo-claim",
            "sandbox/project-demo-sandbox",
            "pvc/workspace-project-demo-sandbox",
            "pvc/workspace-project-demo-claim",
            "-n",
            "anydesign-sandboxes",
            "--ignore-not-found=true",
            "-o",
            "name",
        ]
    );
}

#[tokio::test]
async fn claim_creates_manifest_and_binding() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::default();
    let adapter = SandboxAdapter::new(store.clone(), client.clone(), test_config());

    let binding = adapter.claim("next-app", "project-1").await.unwrap();

    assert_eq!(binding.project_id, "project-1");
    assert_eq!(binding.warm_pool_name, "anydesign-next-app-pool");
    assert_eq!(
        binding.workspace_pvc_name,
        workspace_pvc_name(&binding.sandbox_claim_name)
    );
    assert_eq!(binding.status, SandboxBindingStatus::Claiming);
    assert_eq!(binding.channel_protocol, SandboxChannelProtocol::Websocket);
    let created = client.created.lock().unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].warm_pool_name, "anydesign-next-app-pool");
    assert_eq!(created[0].workspace_pvc_name, binding.workspace_pvc_name);
    let claim_yaml = created[0].to_yaml();
    assert!(claim_yaml.contains(&format!(
        "anydesign.dev/workspace-pvc: {}",
        binding.workspace_pvc_name
    )));
}

#[tokio::test]
async fn wait_ready_updates_binding_status() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::with_phases(vec![
        SandboxClaimPhase::Pending,
        SandboxClaimPhase::Starting,
        SandboxClaimPhase::Ready,
    ]);
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter.claim("next-app", "project-1").await.unwrap();

    let ready = adapter.wait_ready(&binding.id).await.unwrap();

    assert_eq!(ready.status, SandboxBindingStatus::Ready);
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Ready
    );
}

#[tokio::test]
async fn open_channel_rejects_binding_before_ready() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::default();
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter.claim("next-app", "project-1").await.unwrap();

    let error = adapter.open_channel(&binding.id).await.unwrap_err();

    assert!(error.to_string().contains("wait_ready must complete"));
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Claiming
    );
}

#[tokio::test]
async fn open_channel_returns_ready_workspace_endpoint() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::with_phases_services_and_sandbox_names(
        vec![SandboxClaimPhase::Ready],
        vec![Some("workspace-channel-7f9b".to_string())],
        vec![Some("actual-sandbox-7f9b".to_string())],
    );
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter.claim("next-app", "project-1").await.unwrap();
    let ready = adapter.wait_ready(&binding.id).await.unwrap();

    let channel = adapter.open_channel(&ready.id).await.unwrap();

    assert_eq!(channel.binding_id, ready.id);
    assert_eq!(channel.project_id, "project-1");
    assert_eq!(ready.sandbox_name, "actual-sandbox-7f9b");
    assert_eq!(channel.sandbox_name, "actual-sandbox-7f9b");
    assert_eq!(
        ready.workspace_pvc_name,
        workspace_pvc_name(&ready.sandbox_claim_name)
    );
    assert_eq!(channel.workspace_pvc_name, ready.workspace_pvc_name);
    assert_eq!(channel.namespace, "anydesign-sandboxes");
    assert_eq!(channel.protocol, SandboxChannelProtocol::Websocket);
    assert_eq!(
        ready.channel_service_name,
        Some("workspace-channel-7f9b".to_string())
    );
    assert_eq!(
        channel.endpoint,
        "ws://workspace-channel-7f9b.anydesign-sandboxes.svc.cluster.local:3001/workspace"
    );
}

#[tokio::test]
async fn release_deletes_claim_and_marks_binding_deleted() {
    let store = RuntimeStore::new();
    let client =
        FakeSandboxClient::with_phases(vec![SandboxClaimPhase::Ready, SandboxClaimPhase::Deleted]);
    let adapter = SandboxAdapter::new(store.clone(), client.clone(), test_config());
    let binding = adapter.claim("next-app", "project-1").await.unwrap();
    let ready = adapter.wait_ready(&binding.id).await.unwrap();

    let deleted = adapter.release(&ready.id).await.unwrap();

    assert_eq!(deleted.status, SandboxBindingStatus::Deleted);
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Deleted
    );
    assert_eq!(
        client.deleted.lock().unwrap().as_slice(),
        [(
            "anydesign-sandboxes".to_string(),
            ready.sandbox_claim_name.clone()
        )]
    );
    assert!(adapter.open_channel(&ready.id).await.is_err());
}

#[tokio::test]
async fn release_waits_until_claim_and_sandbox_are_absent() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::with_phases(vec![
        SandboxClaimPhase::Ready,
        SandboxClaimPhase::Ready,
        SandboxClaimPhase::Deleted,
    ]);
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter
        .claim("next-app", "project-release-wait")
        .await
        .unwrap();
    let ready = adapter.wait_ready(&binding.id).await.unwrap();

    let deleted = adapter.release(&ready.id).await.unwrap();

    assert_eq!(deleted.status, SandboxBindingStatus::Deleted);
}

#[tokio::test]
async fn release_timeout_does_not_mark_binding_deleted() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::with_phases(vec![SandboxClaimPhase::Ready]);
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter
        .claim("next-app", "project-release-timeout")
        .await
        .unwrap();
    let ready = adapter.wait_ready(&binding.id).await.unwrap();

    let error = adapter.release(&ready.id).await.unwrap_err();

    assert!(error
        .to_string()
        .contains("did not remove SandboxClaim and Sandbox"));
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Ready
    );
}

#[tokio::test]
async fn wait_ready_timeout_marks_binding_failed() {
    let store = RuntimeStore::new();
    let client = FakeSandboxClient::with_phases(vec![SandboxClaimPhase::Pending]);
    let adapter = SandboxAdapter::new(store.clone(), client, test_config());
    let binding = adapter.claim("next-app", "project-1").await.unwrap();

    let result = adapter.wait_ready(&binding.id).await;

    assert!(result.is_err());
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Failed
    );
}
