use crate::{
    sandbox_adapter::{CommandRunner, TokioCommandRunner},
    sandbox_profiles::{
        BuiltInSandboxExecutionProfileRegistry, SandboxExecutionProfile,
        SandboxExecutionProfileRegistry,
    },
    templates::{
        BuiltInTemplateRegistry, TemplateId, TemplateRegistry, TemplateSpec, UnknownTemplate,
    },
};
use async_trait::async_trait;
use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

type ReadinessCache = HashMap<String, (Instant, Result<bool, String>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateDescriptor {
    pub id: TemplateId,
    pub version: String,
    pub framework: String,
    pub surface: String,
    pub structured_page_write: bool,
    pub mdx_document_write: bool,
    pub static_export: bool,
    pub sandbox_execution_profile_id: String,
    pub sandbox_execution_profile_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateAvailabilityError {
    InvalidId(String),
    Unsupported(TemplateId),
    Disabled(TemplateId),
    ExecutionProfileUnavailable {
        template_id: TemplateId,
        reason: String,
    },
}

impl TemplateAvailabilityError {
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::InvalidId(_) => "template.invalid_id",
            Self::Unsupported(_) => "template.unsupported",
            Self::Disabled(_) => "template.disabled",
            Self::ExecutionProfileUnavailable { .. } => "template.execution_profile_unavailable",
        }
    }
}

impl fmt::Display for TemplateAvailabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(value) => write!(formatter, "template id is invalid: {value}"),
            Self::Unsupported(id) => write!(formatter, "template is not registered: {id}"),
            Self::Disabled(id) => write!(formatter, "template is disabled: {id}"),
            Self::ExecutionProfileUnavailable {
                template_id,
                reason,
            } => write!(
                formatter,
                "template execution profile is unavailable for {template_id}: {reason}"
            ),
        }
    }
}

impl Error for TemplateAvailabilityError {}

#[async_trait]
pub trait SandboxExecutionProfileReadiness: Send + Sync {
    async fn is_ready(&self, profile: &SandboxExecutionProfile) -> Result<bool, String>;
}

#[derive(Debug, Clone, Default)]
pub struct StaticSandboxExecutionProfileReadiness;

#[async_trait]
impl SandboxExecutionProfileReadiness for StaticSandboxExecutionProfileReadiness {
    async fn is_ready(&self, _profile: &SandboxExecutionProfile) -> Result<bool, String> {
        Ok(true)
    }
}

pub struct KubernetesSandboxExecutionProfileReadiness<R = TokioCommandRunner> {
    runner: Arc<R>,
    kubectl: String,
    namespace: String,
    ttl: Duration,
    cache: Mutex<ReadinessCache>,
}

impl KubernetesSandboxExecutionProfileReadiness<TokioCommandRunner> {
    pub fn from_env() -> Self {
        Self::with_runner(
            TokioCommandRunner,
            std::env::var("KUBECTL").unwrap_or_else(|_| "kubectl".to_string()),
            std::env::var("RUNTIME_TEMPLATE_PROBE_NAMESPACE")
                .unwrap_or_else(|_| "ws-template-probe".to_string()),
            Duration::from_secs(
                std::env::var("RUNTIME_EXECUTION_PROFILE_READINESS_TTL_SECONDS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(15),
            ),
        )
    }
}

impl<R> KubernetesSandboxExecutionProfileReadiness<R>
where
    R: CommandRunner + 'static,
{
    pub fn with_runner(
        runner: R,
        kubectl: impl Into<String>,
        namespace: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        Self {
            runner: Arc::new(runner),
            kubectl: kubectl.into(),
            namespace: namespace.into(),
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn probe(&self, profile: &SandboxExecutionProfile) -> Result<bool, String> {
        let template = self
            .get_resource("sandboxtemplate", &profile.sandbox_template_name)
            .await?;
        let image_matches = template
            .pointer("/spec/podTemplate/spec/containers")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|containers| {
                containers.iter().any(|container| {
                    container.get("image").and_then(serde_json::Value::as_str)
                        == Some(profile.image_ref.as_str())
                })
            });
        if !image_matches {
            return Err(format!(
                "SandboxTemplate {} does not use expected image {}",
                profile.sandbox_template_name, profile.image_ref
            ));
        }

        let pool = self
            .get_resource("sandboxwarmpool", &profile.warm_pool_name)
            .await?;
        if pool
            .pointer("/spec/sandboxTemplateRef/name")
            .and_then(serde_json::Value::as_str)
            != Some(profile.sandbox_template_name.as_str())
        {
            return Err(format!(
                "SandboxWarmPool {} does not reference SandboxTemplate {}",
                profile.warm_pool_name, profile.sandbox_template_name
            ));
        }
        let desired = pool
            .pointer("/spec/replicas")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let ready = pool
            .pointer("/status/readyReplicas")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        // A zero-replica pool is the v1beta1 template indirection object. Claims
        // cold-create a Sandbox when no resident pooled Sandbox is available.
        Ok(ready >= desired)
    }

    async fn get_resource(&self, resource: &str, name: &str) -> Result<serde_json::Value, String> {
        let args = vec![
            "get".to_string(),
            resource.to_string(),
            name.to_string(),
            "-n".to_string(),
            self.namespace.clone(),
            "-o".to_string(),
            "json".to_string(),
        ];
        let output = self
            .runner
            .run(&self.kubectl, &args, None)
            .await
            .map_err(|error| format!("kubectl {resource}/{name} probe failed: {error}"))?;
        if !output.status_success {
            return Err(format!(
                "kubectl get {resource}/{name} failed: {}",
                output.stderr.trim()
            ));
        }
        serde_json::from_str(&output.stdout)
            .map_err(|error| format!("kubectl {resource}/{name} returned invalid JSON: {error}"))
    }
}

#[async_trait]
impl<R> SandboxExecutionProfileReadiness for KubernetesSandboxExecutionProfileReadiness<R>
where
    R: CommandRunner + 'static,
{
    async fn is_ready(&self, profile: &SandboxExecutionProfile) -> Result<bool, String> {
        let key = format!("{}:{}", profile.id, profile.version);
        {
            let cache = self.cache.lock().await;
            if let Some((checked_at, result)) = cache.get(&key) {
                if checked_at.elapsed() < self.ttl {
                    return result.clone();
                }
            }
        }
        let result = self.probe(profile).await;
        self.cache
            .lock()
            .await
            .insert(key, (Instant::now(), result.clone()));
        result
    }
}

#[async_trait]
pub trait TemplateAvailabilityService: Send + Sync {
    async fn resolve_for_init(
        &self,
        id: &TemplateId,
    ) -> Result<Arc<TemplateSpec>, TemplateAvailabilityError>;

    async fn enabled_templates(&self) -> Vec<TemplateDescriptor>;
}

pub struct BuiltInTemplateAvailabilityService {
    templates: Arc<dyn TemplateRegistry>,
    profiles: Arc<dyn SandboxExecutionProfileRegistry>,
    readiness: Arc<dyn SandboxExecutionProfileReadiness>,
    enabled: BTreeSet<TemplateId>,
}

impl BuiltInTemplateAvailabilityService {
    pub fn new(
        templates: Arc<dyn TemplateRegistry>,
        profiles: Arc<dyn SandboxExecutionProfileRegistry>,
        readiness: Arc<dyn SandboxExecutionProfileReadiness>,
        enabled: impl IntoIterator<Item = TemplateId>,
    ) -> Self {
        Self {
            templates,
            profiles,
            readiness,
            enabled: enabled.into_iter().collect(),
        }
    }

    pub fn built_in() -> Self {
        let configured = std::env::var("RUNTIME_ENABLED_TEMPLATES").ok();
        let readiness: Arc<dyn SandboxExecutionProfileReadiness> = if use_kubernetes_readiness() {
            Arc::new(KubernetesSandboxExecutionProfileReadiness::from_env())
        } else {
            Arc::new(StaticSandboxExecutionProfileReadiness)
        };
        Self::new(
            Arc::new(BuiltInTemplateRegistry::built_in()),
            Arc::new(BuiltInSandboxExecutionProfileRegistry::built_in()),
            readiness,
            enabled_template_ids(configured.as_deref()),
        )
    }

    async fn resolve_registered(
        &self,
        id: &TemplateId,
    ) -> Result<Arc<TemplateSpec>, TemplateAvailabilityError> {
        self.templates
            .current(id)
            .map_err(|UnknownTemplate(id)| TemplateAvailabilityError::Unsupported(id))
    }
}

fn use_kubernetes_readiness() -> bool {
    match std::env::var("RUNTIME_EXECUTION_PROFILE_READINESS_MODE")
        .ok()
        .as_deref()
    {
        Some("kubernetes" | "k8s") => true,
        Some("static" | "disabled" | "off") => false,
        // Workspace-specific readiness is enforced by the claim in the
        // ProjectAccess namespace. A central Runtime has no tenant namespace
        // it can safely probe as a global default.
        _ => false,
    }
}

fn enabled_template_ids(configured: Option<&str>) -> Vec<TemplateId> {
    configured
        .unwrap_or("astro-website,fumadocs-docs")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| TemplateId::parse(value).ok())
        .collect()
}

impl Default for BuiltInTemplateAvailabilityService {
    fn default() -> Self {
        Self::built_in()
    }
}

#[async_trait]
impl TemplateAvailabilityService for BuiltInTemplateAvailabilityService {
    async fn resolve_for_init(
        &self,
        id: &TemplateId,
    ) -> Result<Arc<TemplateSpec>, TemplateAvailabilityError> {
        let spec = self.resolve_registered(id).await?;
        if !self.enabled.contains(id) {
            return Err(TemplateAvailabilityError::Disabled(id.clone()));
        }
        let profile = self
            .profiles
            .resolve(&spec.sandbox_execution_profile)
            .map_err(
                |error| TemplateAvailabilityError::ExecutionProfileUnavailable {
                    template_id: id.clone(),
                    reason: error.to_string(),
                },
            )?;
        match self.readiness.is_ready(&profile).await {
            Ok(true) => Ok(spec),
            Ok(false) => Err(TemplateAvailabilityError::ExecutionProfileUnavailable {
                template_id: id.clone(),
                reason: "profile readiness check returned false".to_string(),
            }),
            Err(reason) => Err(TemplateAvailabilityError::ExecutionProfileUnavailable {
                template_id: id.clone(),
                reason,
            }),
        }
    }

    async fn enabled_templates(&self) -> Vec<TemplateDescriptor> {
        let mut descriptors = Vec::new();
        for id in &self.enabled {
            let Ok(spec) = self.resolve_for_init(id).await else {
                continue;
            };
            descriptors.push(TemplateDescriptor {
                id: id.clone(),
                version: spec.version.to_string(),
                framework: spec.framework.to_string(),
                surface: spec.surface.to_string(),
                structured_page_write: spec.capabilities.structured_page_write,
                mdx_document_write: spec.capabilities.mdx_document_write,
                static_export: spec.capabilities.static_export,
                sandbox_execution_profile_id: spec.sandbox_execution_profile.id.to_string(),
                sandbox_execution_profile_version: spec
                    .sandbox_execution_profile
                    .version
                    .to_string(),
            });
        }
        descriptors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sandbox_adapter::CommandOutput,
        sandbox_profiles::{
            BuiltInSandboxExecutionProfileRegistry, SandboxExecutionProfileRegistry,
        },
        templates::{BuiltInTemplateRegistry, TemplateRegistry},
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct ReadinessRunner {
        template: serde_json::Value,
        pool: serde_json::Value,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CommandRunner for ReadinessRunner {
        async fn run(
            &self,
            _program: &str,
            args: &[String],
            _stdin: Option<String>,
        ) -> anyhow::Result<CommandOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let value = match args.get(1).map(String::as_str) {
                Some("sandboxtemplate") => &self.template,
                Some("sandboxwarmpool") => &self.pool,
                other => anyhow::bail!("unexpected readiness resource: {other:?}"),
            };
            Ok(CommandOutput {
                stdout: serde_json::to_string(value)?,
                stderr: String::new(),
                status_success: true,
            })
        }
    }

    fn astro_profile() -> Arc<SandboxExecutionProfile> {
        let template = BuiltInTemplateRegistry::built_in()
            .current(&TemplateId::parse("astro-website").unwrap())
            .unwrap();
        BuiltInSandboxExecutionProfileRegistry::built_in()
            .resolve(&template.sandbox_execution_profile)
            .unwrap()
    }

    #[tokio::test]
    async fn kubernetes_readiness_validates_resources_and_caches_result() {
        let profile = astro_profile();
        let calls = Arc::new(AtomicUsize::new(0));
        let readiness = KubernetesSandboxExecutionProfileReadiness::with_runner(
            ReadinessRunner {
                template: json!({
                    "spec": { "podTemplate": { "spec": { "containers": [
                        { "image": profile.image_ref }
                    ] } } }
                }),
                pool: json!({
                    "spec": {
                        "replicas": 1,
                        "sandboxTemplateRef": { "name": profile.sandbox_template_name }
                    },
                    "status": { "readyReplicas": 1 }
                }),
                calls: calls.clone(),
            },
            "kubectl",
            "anydesign-sandboxes",
            Duration::from_secs(60),
        );

        assert_eq!(readiness.is_ready(&profile).await, Ok(true));
        assert_eq!(readiness.is_ready(&profile).await, Ok(true));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "second lookup must use TTL cache"
        );
    }

    #[tokio::test]
    async fn kubernetes_readiness_fails_closed_on_image_drift() {
        let profile = astro_profile();
        let readiness = KubernetesSandboxExecutionProfileReadiness::with_runner(
            ReadinessRunner {
                template: json!({
                    "spec": { "podTemplate": { "spec": { "containers": [
                        { "image": "registry.invalid/drifted:latest" }
                    ] } } }
                }),
                pool: json!({}),
                calls: Arc::new(AtomicUsize::new(0)),
            },
            "kubectl",
            "anydesign-sandboxes",
            Duration::from_secs(60),
        );

        let error = readiness.is_ready(&profile).await.unwrap_err();
        assert!(error.contains("does not use expected image"));
    }
}
