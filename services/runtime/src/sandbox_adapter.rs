use crate::{
    conversation::RuntimeStore,
    sandbox_profiles::{BuiltInSandboxExecutionProfileRegistry, SandboxExecutionProfileRegistry},
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::{SandboxBinding, SandboxBindingStatus, SandboxChannelProtocol},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::{collections::HashMap, env, process::Stdio, sync::Arc, time::Duration};
use tokio::time::{self, Instant};
use tokio::{io::AsyncWriteExt, process::Command};

pub const SANDBOX_CLAIM_API_VERSION: &str = "extensions.agents.x-k8s.io/v1beta1";
pub const SANDBOX_CLAIM_KIND: &str = "SandboxClaim";
pub const WORKSPACE_CHANNEL_PORT: u16 = 3001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxClaimManifest {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: String,
    pub workspace_pvc_name: String,
    pub warm_pool_name: String,
    pub ttl_seconds_after_finished: u32,
}

impl SandboxClaimManifest {
    pub fn new(
        name: impl Into<String>,
        namespace: impl Into<String>,
        warm_pool_name: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            api_version: SANDBOX_CLAIM_API_VERSION.to_string(),
            kind: SANDBOX_CLAIM_KIND.to_string(),
            workspace_pvc_name: workspace_pvc_name(&name),
            name,
            namespace: namespace.into(),
            warm_pool_name: warm_pool_name.into(),
            ttl_seconds_after_finished: 14_400,
        }
    }

    pub fn to_yaml(&self) -> String {
        format!(
            "apiVersion: {}\nkind: {}\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    anydesign.dev/workspace-pvc: {}\n  annotations:\n    anydesign.dev/workspace-scope: sandbox+pvc\nspec:\n  additionalPodMetadata:\n    annotations:\n      anydesign.dev/workspace-pvc: {}\n      anydesign.dev/workspace-scope: sandbox+pvc\n  warmPoolRef:\n    name: {}\n  lifecycle:\n    ttlSecondsAfterFinished: {}\n",
            self.api_version,
            self.kind,
            self.name,
            self.namespace,
            self.workspace_pvc_name,
            self.workspace_pvc_name,
            self.warm_pool_name,
            self.ttl_seconds_after_finished
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxClaimPhase {
    Pending,
    Starting,
    Ready,
    Failed,
    Deleted,
}

impl SandboxClaimPhase {
    fn binding_status(self) -> SandboxBindingStatus {
        match self {
            Self::Pending => SandboxBindingStatus::Claiming,
            Self::Starting => SandboxBindingStatus::Starting,
            Self::Ready => SandboxBindingStatus::Ready,
            Self::Failed => SandboxBindingStatus::Failed,
            Self::Deleted => SandboxBindingStatus::Deleted,
        }
    }
}

#[async_trait]
pub trait SandboxKubeClient: Send + Sync {
    async fn create_claim(&self, manifest: &SandboxClaimManifest) -> Result<()>;
    async fn claim_phase(&self, namespace: &str, claim_name: &str) -> Result<SandboxClaimPhase>;
    async fn channel_service_name(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<Option<String>> {
        let _ = (namespace, claim_name, sandbox_name);
        Ok(None)
    }
    async fn claim_sandbox_name(
        &self,
        namespace: &str,
        claim_name: &str,
    ) -> Result<Option<String>> {
        let _ = (namespace, claim_name);
        Ok(None)
    }
    async fn sandbox_runtime_identity(
        &self,
        namespace: &str,
        sandbox_name: &str,
    ) -> Result<SandboxRuntimeIdentity> {
        let _ = (namespace, sandbox_name);
        Ok(SandboxRuntimeIdentity::default())
    }
    async fn delete_claim(&self, namespace: &str, claim_name: &str) -> Result<()>;
    async fn sandbox_resources_absent(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<bool> {
        let _ = sandbox_name;
        Ok(self.claim_phase(namespace, claim_name).await? == SandboxClaimPhase::Deleted)
    }
}

#[async_trait]
impl<T> SandboxKubeClient for Arc<T>
where
    T: SandboxKubeClient + ?Sized,
{
    async fn create_claim(&self, manifest: &SandboxClaimManifest) -> Result<()> {
        (**self).create_claim(manifest).await
    }

    async fn claim_phase(&self, namespace: &str, claim_name: &str) -> Result<SandboxClaimPhase> {
        (**self).claim_phase(namespace, claim_name).await
    }

    async fn channel_service_name(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<Option<String>> {
        (**self)
            .channel_service_name(namespace, claim_name, sandbox_name)
            .await
    }

    async fn claim_sandbox_name(
        &self,
        namespace: &str,
        claim_name: &str,
    ) -> Result<Option<String>> {
        (**self).claim_sandbox_name(namespace, claim_name).await
    }

    async fn sandbox_runtime_identity(
        &self,
        namespace: &str,
        sandbox_name: &str,
    ) -> Result<SandboxRuntimeIdentity> {
        (**self)
            .sandbox_runtime_identity(namespace, sandbox_name)
            .await
    }

    async fn delete_claim(&self, namespace: &str, claim_name: &str) -> Result<()> {
        (**self).delete_claim(namespace, claim_name).await
    }

    async fn sandbox_resources_absent(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<bool> {
        (**self)
            .sandbox_resources_absent(namespace, claim_name, sandbox_name)
            .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status_success: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxRuntimeIdentity {
    pub sandbox_uid: Option<String>,
    pub pod_uid: Option<String>,
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<String>,
    ) -> Result<CommandOutput>;
}

#[derive(Debug, Clone, Default)]
pub struct TokioCommandRunner;

#[async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<String>,
    ) -> Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(stdin) = stdin {
            let mut child_stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("failed to open stdin for {program}"))?;
            child_stdin.write_all(stdin.as_bytes()).await?;
        }
        let output = child.wait_with_output().await?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status_success: output.status.success(),
        })
    }
}

#[derive(Clone)]
pub struct KubectlSandboxClient<R = TokioCommandRunner> {
    runner: Arc<R>,
    kubectl: String,
}

impl KubectlSandboxClient<TokioCommandRunner> {
    pub fn new() -> Self {
        Self::with_runner(TokioCommandRunner)
    }
}

impl Default for KubectlSandboxClient<TokioCommandRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> KubectlSandboxClient<R>
where
    R: CommandRunner + 'static,
{
    pub fn with_runner(runner: R) -> Self {
        Self {
            runner: Arc::new(runner),
            kubectl: "kubectl".to_string(),
        }
    }

    pub fn with_program(mut self, kubectl: impl Into<String>) -> Self {
        self.kubectl = kubectl.into();
        self
    }
}

#[async_trait]
impl<R> SandboxKubeClient for KubectlSandboxClient<R>
where
    R: CommandRunner + 'static,
{
    async fn create_claim(&self, manifest: &SandboxClaimManifest) -> Result<()> {
        let args = vec!["apply".to_string(), "-f".to_string(), "-".to_string()];
        let output = self
            .runner
            .run(&self.kubectl, &args, Some(manifest.to_yaml()))
            .await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl apply SandboxClaim failed: {}",
                output.stderr
            ));
        }
        Ok(())
    }

    async fn sandbox_resources_absent(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<bool> {
        let args = vec![
            "get".to_string(),
            format!("sandboxclaim/{claim_name}"),
            format!("sandbox/{sandbox_name}"),
            "-n".to_string(),
            namespace.to_string(),
            "--ignore-not-found=true".to_string(),
            "-o".to_string(),
            "name".to_string(),
        ];
        let output = self.runner.run(&self.kubectl, &args, None).await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl verify Sandbox release failed: {}",
                output.stderr
            ));
        }
        Ok(output.stdout.trim().is_empty())
    }

    async fn claim_phase(&self, namespace: &str, claim_name: &str) -> Result<SandboxClaimPhase> {
        let args = vec![
            "get".to_string(),
            "sandboxclaim".to_string(),
            claim_name.to_string(),
            "-n".to_string(),
            namespace.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ];
        let output = self.runner.run(&self.kubectl, &args, None).await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl get SandboxClaim status failed: {}",
                output.stderr
            ));
        }
        parse_claim_phase_from_json(&output.stdout)
    }

    async fn channel_service_name(
        &self,
        namespace: &str,
        claim_name: &str,
        sandbox_name: &str,
    ) -> Result<Option<String>> {
        let args = vec![
            "get".to_string(),
            "services".to_string(),
            "-n".to_string(),
            namespace.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ];
        let output = self.runner.run(&self.kubectl, &args, None).await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl get services for sandbox channel failed: {}",
                output.stderr
            ));
        }
        find_sandbox_channel_service(&output.stdout, claim_name, sandbox_name)
    }

    async fn claim_sandbox_name(
        &self,
        namespace: &str,
        claim_name: &str,
    ) -> Result<Option<String>> {
        let args = vec![
            "get".to_string(),
            "sandboxclaim".to_string(),
            claim_name.to_string(),
            "-n".to_string(),
            namespace.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ];
        let output = self.runner.run(&self.kubectl, &args, None).await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl get SandboxClaim sandbox name failed: {}",
                output.stderr
            ));
        }
        parse_claim_sandbox_name_from_json(&output.stdout)
    }

    async fn sandbox_runtime_identity(
        &self,
        namespace: &str,
        sandbox_name: &str,
    ) -> Result<SandboxRuntimeIdentity> {
        let get_uid = |resource: &str| {
            vec![
                "get".to_string(),
                resource.to_string(),
                sandbox_name.to_string(),
                "-n".to_string(),
                namespace.to_string(),
                "-o".to_string(),
                "json".to_string(),
            ]
        };
        let sandbox = self
            .runner
            .run(&self.kubectl, &get_uid("sandbox"), None)
            .await?;
        if !sandbox.status_success {
            return Err(anyhow!(
                "kubectl get Sandbox identity failed: {}",
                sandbox.stderr
            ));
        }
        let pod = self
            .runner
            .run(&self.kubectl, &get_uid("pod"), None)
            .await?;
        if !pod.status_success {
            return Err(anyhow!(
                "kubectl get Sandbox pod UID failed: {}",
                pod.stderr
            ));
        }
        Ok(SandboxRuntimeIdentity {
            sandbox_uid: parse_metadata_uid(&sandbox.stdout)?,
            pod_uid: parse_metadata_uid(&pod.stdout)?,
        })
    }

    async fn delete_claim(&self, namespace: &str, claim_name: &str) -> Result<()> {
        let args = vec![
            "delete".to_string(),
            "sandboxclaim".to_string(),
            claim_name.to_string(),
            "-n".to_string(),
            namespace.to_string(),
            "--ignore-not-found=true".to_string(),
            "--wait=false".to_string(),
        ];
        let output = self.runner.run(&self.kubectl, &args, None).await?;
        if !output.status_success {
            return Err(anyhow!(
                "kubectl delete SandboxClaim failed: {}",
                output.stderr
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ServiceList {
    #[serde(default)]
    items: Vec<ServiceItem>,
}

#[derive(Debug, Deserialize)]
struct ServiceItem {
    #[serde(default)]
    metadata: ServiceMetadata,
    #[serde(default)]
    spec: ServiceSpec,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceMetadata {
    #[serde(default)]
    name: String,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    annotations: HashMap<String, String>,
    #[serde(default)]
    owner_references: Vec<OwnerReference>,
}

#[derive(Debug, Default, Deserialize)]
struct OwnerReference {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ServiceSpec {
    #[serde(default)]
    ports: Vec<ServicePort>,
}

#[derive(Debug, Default, Deserialize)]
struct ServicePort {
    #[serde(default)]
    name: String,
    #[serde(default)]
    port: u16,
}

pub fn find_sandbox_channel_service(
    services_json: &str,
    claim_name: &str,
    sandbox_name: &str,
) -> Result<Option<String>> {
    let services: ServiceList = serde_json::from_str(services_json)?;
    let mut ranked = services
        .items
        .into_iter()
        .filter_map(|service| {
            let name = service.metadata.name;
            if name.is_empty() {
                return None;
            }
            let score = service_match_score(
                &name,
                &service.metadata.labels,
                &service.metadata.annotations,
                &service.metadata.owner_references,
                &service.spec.ports,
                claim_name,
                sandbox_name,
            );
            (score > 0).then_some((score, name))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_score, left_name), (right_score, right_name)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(ranked.into_iter().next().map(|(_, name)| name))
}

fn service_match_score(
    service_name: &str,
    labels: &HashMap<String, String>,
    annotations: &HashMap<String, String>,
    owner_references: &[OwnerReference],
    ports: &[ServicePort],
    claim_name: &str,
    sandbox_name: &str,
) -> u16 {
    let mut score = 0;
    if service_name == sandbox_name {
        score = score.max(100);
    }
    if service_name == claim_name {
        score = score.max(95);
    }
    for owner in owner_references {
        if owner.name == sandbox_name {
            score = score.max(90);
        }
        if owner.name == claim_name {
            score = score.max(88);
        }
    }
    for value in labels.values().chain(annotations.values()) {
        if value == sandbox_name {
            score = score.max(85);
        }
        if value == claim_name {
            score = score.max(82);
        }
    }
    if service_name.contains(sandbox_name) {
        score = score.max(70);
    }
    if service_name.contains(claim_name) {
        score = score.max(68);
    }
    if score > 0 {
        if ports.iter().any(|port| port.port == 80) {
            score += 10;
        }
        if ports.iter().any(|port| {
            matches!(
                port.name.as_str(),
                "workspace" | "workspace-channel" | "channel" | "http"
            )
        }) {
            score += 5;
        }
    }
    score
}

pub fn parse_claim_phase(value: &str) -> Result<SandboxClaimPhase> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "pending" | "claimed" => Ok(SandboxClaimPhase::Pending),
        "starting" | "provisioning" | "creating" | "initializing" => {
            Ok(SandboxClaimPhase::Starting)
        }
        "ready" => Ok(SandboxClaimPhase::Ready),
        "failed" | "error" => Ok(SandboxClaimPhase::Failed),
        "deleted" | "terminating" => Ok(SandboxClaimPhase::Deleted),
        other => Err(anyhow!("unknown SandboxClaim phase: {other}")),
    }
}

pub fn parse_claim_phase_from_json(value: &str) -> Result<SandboxClaimPhase> {
    let value: serde_json::Value = serde_json::from_str(value)?;
    if let Some(phase) = value
        .pointer("/status/phase")
        .and_then(|phase| phase.as_str())
    {
        return parse_claim_phase(phase);
    }

    if let Some(conditions) = value
        .pointer("/status/conditions")
        .and_then(|conditions| conditions.as_array())
    {
        if let Some(ready) = conditions.iter().find(|condition| {
            condition
                .get("type")
                .and_then(|kind| kind.as_str())
                .is_some_and(|kind| kind == "Ready")
        }) {
            let status = ready
                .get("status")
                .and_then(|status| status.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let reason = ready
                .get("reason")
                .and_then(|reason| reason.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let message = ready
                .get("message")
                .and_then(|message| message.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            if status == "true" {
                return Ok(SandboxClaimPhase::Ready);
            }
            if reason.contains("fail") || message.contains("fail") || message.contains("error") {
                return Ok(SandboxClaimPhase::Failed);
            }
            return Ok(SandboxClaimPhase::Starting);
        }
    }

    if value.pointer("/status/sandbox/name").is_some() {
        return Ok(SandboxClaimPhase::Starting);
    }
    Ok(SandboxClaimPhase::Pending)
}

pub fn parse_claim_sandbox_name_from_json(value: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(value)?;
    Ok(value
        .pointer("/status/sandbox/name")
        .and_then(|name| name.as_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string))
}

pub fn parse_metadata_uid(value: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(value)?;
    Ok(value
        .pointer("/metadata/uid")
        .and_then(|uid| uid.as_str())
        .filter(|uid| !uid.trim().is_empty())
        .map(ToString::to_string))
}

#[derive(Debug, Clone)]
pub struct SandboxAdapterConfig {
    pub namespace: String,
    pub channel_protocol: SandboxChannelProtocol,
    pub wait_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for SandboxAdapterConfig {
    fn default() -> Self {
        Self {
            namespace: "ws-unassigned".to_string(),
            channel_protocol: SandboxChannelProtocol::Websocket,
            wait_timeout: Duration::from_secs(120),
            poll_interval: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxChannel {
    pub binding_id: String,
    pub project_id: String,
    pub sandbox_name: String,
    pub workspace_pvc_name: String,
    pub namespace: String,
    pub protocol: SandboxChannelProtocol,
    pub endpoint: String,
    pub sandbox_uid: Option<String>,
    pub pod_uid: Option<String>,
}

pub struct SandboxAdapter<C> {
    store: RuntimeStore,
    client: C,
    config: SandboxAdapterConfig,
}

impl<C> SandboxAdapter<C>
where
    C: SandboxKubeClient,
{
    pub fn new(store: RuntimeStore, client: C, config: SandboxAdapterConfig) -> Self {
        Self {
            store,
            client,
            config,
        }
    }

    pub fn set_namespace(&mut self, namespace: &str) {
        self.config.namespace = namespace.to_string();
    }

    pub async fn claim(&self, template_key: &str, project_id: &str) -> Result<SandboxBinding> {
        let short_id = self.store.next_id("sandbox");
        let claim_name = sandbox_claim_name(project_id, &short_id);
        let warm_pool_name = resolve_warm_pool_name(template_key)?;
        let manifest = SandboxClaimManifest::new(
            claim_name.clone(),
            self.config.namespace.clone(),
            warm_pool_name.clone(),
        );
        self.client.create_claim(&manifest).await?;
        self.store
            .create_sandbox_binding(
                project_id,
                claim_name.clone(),
                claim_name.clone(),
                workspace_pvc_name(&claim_name),
                warm_pool_name,
                self.config.namespace.clone(),
                self.config.channel_protocol,
            )
            .await
    }

    pub async fn wait_ready(&self, binding_id: &str) -> Result<SandboxBinding> {
        let binding = self
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
        let deadline = Instant::now() + self.config.wait_timeout;

        loop {
            let phase = self
                .client
                .claim_phase(&binding.namespace, &binding.sandbox_claim_name)
                .await?;
            let status = phase.binding_status();
            self.store
                .update_sandbox_binding_status(binding_id, status)
                .await?;
            if phase == SandboxClaimPhase::Ready {
                let sandbox_name = self
                    .client
                    .claim_sandbox_name(&binding.namespace, &binding.sandbox_claim_name)
                    .await?
                    .unwrap_or_else(|| binding.sandbox_name.clone());
                let channel_service_name = self
                    .client
                    .channel_service_name(
                        &binding.namespace,
                        &binding.sandbox_claim_name,
                        &sandbox_name,
                    )
                    .await?;
                let runtime_identity = self
                    .client
                    .sandbox_runtime_identity(&binding.namespace, &sandbox_name)
                    .await?;
                return self
                    .store
                    .update_sandbox_binding_runtime_identity_with_uids(
                        binding_id,
                        sandbox_name,
                        channel_service_name,
                        runtime_identity.sandbox_uid,
                        runtime_identity.pod_uid,
                    )
                    .await;
            }
            if matches!(
                phase,
                SandboxClaimPhase::Failed | SandboxClaimPhase::Deleted
            ) {
                return Err(anyhow!("sandbox claim entered terminal phase: {phase:?}"));
            }
            if Instant::now() >= deadline {
                let failed = self
                    .store
                    .update_sandbox_binding_status(binding_id, SandboxBindingStatus::Failed)
                    .await?;
                return Err(anyhow!(
                    "sandbox_unavailable: {} did not become ready before timeout; binding={}",
                    failed.sandbox_claim_name,
                    failed.id
                ));
            }
            time::sleep(self.config.poll_interval).await;
        }
    }

    pub async fn open_channel(&self, binding_id: &str) -> Result<SandboxChannel> {
        let binding = self
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
        sandbox_channel_from_binding(&binding)
    }

    pub async fn release(&self, binding_id: &str) -> Result<SandboxBinding> {
        let binding = self
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
        self.client
            .delete_claim(&binding.namespace, &binding.sandbox_claim_name)
            .await?;
        let deadline = Instant::now() + self.config.wait_timeout;
        loop {
            if self
                .client
                .sandbox_resources_absent(
                    &binding.namespace,
                    &binding.sandbox_claim_name,
                    &binding.sandbox_name,
                )
                .await?
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "sandbox release did not remove SandboxClaim and Sandbox before timeout: claim={} sandbox={} binding={}",
                    binding.sandbox_claim_name,
                    binding.sandbox_name,
                    binding.id
                ));
            }
            time::sleep(self.config.poll_interval).await;
        }
        self.store
            .update_sandbox_binding_status(binding_id, SandboxBindingStatus::Deleted)
            .await
    }
}

pub fn warm_pool_name(template_key: &str) -> String {
    resolve_warm_pool_name(template_key)
        .unwrap_or_else(|error| panic!("cannot resolve warm pool for {template_key}: {error}"))
}

pub fn resolve_warm_pool_name(template_key: &str) -> Result<String> {
    let id = TemplateId::parse(template_key).map_err(|error| anyhow!(error))?;
    let template = BuiltInTemplateRegistry::built_in()
        .current(&id)
        .map_err(|error| anyhow!(error))?;
    let profile = BuiltInSandboxExecutionProfileRegistry::built_in()
        .resolve(&template.sandbox_execution_profile)
        .map_err(|error| anyhow!(error))?;
    Ok(profile.warm_pool_name.clone())
}

pub fn sandbox_claim_name(project_id: &str, short_id: &str) -> String {
    let project = sanitize_k8s_name(project_id);
    let unique_suffix = sanitize_k8s_name(short_id);
    let project_budget = 63usize.saturating_sub("project--".len() + unique_suffix.len());
    let project = project
        .chars()
        .take(project_budget)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    format!("project-{project}-{unique_suffix}")
}

pub fn workspace_pvc_name(sandbox_claim_name: &str) -> String {
    let claim = sanitize_k8s_name(sandbox_claim_name);
    let claim_budget = 63 - "workspace-".len();
    if claim.len() <= claim_budget {
        return format!("workspace-{claim}");
    }
    let suffix_budget = 20.min(claim_budget.saturating_sub(2));
    let prefix_budget = claim_budget - suffix_budget - 1;
    let prefix = claim
        .chars()
        .take(prefix_budget)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    let suffix = claim
        .chars()
        .rev()
        .take(suffix_budget)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .trim_start_matches('-')
        .to_string();
    format!("workspace-{prefix}-{suffix}")
}

pub fn sandbox_channel_endpoint(
    service_name: &str,
    namespace: &str,
    protocol: SandboxChannelProtocol,
) -> String {
    sandbox_channel_endpoint_with_overrides(
        service_name,
        namespace,
        protocol,
        env::var("SANDBOX_CHANNEL_HOST_OVERRIDE")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        env::var("SANDBOX_CHANNEL_PORT_OVERRIDE")
            .ok()
            .and_then(|value| value.parse::<u16>().ok()),
    )
}

pub fn sandbox_channel_endpoint_with_overrides(
    service_name: &str,
    namespace: &str,
    protocol: SandboxChannelProtocol,
    host_override: Option<String>,
    port_override: Option<u16>,
) -> String {
    let host =
        host_override.unwrap_or_else(|| format!("{service_name}.{namespace}.svc.cluster.local"));
    let port = port_override.unwrap_or(WORKSPACE_CHANNEL_PORT);
    match protocol {
        SandboxChannelProtocol::Websocket => format!("ws://{host}:{port}/workspace"),
        SandboxChannelProtocol::Grpc => format!("grpc://{host}:{port}/workspace"),
    }
}

pub fn sandbox_channel_from_binding(binding: &SandboxBinding) -> Result<SandboxChannel> {
    if !is_channel_openable(binding.status) {
        return Err(anyhow!(
            "sandbox channel unavailable: binding={} status={:?}; wait_ready must complete before open_channel",
            binding.id,
            binding.status
        ));
    }

    Ok(SandboxChannel {
        binding_id: binding.id.clone(),
        project_id: binding.project_id.clone(),
        sandbox_name: binding.sandbox_name.clone(),
        workspace_pvc_name: binding.workspace_pvc_name.clone(),
        namespace: binding.namespace.clone(),
        protocol: binding.channel_protocol,
        endpoint: sandbox_channel_endpoint(
            binding
                .channel_service_name
                .as_deref()
                .unwrap_or(&binding.sandbox_name),
            &binding.namespace,
            binding.channel_protocol,
        ),
        sandbox_uid: binding.sandbox_uid.clone(),
        pod_uid: binding.pod_uid.clone(),
    })
}

fn is_channel_openable(status: SandboxBindingStatus) -> bool {
    matches!(
        status,
        SandboxBindingStatus::Ready | SandboxBindingStatus::Busy | SandboxBindingStatus::Idle
    )
}

fn sanitize_k8s_name(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "sandbox".to_string()
    } else {
        output
    }
}
