pub mod storage;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::{
    extract::DefaultBodyLimit,
    extract::Path,
    extract::Query,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    fmt::Write as _,
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, RwLock, Semaphore};

use crate::storage::{
    AuditEventRecord, PersistentStore, ResourceHealthState, StoredAdminOperation, StoredTurn,
};

pub const TURN_REQUEST_SCHEMA: &str = "provider-gateway-turn-request@1";
pub const TURN_RESPONSE_SCHEMA: &str = "provider-gateway-turn-response@1";
pub const ERROR_SCHEMA: &str = "provider-gateway-error@1";
pub const MODEL_RESOURCE_SCHEMA: &str = "model-resource@1";
pub const MODEL_SELECTION_POLICY_SCHEMA: &str = "model-selection-policy@1";
pub const MODEL_EXECUTION_SNAPSHOT_SCHEMA: &str = "model-execution-snapshot@1";

const MAX_TURN_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOOL_COUNT: usize = 128;
const MAX_TOOL_SCHEMA_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_COUNT: usize = 256;
const MAX_SYSTEM_PROMPT_BYTES: usize = 256 * 1024;
const MAX_JSON_STRING_BYTES: usize = 256 * 1024;
const MAX_JSON_DEPTH: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub database_url: Option<String>,
    #[serde(default, skip_deserializing)]
    pub runtime_bearer_token: Option<String>,
    #[serde(default, skip_deserializing)]
    pub admin_bearer_token: Option<String>,
    #[serde(default)]
    pub resources: Vec<ModelResource>,
    #[serde(default)]
    pub policies: Vec<ModelSelectionPolicy>,
}

fn default_listen() -> String {
    "0.0.0.0:9000".to_string()
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
        let path = env::var("PROVIDER_GATEWAY_CONFIG_FILE").map_err(|_| {
            anyhow!(
                "PROVIDER_GATEWAY_CONFIG_FILE is required and must point to a model resource config"
            )
        })?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("reading provider gateway config {path}"))?;
        let mut config: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parsing provider gateway config {path} as JSON"))?;
        config.listen = env::var("PROVIDER_GATEWAY_LISTEN").unwrap_or(config.listen);
        config.database_url = optional_env_or_file(
            "PROVIDER_GATEWAY_DATABASE_URL",
            "PROVIDER_GATEWAY_DATABASE_URL_FILE",
        )?
        .or(config.database_url);
        if config.database_url.is_none() {
            return Err(anyhow!(
                "PROVIDER_GATEWAY_DATABASE_URL or PROVIDER_GATEWAY_DATABASE_URL_FILE is required; use sqlite:/path/to/gateway.db for development"
            ));
        }
        config.runtime_bearer_token = env::var("PROVIDER_GATEWAY_RUNTIME_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
            .or_else(|| {
                env::var("PROVIDER_GATEWAY_RUNTIME_TOKEN_FILE")
                    .ok()
                    .and_then(|path| fs::read_to_string(path).ok())
                    .map(|token| token.trim().to_string())
                    .filter(|token| !token.is_empty())
            });
        config.admin_bearer_token = env::var("PROVIDER_GATEWAY_ADMIN_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
            .or_else(|| {
                env::var("PROVIDER_GATEWAY_ADMIN_TOKEN_FILE")
                    .ok()
                    .and_then(|path| fs::read_to_string(path).ok())
                    .map(|token| token.trim().to_string())
                    .filter(|token| !token.is_empty())
            });
        Ok(config)
    }
}

fn optional_env_or_file(value_name: &str, file_name: &str) -> Result<Option<String>> {
    if let Some(value) = env::var(value_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(value));
    }
    let Some(path) = env::var(file_name)
        .ok()
        .filter(|path| !path.trim().is_empty())
    else {
        return Ok(None);
    };
    let value = fs::read_to_string(&path)
        .with_context(|| format!("reading {file_name} from {path}"))?
        .trim()
        .to_string();
    if value.is_empty() {
        return Err(anyhow!("{file_name} resolved to an empty value"));
    }
    Ok(Some(value))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayTurnRequest {
    pub schema_version: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub deadline_at: String,
    pub scope: TurnScope,
    pub routing: TurnRouting,
    pub input: TurnInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnScope {
    pub workspace_id: String,
    pub project_id: String,
    pub run_id: String,
    pub turn: u32,
    pub phase: String,
    pub agent_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRouting {
    #[serde(default)]
    pub model_resource_id: Option<String>,
    #[serde(default)]
    pub required_capabilities: RequiredCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiredCapabilities {
    #[serde(default)]
    pub tool_calls: bool,
    #[serde(default)]
    pub strict_tool_schema: bool,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    pub system_prompt: String,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub deferred_tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub input_schema: Value,
    #[serde(default)]
    pub input_json_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResource {
    pub schema_version: String,
    pub id: String,
    pub display_name: String,
    pub kind: ModelResourceKind,
    pub enabled: bool,
    pub revision: u64,
    pub endpoint: ProviderEndpoint,
    pub auth: ProviderAuth,
    pub physical_model: String,
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
    #[serde(default)]
    pub defaults: ModelDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelResourceKind {
    OpenaiCompatible,
    Fixture,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEndpoint {
    pub base_url: String,
    #[serde(default = "default_chat_completions_path")]
    pub chat_completions_path: String,
}

fn default_chat_completions_path() -> String {
    "/chat/completions".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuth {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub secret_ref: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub tool_calls: bool,
    #[serde(default)]
    pub strict_tool_schema: bool,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefaults {
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
}

impl Default for ModelDefaults {
    fn default() -> Self {
        Self {
            request_timeout_ms: default_request_timeout_ms(),
            max_attempts: default_max_attempts(),
            temperature: None,
            max_concurrent_requests: default_max_concurrent_requests(),
        }
    }
}

fn default_request_timeout_ms() -> u64 {
    180_000
}

fn default_max_attempts() -> u32 {
    1
}

fn default_max_concurrent_requests() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionPolicy {
    pub schema_version: String,
    pub id: String,
    pub revision: u64,
    pub scope: PolicyScope,
    #[serde(default)]
    pub applies_to: PolicyApplicability,
    #[serde(default)]
    pub candidates: Vec<ModelCandidate>,
    #[serde(default)]
    pub automatic_switch: AutomaticSwitchPolicy,
    #[serde(default)]
    pub direct_selection: DirectSelectionPolicy,
    #[serde(default)]
    pub limits: ModelSelectionLimits,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionLimits {
    #[serde(default)]
    pub max_concurrent_turns: Option<usize>,
    #[serde(default)]
    pub daily_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyScope {
    #[serde(default)]
    pub workspace_ids: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
}

/// Optional turn-level selector. Empty lists mean "all" so existing policies
/// stay backwards compatible while a project can add a Build/Edit-specific
/// policy without binding a model to the Run itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyApplicability {
    #[serde(default)]
    pub phases: Vec<String>,
    #[serde(default)]
    pub agent_profiles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCandidate {
    pub model_resource_id: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_priority() -> u32 {
    100
}

fn default_weight() -> u32 {
    100
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectSelectionPolicy {
    #[serde(default)]
    pub allowed_model_resource_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticSwitchPolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_reasons: Vec<String>,
    #[serde(default)]
    pub max_model_switches_per_turn: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayTurnResponse {
    pub schema_version: String,
    pub request_id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    #[serde(default)]
    pub tool_calls: Vec<GatewayToolCall>,
    #[serde(default)]
    pub text: Option<String>,
    pub finish_reason: String,
    pub model_execution: ModelExecutionSummary,
    #[serde(default)]
    pub usage: Usage,
    pub provider: ProviderMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelExecutionSummary {
    pub id: String,
    pub model_resource_id: String,
    pub model_resource_revision: u64,
    pub provider_id: String,
    pub physical_model: String,
    pub selection_policy_id: String,
    pub selection_policy_revision: u64,
    pub capability_snapshot_hash: String,
    pub selection_reason: String,
    pub automatic_switch: AutomaticSwitchSummary,
    /// Low-sensitivity correlation metadata. These are copied from Gateway's
    /// normalized attempt record, never from an untrusted Provider body.
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub provider_attempt_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticSwitchSummary {
    pub used: bool,
    pub reason: Option<String>,
    pub from_model_resource_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMetadata {
    pub request_id: Option<String>,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayErrorEnvelope {
    pub schema_version: String,
    pub request_id: String,
    pub error: GatewayErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayErrorDetail {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GatewayApiError {
    status: StatusCode,
    envelope: Box<GatewayErrorEnvelope>,
}

impl GatewayApiError {
    fn new(
        status: StatusCode,
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            envelope: Box::new(GatewayErrorEnvelope {
                schema_version: ERROR_SCHEMA.to_string(),
                request_id: request_id.into(),
                error: GatewayErrorDetail {
                    code: code.into(),
                    message: message.into(),
                    retryable,
                    retry_after_ms: None,
                    provider_request_id: None,
                },
            }),
        }
    }

    fn with_provider_request_id(mut self, provider_request_id: Option<String>) -> Self {
        self.envelope.error.provider_request_id = provider_request_id;
        self
    }

    fn with_retry_after_ms(mut self, retry_after_ms: u64) -> Self {
        self.envelope.error.retry_after_ms = Some(retry_after_ms);
        self
    }

    fn code(&self) -> &str {
        &self.envelope.error.code
    }

    fn retry_after_ms(&self) -> Option<u64> {
        self.envelope.error.retry_after_ms
    }

    fn is_retryable_upstream(&self) -> bool {
        self.envelope.error.retryable
            && matches!(
                self.envelope.error.code.as_str(),
                "provider_rate_limited"
                    | "provider_unavailable"
                    | "provider_timeout"
                    | "provider_response_invalid"
            )
    }
}

impl IntoResponse for GatewayApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.envelope.clone())).into_response();
        if let Some(retry_after_ms) = self.envelope.error.retry_after_ms {
            if let Ok(value) = HeaderValue::from_str(&retry_after_ms.to_string()) {
                response.headers_mut().insert("retry-after-ms", value);
            }
        }
        response
    }
}

#[derive(Clone)]
pub struct GatewayService {
    inner: Arc<GatewayInner>,
}

struct GatewayInner {
    config: GatewayConfig,
    gitops_config_file: Option<PathBuf>,
    resources: RwLock<BTreeMap<String, ModelResource>>,
    policies: RwLock<Vec<ModelSelectionPolicy>>,
    idempotency: Mutex<HashMap<String, IdempotencyEntry>>,
    bulkheads: Mutex<HashMap<String, Arc<Semaphore>>>,
    metrics: Mutex<GatewayMetrics>,
    accepting_turns: AtomicBool,
    storage: Mutex<PersistentStore>,
    cipher: DataCipher,
    client: Client,
}

enum IdempotencyEntry {
    InProgress {
        request_hash: String,
        completion: watch::Sender<bool>,
    },
    Completed {
        request_hash: String,
        response: Box<GatewayTurnResponse>,
    },
    Failed {
        request_hash: String,
        error: GatewayApiError,
    },
}

struct PolicyQuotaLease {
    period_utc: String,
    workspace_id: String,
    project_id: String,
    reserved_input_tokens: u64,
    concurrency_lease_id: Option<String>,
}

#[derive(Clone)]
struct DataCipher {
    cipher: Aes256Gcm,
}

impl DataCipher {
    fn new(secret: &str) -> Result<Self> {
        if secret.trim().len() < 32 {
            return Err(anyhow!(
                "PROVIDER_GATEWAY_ENCRYPTION_KEY must contain at least 32 characters"
            ));
        }
        let digest = Sha256::digest(secret.trim().as_bytes());
        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(&digest)
                .map_err(|_| anyhow!("building Provider Gateway data cipher"))?,
        })
    }

    fn development() -> Self {
        Self::new("provider-gateway-development-key-not-for-production")
            .expect("development encryption key is valid")
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| anyhow!("encrypting Provider Gateway protected data"))?;
        let mut encoded = nonce.to_vec();
        encoded.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(encoded))
    }

    fn decrypt(&self, encoded: &str) -> Result<Vec<u8>> {
        let encoded = URL_SAFE_NO_PAD
            .decode(encoded)
            .context("decoding Provider Gateway protected data")?;
        if encoded.len() <= 12 {
            return Err(anyhow!("Provider Gateway protected data is truncated"));
        }
        self.cipher
            .decrypt((&encoded[..12]).into(), &encoded[12..])
            .map_err(|_| anyhow!("decrypting Provider Gateway protected data"))
    }
}

/// A compile-time Provider extension boundary. Adapters receive the already
/// authorized resource and normalized turn; they cannot choose a resource,
/// resolve arbitrary secrets, or bypass Gateway retry/audit handling.
#[async_trait]
trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> ModelResourceKind;

    async fn execute_turn(
        &self,
        gateway: &GatewayService,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<ProviderTurn, GatewayApiError>;
}

struct OpenAiCompatibleAdapter;
struct FixtureAdapter;

#[async_trait]
impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn kind(&self) -> ModelResourceKind {
        ModelResourceKind::OpenaiCompatible
    }

    async fn execute_turn(
        &self,
        gateway: &GatewayService,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<ProviderTurn, GatewayApiError> {
        gateway.call_openai_compatible(resource, request).await
    }
}

#[async_trait]
impl ProviderAdapter for FixtureAdapter {
    fn kind(&self) -> ModelResourceKind {
        ModelResourceKind::Fixture
    }

    async fn execute_turn(
        &self,
        _gateway: &GatewayService,
        _resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<ProviderTurn, GatewayApiError> {
        Err(GatewayApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            request.request_id.clone(),
            "provider_unavailable",
            "The fixture adapter is not available in this production gateway binary",
            false,
        ))
    }
}

static OPENAI_COMPATIBLE_ADAPTER: OpenAiCompatibleAdapter = OpenAiCompatibleAdapter;
static FIXTURE_ADAPTER: FixtureAdapter = FixtureAdapter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitopsReconcileReport {
    pub config_digest: String,
    pub changed_model_resources: Vec<String>,
    pub changed_model_selection_policies: Vec<String>,
}

/// In-process low-cardinality metric registry.  Deliberately do not include
/// workspace, project, run, or request identifiers in any metric key.
#[derive(Default)]
struct GatewayMetrics {
    counters: BTreeMap<MetricKey, u64>,
    durations: BTreeMap<MetricKey, DurationMetric>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MetricKey {
    name: &'static str,
    labels: Vec<(&'static str, String)>,
}

#[derive(Default)]
struct DurationMetric {
    count: u64,
    sum_seconds: f64,
}

impl MetricKey {
    fn new(name: &'static str, labels: Vec<(&'static str, String)>) -> Self {
        Self { name, labels }
    }
}

impl GatewayMetrics {
    fn increment(&mut self, key: MetricKey) {
        *self.counters.entry(key).or_default() += 1;
    }

    fn add(&mut self, key: MetricKey, value: u64) {
        *self.counters.entry(key).or_default() += value;
    }

    fn observe_seconds(&mut self, key: MetricKey, seconds: f64) {
        let duration = self.durations.entry(key).or_default();
        duration.count += 1;
        duration.sum_seconds += seconds;
    }

    fn render_prometheus(&self) -> String {
        let mut output = String::new();
        output.push_str("# TYPE provider_gateway_turn_total counter\n");
        output.push_str("# TYPE provider_gateway_turn_duration_seconds summary\n");
        output.push_str("# TYPE provider_gateway_upstream_attempt_total counter\n");
        output.push_str("# TYPE provider_gateway_retry_total counter\n");
        output.push_str("# TYPE provider_gateway_model_switch_total counter\n");
        output.push_str("# TYPE provider_gateway_quota_rejection_total counter\n");
        output.push_str("# TYPE provider_gateway_input_tokens_total counter\n");
        output.push_str("# TYPE provider_gateway_output_tokens_total counter\n");
        output.push_str("# TYPE provider_gateway_idempotency_hit_total counter\n");
        output.push_str("# TYPE provider_gateway_invalid_tool_response_total counter\n");
        output.push_str("# TYPE provider_gateway_circuit_state gauge\n");
        output.push_str("# TYPE provider_gateway_queue_depth gauge\n");
        for (key, value) in &self.counters {
            let _ = writeln!(
                output,
                "{}{} {}",
                key.name,
                prometheus_labels(&key.labels),
                value
            );
        }
        for (key, value) in &self.durations {
            let labels = prometheus_labels(&key.labels);
            let _ = writeln!(output, "{}_count{} {}", key.name, labels, value.count);
            let _ = writeln!(output, "{}_sum{} {}", key.name, labels, value.sum_seconds);
        }
        output
    }
}

fn prometheus_labels(labels: &[(&'static str, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let labels = labels
        .iter()
        .map(|(key, value)| {
            format!(
                "{key}=\"{}\"",
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{labels}}}")
}

impl GatewayService {
    pub fn new(config: GatewayConfig) -> Result<Self> {
        validate_config(&config)?;
        let database_url = config.database_url.as_deref().unwrap_or(":memory:");
        let production_storage =
            database_url.starts_with("postgres://") || database_url.starts_with("postgresql://");
        let cipher = match optional_env_or_file(
            "PROVIDER_GATEWAY_ENCRYPTION_KEY",
            "PROVIDER_GATEWAY_ENCRYPTION_KEY_FILE",
        )? {
            Some(secret) => DataCipher::new(&secret)?,
            None if production_storage => {
                return Err(anyhow!(
                    "PROVIDER_GATEWAY_ENCRYPTION_KEY or PROVIDER_GATEWAY_ENCRYPTION_KEY_FILE is required with PostgreSQL"
                ))
            }
            None => DataCipher::development(),
        };
        let storage = PersistentStore::open(database_url)?;
        let (persisted_resources, persisted_policies) =
            storage.initialize_configuration(&config.resources, &config.policies)?;
        for resource in &persisted_resources {
            validate_model_resource(resource)?;
        }
        for policy in &persisted_policies {
            validate_model_selection_policy(policy, &persisted_resources)?;
        }
        let resources = persisted_resources
            .iter()
            .cloned()
            .map(|resource| (resource.id.clone(), resource))
            .collect();
        Ok(Self {
            inner: Arc::new(GatewayInner {
                gitops_config_file: env::var("PROVIDER_GATEWAY_CONFIG_FILE")
                    .ok()
                    .filter(|path| !path.trim().is_empty())
                    .map(PathBuf::from),
                resources: RwLock::new(resources),
                policies: RwLock::new(persisted_policies),
                config,
                idempotency: Mutex::new(HashMap::new()),
                bulkheads: Mutex::new(HashMap::new()),
                metrics: Mutex::new(GatewayMetrics::default()),
                accepting_turns: AtomicBool::new(true),
                storage: Mutex::new(storage),
                cipher,
                client: Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .context("building provider HTTP client")?,
            }),
        })
    }

    pub async fn execute_turn(
        &self,
        request: GatewayTurnRequest,
    ) -> std::result::Result<GatewayTurnResponse, GatewayApiError> {
        validate_turn_request(&request)?;
        let idempotency_key = format!(
            "{}:{}:{}",
            request.scope.run_id, request.scope.turn, request.idempotency_key
        );
        let request_hash = idempotency_request_hash(&request).map_err(|_| {
            GatewayApiError::new(
                StatusCode::BAD_REQUEST,
                request.request_id.clone(),
                "invalid_turn_request",
                "Turn request cannot be hashed",
                false,
            )
        })?;

        let stored_turn = self
            .inner
            .storage
            .lock()
            .await
            .reserve_turn(&idempotency_key, &request_hash)
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })?;
        match stored_turn {
            StoredTurn::Reserved => {}
            StoredTurn::Completed(encrypted_response) => {
                self.record_idempotency_hit("completed").await;
                return self.decrypt_turn_response(&request.request_id, &encrypted_response);
            }
            StoredTurn::Expired => {
                return Err(GatewayApiError::new(
                    StatusCode::GONE,
                    request.request_id.clone(),
                    "idempotency_result_expired",
                    "The protected idempotency replay window has expired",
                    false,
                ));
            }
            StoredTurn::Failed { status, error } => {
                self.record_idempotency_hit("failed").await;
                return Err(GatewayApiError {
                    status: StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                    envelope: Box::new(error),
                });
            }
            StoredTurn::Conflict => {
                return Err(GatewayApiError::new(
                    StatusCode::CONFLICT,
                    request.request_id.clone(),
                    "idempotency_conflict",
                    "The idempotency key was already used with a different request",
                    false,
                ));
            }
            StoredTurn::InProgress => {
                self.record_idempotency_hit("in_progress").await;
                let completion = self
                    .inner
                    .idempotency
                    .lock()
                    .await
                    .get(&idempotency_key)
                    .and_then(|entry| match entry {
                        IdempotencyEntry::InProgress {
                            request_hash: saved_hash,
                            completion,
                        } if saved_hash == &request_hash => Some(completion.subscribe()),
                        _ => None,
                    });
                if let Some(mut completion) = completion {
                    if !*completion.borrow() {
                        let _ = completion.changed().await;
                    }
                    return Box::pin(self.execute_turn(request)).await;
                }
                return self
                    .wait_for_durable_turn(&idempotency_key, &request_hash, &request)
                    .await;
            }
        }

        loop {
            let wait_for = {
                let mut entries = self.inner.idempotency.lock().await;
                match entries.get(&idempotency_key) {
                    Some(IdempotencyEntry::Completed {
                        request_hash: saved_hash,
                        response,
                    }) if saved_hash == &request_hash => return Ok((**response).clone()),
                    Some(IdempotencyEntry::Failed {
                        request_hash: saved_hash,
                        error,
                    }) if saved_hash == &request_hash => return Err(error.clone()),
                    Some(IdempotencyEntry::InProgress {
                        request_hash: saved_hash,
                        completion,
                    }) if saved_hash == &request_hash => Some(completion.subscribe()),
                    Some(_) => {
                        return Err(GatewayApiError::new(
                            StatusCode::CONFLICT,
                            request.request_id.clone(),
                            "idempotency_conflict",
                            "The idempotency key was already used with a different request",
                            false,
                        ));
                    }
                    None => {
                        let (completion, _) = watch::channel(false);
                        entries.insert(
                            idempotency_key.clone(),
                            IdempotencyEntry::InProgress {
                                request_hash: request_hash.clone(),
                                completion,
                            },
                        );
                        None
                    }
                }
            };
            if let Some(mut completion) = wait_for {
                if !*completion.borrow() {
                    let _ = completion.changed().await;
                }
                continue;
            }
            break;
        }

        let mut result = self.execute_new_turn(&request).await;
        let persistence = match &result {
            Ok(response) => match self.encrypt_turn_response(response) {
                Ok(encrypted_response) => self.inner.storage.lock().await.complete_turn(
                    &idempotency_key,
                    &encrypted_response,
                    response,
                    &request.scope.run_id,
                    request.scope.turn,
                ),
                Err(error) => Err(error),
            },
            Err(error) => self.inner.storage.lock().await.fail_turn(
                &idempotency_key,
                error.status.as_u16(),
                &error.envelope,
            ),
        };
        if persistence.is_err() {
            result = Err(GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                request.request_id.clone(),
                "gateway_storage_unavailable",
                "Gateway durable state is temporarily unavailable",
                true,
            ));
        }
        let completion = {
            let mut entries = self.inner.idempotency.lock().await;
            let completion = match entries.remove(&idempotency_key) {
                Some(IdempotencyEntry::InProgress { completion, .. }) => completion,
                _ => {
                    let (completion, _) = watch::channel(false);
                    completion
                }
            };
            let entry = match &result {
                Ok(response) => IdempotencyEntry::Completed {
                    request_hash,
                    response: Box::new(response.clone()),
                },
                Err(error) => IdempotencyEntry::Failed {
                    request_hash,
                    error: error.clone(),
                },
            };
            entries.insert(idempotency_key, entry);
            completion
        };
        let _ = completion.send(true);
        result
    }

    fn encrypt_turn_response(&self, response: &GatewayTurnResponse) -> Result<String> {
        self.inner
            .cipher
            .encrypt(&serde_json::to_vec(response).context("serializing turn response")?)
    }

    fn decrypt_turn_response(
        &self,
        request_id: &str,
        encrypted_response: &str,
    ) -> std::result::Result<GatewayTurnResponse, GatewayApiError> {
        self.inner
            .cipher
            .decrypt(encrypted_response)
            .and_then(|plaintext| {
                serde_json::from_slice(&plaintext).context("decoding protected turn response")
            })
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request_id,
                    "gateway_storage_unavailable",
                    "Gateway protected replay state is unavailable",
                    true,
                )
            })
    }

    async fn wait_for_durable_turn(
        &self,
        idempotency_key: &str,
        request_hash: &str,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<GatewayTurnResponse, GatewayApiError> {
        loop {
            let remaining = remaining_turn_deadline(request)?;
            if remaining <= Duration::from_millis(50) {
                return Err(GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "provider_unavailable",
                    "A previous upstream attempt remains uncertain and will not be replayed",
                    true,
                ));
            }
            tokio::time::sleep(Duration::from_millis(50).min(remaining)).await;
            let stored = self
                .inner
                .storage
                .lock()
                .await
                .reserve_turn(idempotency_key, request_hash)
                .map_err(|_| {
                    GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "gateway_storage_unavailable",
                        "Gateway durable state is temporarily unavailable",
                        true,
                    )
                })?;
            match stored {
                StoredTurn::Completed(encrypted_response) => {
                    return self.decrypt_turn_response(&request.request_id, &encrypted_response);
                }
                StoredTurn::Failed { status, error } => {
                    return Err(GatewayApiError {
                        status: StatusCode::from_u16(status)
                            .unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                        envelope: Box::new(error),
                    });
                }
                StoredTurn::Expired => {
                    return Err(GatewayApiError::new(
                        StatusCode::GONE,
                        request.request_id.clone(),
                        "idempotency_result_expired",
                        "The protected idempotency replay window has expired",
                        false,
                    ));
                }
                StoredTurn::Conflict => {
                    return Err(GatewayApiError::new(
                        StatusCode::CONFLICT,
                        request.request_id.clone(),
                        "idempotency_conflict",
                        "The idempotency key was already used with a different request",
                        false,
                    ));
                }
                StoredTurn::InProgress => {}
                StoredTurn::Reserved => {
                    return Err(GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "gateway_storage_unavailable",
                        "Gateway idempotency state changed unexpectedly",
                        true,
                    ));
                }
            }
        }
    }

    /// Begin a controlled shutdown. Health checks fail immediately and HTTP
    /// handlers reject new turns, while already accepted handlers may finish.
    pub fn begin_shutdown(&self) {
        self.inner.accepting_turns.store(false, Ordering::Release);
    }

    fn is_accepting_turns(&self) -> bool {
        self.inner.accepting_turns.load(Ordering::Acquire)
    }

    fn read_gitops_config_file(&self) -> Result<(GatewayConfig, String)> {
        let path = self
            .inner
            .gitops_config_file
            .as_ref()
            .ok_or_else(|| anyhow!("Provider Gateway has no configured GitOps config file"))?;
        let contents = fs::read_to_string(path).with_context(|| {
            format!("reading Provider Gateway GitOps config {}", path.display())
        })?;
        let config: GatewayConfig = serde_json::from_str(&contents).with_context(|| {
            format!("parsing Provider Gateway GitOps config {}", path.display())
        })?;
        validate_config(&config)?;
        let digest = format!("{:x}", Sha256::digest(contents.as_bytes()));
        Ok((config, digest))
    }

    async fn reconcile_gitops_config(
        &self,
        config: GatewayConfig,
        config_digest: String,
    ) -> Result<GitopsReconcileReport> {
        let mut changed_model_resources = Vec::new();
        let mut changed_model_selection_policies = Vec::new();
        for desired in config.resources {
            let current = self
                .inner
                .storage
                .lock()
                .await
                .model_resource(&desired.id, None)?;
            if current
                .as_ref()
                .is_some_and(|current| same_revisioned_configuration(current, &desired))
            {
                continue;
            }
            let expected_revision = current.as_ref().map(|resource| resource.revision);
            let saved = self
                .inner
                .storage
                .lock()
                .await
                .save_model_resource(desired, expected_revision)?;
            changed_model_resources.push(format!("{}@{}", saved.id, saved.revision));
        }
        for desired in config.policies {
            let current = self
                .inner
                .storage
                .lock()
                .await
                .model_selection_policy(&desired.id, None)?;
            if current
                .as_ref()
                .is_some_and(|current| same_revisioned_configuration(current, &desired))
            {
                continue;
            }
            let expected_revision = current.as_ref().map(|policy| policy.revision);
            let saved = self
                .inner
                .storage
                .lock()
                .await
                .save_model_selection_policy(desired, expected_revision)?;
            changed_model_selection_policies.push(format!("{}@{}", saved.id, saved.revision));
        }
        refresh_configuration(self).await?;
        self.inner.storage.lock().await.audit_event(
            "gitops_configuration.reconciled",
            "provider-gateway",
            &json!({
                "configDigest": &config_digest,
                "changedModelResources": &changed_model_resources,
                "changedModelSelectionPolicies": &changed_model_selection_policies,
            }),
        )?;
        Ok(GitopsReconcileReport {
            config_digest,
            changed_model_resources,
            changed_model_selection_policies,
        })
    }

    async fn execute_new_turn(
        &self,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<GatewayTurnResponse, GatewayApiError> {
        let policy = self.resolve_policy(&request.scope).await.ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::FORBIDDEN,
                request.request_id.clone(),
                "model_resource_not_allowed",
                "No active model selection policy allows this scope",
                false,
            )
        })?;

        let resources = self.inner.resources.read().await;
        let (candidates, selection_reason) =
            if let Some(resource_id) = &request.routing.model_resource_id {
                if !policy
                    .direct_selection
                    .allowed_model_resource_ids
                    .iter()
                    .any(|allowed| allowed == resource_id)
                {
                    return Err(GatewayApiError::new(
                    StatusCode::FORBIDDEN,
                    request.request_id.clone(),
                    "model_resource_not_allowed",
                    "The requested model resource is not allowed by the active selection policy",
                    false,
                ));
                }
                let resource = resources.get(resource_id).cloned().ok_or_else(|| {
                    GatewayApiError::new(
                        StatusCode::FORBIDDEN,
                        request.request_id.clone(),
                        "model_resource_not_allowed",
                        "The requested model resource does not exist",
                        false,
                    )
                })?;
                (vec![resource], "explicit_resource".to_string())
            } else {
                (
                    ordered_automatic_candidates(&policy, &resources, &request.idempotency_key),
                    "automatic_selection".to_string(),
                )
            };
        drop(resources);

        let capability_compatible = candidates
            .into_iter()
            .filter(|resource| {
                resource.enabled
                    && capabilities_match(resource, &request.routing.required_capabilities)
            })
            .collect::<Vec<_>>();
        if capability_compatible.is_empty() {
            return Err(GatewayApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                request.request_id.clone(),
                "provider_capability_mismatch",
                "No allowed model resource satisfies the required capabilities",
                false,
            ));
        }
        let mut compatible = Vec::new();
        for resource in capability_compatible {
            if self.resource_circuit_allows(&resource, request).await? {
                compatible.push(resource);
            }
        }
        if compatible.is_empty() {
            return Err(GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                request.request_id.clone(),
                "provider_unavailable",
                "Every allowed model resource is temporarily unavailable",
                true,
            ));
        }
        let quota_lease = self.acquire_policy_quota(&policy, request).await?;

        let mut prior_resource_id = None;
        let mut last_error = None;
        for (index, resource) in compatible.iter().enumerate() {
            let call_result = match self.acquire_resource_bulkhead(resource, request).await {
                Ok(_permit) => self.call_resource(resource, request).await,
                Err(error) => Err(error),
            };
            match call_result {
                Ok(mut provider_response) => {
                    if let Err(error) = self.record_resource_success(resource, request).await {
                        self.release_policy_quota(&quota_lease, request).await?;
                        return Err(error);
                    }
                    self.settle_policy_quota(&quota_lease, &provider_response.usage, request)
                        .await?;
                    provider_response.attempt_count += index as u32;
                    let automatic_switch = AutomaticSwitchSummary {
                        used: index > 0,
                        reason: if index > 0 {
                            last_error
                                .as_ref()
                                .map(|error: &GatewayApiError| error.code().to_string())
                        } else {
                            None
                        },
                        from_model_resource_id: prior_resource_id.clone(),
                    };
                    return Ok(build_turn_response(
                        request,
                        &policy,
                        resource,
                        &selection_reason,
                        automatic_switch,
                        provider_response,
                    ));
                }
                Err(error) => {
                    if error.is_retryable_upstream() {
                        if let Err(storage_error) = self
                            .record_resource_retryable_failure(resource, request)
                            .await
                        {
                            self.release_policy_quota(&quota_lease, request).await?;
                            return Err(storage_error);
                        }
                    }
                    let may_switch = policy.automatic_switch.enabled
                        && index + 1 < compatible.len()
                        && (index as u32) < policy.automatic_switch.max_model_switches_per_turn
                        && policy
                            .automatic_switch
                            .allowed_reasons
                            .iter()
                            .any(|reason| reason == error.code());
                    if may_switch {
                        prior_resource_id = Some(resource.id.clone());
                        last_error = Some(error);
                        continue;
                    }
                    self.release_policy_quota(&quota_lease, request).await?;
                    return Err(error);
                }
            }
        }
        self.release_policy_quota(&quota_lease, request).await?;
        Err(last_error.unwrap_or_else(|| {
            GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                request.request_id.clone(),
                "provider_unavailable",
                "No selected provider could complete the turn",
                true,
            )
        }))
    }

    async fn resource_circuit_allows(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<bool, GatewayApiError> {
        self.inner
            .storage
            .lock()
            .await
            .resource_circuit_allows(&resource.id, resource.revision, Utc::now().timestamp())
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })
    }

    async fn acquire_resource_bulkhead(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<OwnedSemaphorePermit, GatewayApiError> {
        let key = format!("{}:{}", resource.id, resource.revision);
        let semaphore = {
            let mut bulkheads = self.inner.bulkheads.lock().await;
            bulkheads
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(Semaphore::new(resource.defaults.max_concurrent_requests))
                })
                .clone()
        };
        semaphore.try_acquire_owned().map_err(|_| {
            GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                request.request_id.clone(),
                "gateway_overloaded",
                "The selected model resource is at concurrent request capacity",
                true,
            )
            .with_retry_after_ms(250)
        })
    }

    async fn acquire_policy_quota(
        &self,
        policy: &ModelSelectionPolicy,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<PolicyQuotaLease, GatewayApiError> {
        let period_utc = Utc::now().format("%Y-%m-%d").to_string();
        let estimated_input_tokens = estimate_input_tokens(&request.input);
        let reserved_input_tokens = if let Some(limit) = policy.limits.daily_input_tokens {
            let allowed = self
                .inner
                .storage
                .lock()
                .await
                .reserve_project_daily_input_tokens(
                    &period_utc,
                    &request.scope.workspace_id,
                    &request.scope.project_id,
                    estimated_input_tokens,
                    limit,
                )
                .map_err(|_| {
                    GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "gateway_storage_unavailable",
                        "Gateway durable state is temporarily unavailable",
                        true,
                    )
                })?;
            if !allowed {
                return Err(GatewayApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    request.request_id.clone(),
                    "gateway_quota_exceeded",
                    "The project daily input token quota is exhausted",
                    false,
                ));
            }
            estimated_input_tokens
        } else {
            0
        };
        let concurrency_lease_id = if let Some(limit) = policy.limits.max_concurrent_turns {
            if limit == 0 {
                self.release_policy_quota_reservation(
                    &period_utc,
                    &request.scope.workspace_id,
                    &request.scope.project_id,
                    reserved_input_tokens,
                    request,
                )
                .await?;
                return Err(GatewayApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    request.request_id.clone(),
                    "gateway_quota_exceeded",
                    "The project is not allowed to start model turns",
                    false,
                ));
            }
            let key = format!(
                "{}:{}:{}:{}",
                policy.id, policy.revision, request.scope.workspace_id, request.scope.project_id
            );
            let lease_id = format!(
                "{}:{}:{}",
                request.scope.run_id, request.scope.turn, request.idempotency_key
            );
            let expires_at = DateTime::parse_from_rfc3339(&request.deadline_at)
                .map(|deadline| deadline.timestamp().saturating_add(60))
                .unwrap_or_else(|_| Utc::now().timestamp().saturating_add(240));
            let acquired = self
                .inner
                .storage
                .lock()
                .await
                .acquire_project_concurrency_lease(&key, &lease_id, limit, expires_at)
                .map_err(|_| {
                    GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "gateway_storage_unavailable",
                        "Gateway durable state is temporarily unavailable",
                        true,
                    )
                })?;
            if !acquired {
                self.release_policy_quota_reservation(
                    &period_utc,
                    &request.scope.workspace_id,
                    &request.scope.project_id,
                    reserved_input_tokens,
                    request,
                )
                .await?;
                return Err(GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_overloaded",
                    "The project is at concurrent turn capacity",
                    true,
                )
                .with_retry_after_ms(250));
            }
            Some(lease_id)
        } else {
            None
        };
        Ok(PolicyQuotaLease {
            period_utc,
            workspace_id: request.scope.workspace_id.clone(),
            project_id: request.scope.project_id.clone(),
            reserved_input_tokens,
            concurrency_lease_id,
        })
    }

    async fn settle_policy_quota(
        &self,
        lease: &PolicyQuotaLease,
        usage: &Usage,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        if lease.reserved_input_tokens != 0 {
            self.inner
                .storage
                .lock()
                .await
                .settle_project_daily_usage(
                    &lease.period_utc,
                    &lease.workspace_id,
                    &lease.project_id,
                    lease.reserved_input_tokens,
                    usage.input_tokens,
                )
                .map_err(|_| {
                    GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "gateway_storage_unavailable",
                        "Gateway durable state is temporarily unavailable",
                        true,
                    )
                })?;
        }
        self.release_concurrency_lease(lease, request).await
    }

    async fn release_concurrency_lease(
        &self,
        lease: &PolicyQuotaLease,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        let Some(lease_id) = &lease.concurrency_lease_id else {
            return Ok(());
        };
        self.inner
            .storage
            .lock()
            .await
            .release_project_concurrency_lease(lease_id)
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })
    }

    async fn release_policy_quota_reservation(
        &self,
        period_utc: &str,
        workspace_id: &str,
        project_id: &str,
        reserved_input_tokens: u64,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        if reserved_input_tokens == 0 {
            return Ok(());
        }
        self.inner
            .storage
            .lock()
            .await
            .settle_project_daily_usage(
                period_utc,
                workspace_id,
                project_id,
                reserved_input_tokens,
                0,
            )
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })
    }

    async fn release_policy_quota(
        &self,
        lease: &PolicyQuotaLease,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        self.release_policy_quota_reservation(
            &lease.period_utc,
            &lease.workspace_id,
            &lease.project_id,
            lease.reserved_input_tokens,
            request,
        )
        .await?;
        self.release_concurrency_lease(lease, request).await
    }

    async fn record_resource_success(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        self.inner
            .storage
            .lock()
            .await
            .record_resource_success(&resource.id, resource.revision)
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })
    }

    async fn record_resource_retryable_failure(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<(), GatewayApiError> {
        self.inner
            .storage
            .lock()
            .await
            .record_resource_retryable_failure(
                &resource.id,
                resource.revision,
                Utc::now().timestamp(),
            )
            .map(|_| ())
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })
    }

    async fn resolve_policy(&self, scope: &TurnScope) -> Option<ModelSelectionPolicy> {
        let policies = self.inner.policies.read().await;
        policies
            .iter()
            .filter(|policy| policy_matches_turn(policy, scope))
            .max_by_key(|policy| {
                let project_specific = policy
                    .scope
                    .project_ids
                    .iter()
                    .any(|project_id| project_id == &scope.project_id)
                    as u8;
                let workspace_specific = policy
                    .scope
                    .workspace_ids
                    .iter()
                    .any(|workspace_id| workspace_id == &scope.workspace_id)
                    as u8;
                let phase_specific = policy
                    .applies_to
                    .phases
                    .iter()
                    .any(|phase| phase == &scope.phase) as u8;
                let profile_specific = policy
                    .applies_to
                    .agent_profiles
                    .iter()
                    .any(|profile| profile == &scope.agent_profile)
                    as u8;
                (
                    project_specific,
                    workspace_specific,
                    phase_specific,
                    profile_specific,
                    policy.revision,
                )
            })
            .cloned()
    }

    async fn call_resource(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<ProviderTurn, GatewayApiError> {
        self.adapter_for(resource)
            .execute_turn(self, resource, request)
            .await
    }

    fn adapter_for(&self, resource: &ModelResource) -> &'static dyn ProviderAdapter {
        let adapter: &'static dyn ProviderAdapter = match resource.kind {
            ModelResourceKind::OpenaiCompatible => &OPENAI_COMPATIBLE_ADAPTER,
            ModelResourceKind::Fixture => &FIXTURE_ADAPTER,
        };
        debug_assert_eq!(adapter.kind(), resource.kind);
        adapter
    }

    async fn call_openai_compatible(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
    ) -> std::result::Result<ProviderTurn, GatewayApiError> {
        if resource.auth.auth_type != "bearer" {
            return Err(GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                request.request_id.clone(),
                "provider_unavailable",
                "The selected model resource has unsupported authentication",
                false,
            ));
        }
        let api_key = self
            .resolve_secret_ref(&resource.auth.secret_ref)
            .await
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "provider_unavailable",
                    "The selected model credential is unavailable",
                    true,
                )
            })?;
        validate_resource_endpoint_dns(&resource.endpoint.base_url)
            .await
            .map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "provider_unavailable",
                    "The selected provider endpoint cannot be reached through the approved network policy",
                    true,
                )
            })?;
        for attempt in 1..=resource.defaults.max_attempts {
            match self
                .call_openai_compatible_once(resource, request, &api_key)
                .await
            {
                Ok(mut provider_turn) => {
                    self.record_upstream_attempt(
                        resource,
                        if attempt == 1 { "initial" } else { "retry" },
                        "success",
                    )
                    .await;
                    provider_turn.attempt_count = attempt;
                    return Ok(provider_turn);
                }
                Err(error)
                    if error.is_retryable_upstream()
                        && attempt < resource.defaults.max_attempts =>
                {
                    self.record_provider_error(resource, request, attempt, &error)
                        .await;
                    self.record_upstream_attempt(
                        resource,
                        if attempt == 1 { "initial" } else { "retry" },
                        error.code(),
                    )
                    .await;
                    self.record_retry(resource, error.code()).await;
                    let delay = provider_retry_delay(request, attempt, &error);
                    if remaining_turn_deadline(request)? <= delay {
                        return Err(GatewayApiError::new(
                            StatusCode::GATEWAY_TIMEOUT,
                            request.request_id.clone(),
                            "provider_timeout",
                            "Turn request deadline elapsed before a Provider retry",
                            true,
                        ));
                    }
                    tokio::time::sleep(delay).await;
                }
                Err(error) => {
                    self.record_provider_error(resource, request, attempt, &error)
                        .await;
                    self.record_upstream_attempt(
                        resource,
                        if attempt == 1 { "initial" } else { "retry" },
                        error.code(),
                    )
                    .await;
                    return Err(error);
                }
            }
        }
        unreachable!("model resource maxAttempts is validated as at least one")
    }

    async fn record_upstream_attempt(&self, resource: &ModelResource, reason: &str, status: &str) {
        self.inner.metrics.lock().await.increment(MetricKey::new(
            "provider_gateway_upstream_attempt_total",
            vec![
                ("provider", provider_metric_label(resource)),
                ("reason", reason.to_string()),
                ("status", status.to_string()),
            ],
        ));
    }

    async fn record_retry(&self, resource: &ModelResource, reason: &str) {
        self.inner.metrics.lock().await.increment(MetricKey::new(
            "provider_gateway_retry_total",
            vec![
                ("provider", provider_metric_label(resource)),
                ("reason", reason.to_string()),
            ],
        ));
    }

    async fn record_provider_error(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
        attempt: u32,
        error: &GatewayApiError,
    ) {
        let _ = self.inner.storage.lock().await.audit_event(
            "provider_attempt.failed",
            &resource.id,
            &json!({
                "requestId": request.request_id,
                "runId": request.scope.run_id,
                "phase": request.scope.phase,
                "attempt": attempt,
                "code": error.envelope.error.code,
                "message": error.envelope.error.message,
                "providerRequestId": error.envelope.error.provider_request_id,
                "retryable": error.envelope.error.retryable,
            }),
        );
    }

    async fn call_openai_compatible_once(
        &self,
        resource: &ModelResource,
        request: &GatewayTurnRequest,
        api_key: &str,
    ) -> std::result::Result<ProviderTurn, GatewayApiError> {
        let endpoint = format!(
            "{}{}",
            resource.endpoint.base_url.trim_end_matches('/'),
            ensure_leading_slash(&resource.endpoint.chat_completions_path)
        );
        let tool_aliases = ProviderToolAliasMap::from_request(request)?;
        let body = openai_request_body(request, resource, &tool_aliases)?;
        // Runtime's deadline is the authoritative bound for a logical turn.
        // A resource may tighten it, but may never extend it.
        let timeout = provider_call_timeout(resource, request)?;
        let response = self
            .inner
            .client
            .post(&endpoint)
            .timeout(timeout)
            .bearer_auth(api_key)
            .header("x-request-id", &request.request_id)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    GatewayApiError::new(
                        StatusCode::GATEWAY_TIMEOUT,
                        request.request_id.clone(),
                        "provider_timeout",
                        "Selected provider did not respond before the turn deadline",
                        true,
                    )
                } else {
                    GatewayApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        request.request_id.clone(),
                        "provider_unavailable",
                        "Selected provider is temporarily unavailable",
                        true,
                    )
                }
            })?;
        let status = response.status();
        let retry_after_ms = provider_retry_after_ms(response.headers());
        let provider_request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        if !status.is_success() {
            let rejection_detail = response
                .text()
                .await
                .ok()
                .and_then(|body| provider_rejection_detail(&body));
            let error = if status.as_u16() == 429 {
                GatewayApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    request.request_id.clone(),
                    "provider_rate_limited",
                    "Selected provider is temporarily rate limited",
                    true,
                )
            } else if status.is_server_error() {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    request.request_id.clone(),
                    "provider_unavailable",
                    "Selected provider is temporarily unavailable",
                    true,
                )
            } else {
                GatewayApiError::new(
                    StatusCode::BAD_GATEWAY,
                    request.request_id.clone(),
                    "provider_response_invalid",
                    rejection_detail
                        .map(|detail| {
                            format!("Selected provider rejected the normalized request: {detail}")
                        })
                        .unwrap_or_else(|| {
                            "Selected provider rejected the normalized request".to_string()
                        }),
                    false,
                )
            };
            let error = if let Some(retry_after_ms) = retry_after_ms {
                error.with_retry_after_ms(retry_after_ms)
            } else {
                error
            };
            return Err(error.with_provider_request_id(provider_request_id));
        }
        let value = response.json::<Value>().await.map_err(|_| {
            GatewayApiError::new(
                StatusCode::BAD_GATEWAY,
                request.request_id.clone(),
                "provider_response_invalid",
                "Selected provider returned an invalid response",
                true,
            )
            .with_provider_request_id(provider_request_id.clone())
        })?;
        let provider_request_id = provider_request_id.or_else(|| {
            value
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
        });
        parse_openai_response(value, provider_request_id, request, &tool_aliases)
    }
}

impl GatewayService {
    async fn resolve_secret_ref(&self, secret_ref: &str) -> Result<String> {
        if let Some(name) = secret_ref.strip_prefix("db:") {
            validate_database_secret_name(name)?;
            let ciphertext = self
                .inner
                .storage
                .lock()
                .await
                .encrypted_secret(name)?
                .ok_or_else(|| anyhow!("encrypted secret {name} does not exist"))?;
            let plaintext = self.inner.cipher.decrypt(&ciphertext)?;
            return String::from_utf8(plaintext).context("decoding encrypted Provider secret");
        }
        resolve_local_secret_ref(secret_ref)
    }

    async fn write_secret_ref(&self, secret_ref: &str, api_key: &str) -> Result<()> {
        if api_key.trim().is_empty() {
            return Err(anyhow!("API key must not be empty"));
        }
        if let Some(name) = secret_ref.strip_prefix("db:") {
            validate_database_secret_name(name)?;
            let ciphertext = self.inner.cipher.encrypt(api_key.as_bytes())?;
            return self
                .inner
                .storage
                .lock()
                .await
                .save_encrypted_secret(name, &ciphertext);
        }
        write_file_secret_ref(secret_ref, api_key)
    }

    async fn record_idempotency_hit(&self, state: &str) {
        self.inner.metrics.lock().await.increment(MetricKey::new(
            "provider_gateway_idempotency_hit_total",
            vec![("state", state.to_string())],
        ));
    }

    async fn record_turn_metric(
        &self,
        request: &GatewayTurnRequest,
        result: &std::result::Result<GatewayTurnResponse, GatewayApiError>,
        elapsed: Duration,
    ) {
        let mut metrics = self.inner.metrics.lock().await;
        match result {
            Ok(response) => {
                let provider = response.model_execution.provider_id.clone();
                let model_alias = response.model_execution.model_resource_id.clone();
                metrics.increment(MetricKey::new(
                    "provider_gateway_turn_total",
                    vec![
                        ("provider", provider.clone()),
                        ("model_alias", model_alias.clone()),
                        ("phase", request.scope.phase.clone()),
                        ("status", "success".to_string()),
                    ],
                ));
                metrics.observe_seconds(
                    MetricKey::new(
                        "provider_gateway_turn_duration_seconds",
                        vec![
                            ("provider", provider.clone()),
                            ("model_alias", model_alias.clone()),
                            ("phase", request.scope.phase.clone()),
                        ],
                    ),
                    elapsed.as_secs_f64(),
                );
                metrics.add(
                    MetricKey::new(
                        "provider_gateway_input_tokens_total",
                        vec![
                            ("provider", provider.clone()),
                            ("model_alias", model_alias.clone()),
                        ],
                    ),
                    response.usage.input_tokens,
                );
                metrics.add(
                    MetricKey::new(
                        "provider_gateway_output_tokens_total",
                        vec![
                            ("provider", provider.clone()),
                            ("model_alias", model_alias.clone()),
                        ],
                    ),
                    response.usage.output_tokens,
                );
                if response.model_execution.automatic_switch.used {
                    metrics.increment(MetricKey::new(
                        "provider_gateway_model_switch_total",
                        vec![
                            (
                                "from_model_resource",
                                response
                                    .model_execution
                                    .automatic_switch
                                    .from_model_resource_id
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                            ),
                            (
                                "to_model_resource",
                                response.model_execution.model_resource_id.clone(),
                            ),
                            (
                                "reason",
                                response
                                    .model_execution
                                    .automatic_switch
                                    .reason
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                            ),
                        ],
                    ));
                }
            }
            Err(error) => {
                metrics.increment(MetricKey::new(
                    "provider_gateway_turn_total",
                    vec![
                        ("provider", "unresolved".to_string()),
                        ("model_alias", "unresolved".to_string()),
                        ("phase", request.scope.phase.clone()),
                        ("status", error.code().to_string()),
                    ],
                ));
                if error.code() == "gateway_quota_exceeded" {
                    metrics.increment(MetricKey::new(
                        "provider_gateway_quota_rejection_total",
                        vec![
                            ("scope_type", "project".to_string()),
                            ("reason", "limit_exhausted".to_string()),
                        ],
                    ));
                }
                if error.code() == "provider_response_invalid" {
                    metrics.increment(MetricKey::new(
                        "provider_gateway_invalid_tool_response_total",
                        vec![
                            ("provider", "unresolved".to_string()),
                            ("reason", "invalid_response".to_string()),
                        ],
                    ));
                }
            }
        }
    }
}

fn provider_metric_label(resource: &ModelResource) -> String {
    match resource.kind {
        ModelResourceKind::OpenaiCompatible => "openai_compatible".to_string(),
        ModelResourceKind::Fixture => "fixture".to_string(),
    }
}

pub fn router(service: GatewayService) -> Router {
    Router::new()
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(ready_handler))
        .route("/metrics", get(metrics_handler))
        .route("/v1/agent/turn", post(turn_handler))
        .route(
            "/internal/provider-gateway/admin/v1/model-resources",
            get(list_model_resources).post(create_model_resource),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-resources/{id}",
            get(get_model_resource),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-resources/{id}/validate",
            post(validate_model_resource_endpoint),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-resources/{id}/readiness",
            post(readiness_model_resource),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-resources/{id}/enable",
            post(enable_model_resource),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-resources/{id}/disable",
            post(disable_model_resource),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-selection-policies",
            get(list_model_selection_policies).post(create_model_selection_policy),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-selection-policies/{id}",
            get(get_model_selection_policy),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-selection-policies/{id}/activate",
            post(activate_model_selection_policy),
        )
        .route(
            "/internal/provider-gateway/admin/v1/model-executions",
            get(list_model_executions),
        )
        .route(
            "/internal/provider-gateway/admin/v1/audit-events",
            get(list_audit_events),
        )
        .route(
            "/internal/provider-gateway/admin/v1/configuration/reconcile",
            post(reconcile_gitops_configuration),
        )
        .layer(DefaultBodyLimit::max(MAX_TURN_BODY_BYTES))
        .with_state(service)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminModelResourceWrite {
    #[serde(flatten)]
    resource: ModelResource,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminModelSelectionPolicyWrite {
    #[serde(flatten)]
    policy: ModelSelectionPolicy,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminRevisionRequest {
    expected_revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminPolicyActivationRequest {
    expected_revision: u64,
    revision_to_activate: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitopsReconcileRequest {
    config_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminModelResourceView {
    schema_version: String,
    id: String,
    display_name: String,
    kind: ModelResourceKind,
    enabled: bool,
    revision: u64,
    endpoint: ProviderEndpoint,
    auth: AdminModelResourceAuthView,
    physical_model: String,
    capabilities: ProviderCapabilities,
    defaults: ModelDefaults,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminModelResourceAuthView {
    #[serde(rename = "type")]
    auth_type: String,
    secret_configured: bool,
}

impl From<&ModelResource> for AdminModelResourceView {
    fn from(resource: &ModelResource) -> Self {
        Self {
            schema_version: resource.schema_version.clone(),
            id: resource.id.clone(),
            display_name: resource.display_name.clone(),
            kind: resource.kind.clone(),
            enabled: resource.enabled,
            revision: resource.revision,
            endpoint: resource.endpoint.clone(),
            auth: AdminModelResourceAuthView {
                auth_type: resource.auth.auth_type.clone(),
                secret_configured: !resource.auth.secret_ref.trim().is_empty(),
            },
            physical_model: resource.physical_model.clone(),
            capabilities: resource.capabilities.clone(),
            defaults: resource.defaults.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct AdminChangeContext {
    operator_id: String,
    reason: String,
    change_reference: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionQuery {
    run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditEventsQuery {
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default)]
    before_id: Option<i64>,
    #[serde(default)]
    limit: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminAuditEventView {
    id: i64,
    event_type: String,
    subject_id: String,
    metadata: Value,
    created_at: String,
}

impl From<AuditEventRecord> for AdminAuditEventView {
    fn from(event: AuditEventRecord) -> Self {
        Self {
            id: event.id,
            event_type: event.event_type,
            subject_id: event.subject_id,
            metadata: redact_audit_metadata(event.metadata),
            created_at: event.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevisionQuery {
    #[serde(default)]
    revision: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelResourceValidationResponse {
    model_resource_id: String,
    revision: u64,
    valid: bool,
    checks: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelResourceReadinessResponse {
    model_resource_id: String,
    revision: u64,
    ready: bool,
    provider_request_id: Option<String>,
    usage: Usage,
}

enum AdminWriteReservation<T> {
    Reserved { key: String },
    Completed(T),
}

async fn reserve_admin_write<T: DeserializeOwned>(
    service: &GatewayService,
    headers: &HeaderMap,
    operation: &str,
    payload: &impl Serialize,
) -> std::result::Result<AdminWriteReservation<T>, GatewayApiError> {
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::BAD_REQUEST,
                "admin",
                "idempotency_key_required",
                "Admin write operations require Idempotency-Key",
                true,
            )
        })?;
    let request_hash = sha256_json(payload).map_err(|_| {
        GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            "admin",
            "invalid_admin_request",
            "Admin request cannot be hashed",
            true,
        )
    })?;
    let key = format!("{operation}:{idempotency_key}");
    match service
        .inner
        .storage
        .lock()
        .await
        .reserve_admin_operation(&key, &request_hash)
        .map_err(admin_storage_error)?
    {
        StoredAdminOperation::Reserved => Ok(AdminWriteReservation::Reserved { key }),
        StoredAdminOperation::Completed(response) => {
            let response = serde_json::from_str(&response).map_err(|_| {
                GatewayApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "admin",
                    "gateway_storage_unavailable",
                    "Gateway durable state is temporarily unavailable",
                    true,
                )
            })?;
            Ok(AdminWriteReservation::Completed(response))
        }
        StoredAdminOperation::Conflict => Err(GatewayApiError::new(
            StatusCode::CONFLICT,
            "admin",
            "idempotency_conflict",
            "The idempotency key was already used for a different admin request",
            false,
        )),
        StoredAdminOperation::InProgress => Err(GatewayApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "admin",
            "admin_operation_in_progress",
            "The same admin operation is still in progress",
            true,
        )),
    }
}

async fn complete_admin_write(
    service: &GatewayService,
    key: &str,
    response: &impl Serialize,
) -> std::result::Result<(), GatewayApiError> {
    let response_json = serde_json::to_string(response).map_err(|_| {
        GatewayApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "admin",
            "gateway_storage_unavailable",
            "Gateway durable state is temporarily unavailable",
            true,
        )
    })?;
    service
        .inner
        .storage
        .lock()
        .await
        .complete_admin_operation(key, &response_json)
        .map_err(admin_storage_error)
}

async fn discard_admin_write(service: &GatewayService, key: &str) {
    let _ = service
        .inner
        .storage
        .lock()
        .await
        .discard_admin_operation(key);
}

async fn list_model_resources(
    State(service): State<GatewayService>,
    headers: HeaderMap,
) -> std::result::Result<Json<Vec<AdminModelResourceView>>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    Ok(Json(
        service
            .inner
            .storage
            .lock()
            .await
            .current_model_resources()
            .map_err(admin_storage_error)?
            .iter()
            .map(AdminModelResourceView::from)
            .collect(),
    ))
}

async fn get_model_resource(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> std::result::Result<Json<AdminModelResourceView>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    let resource = service
        .inner
        .storage
        .lock()
        .await
        .model_resource(&id, query.revision)
        .map_err(admin_storage_error)?
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::NOT_FOUND,
                "admin",
                "model_resource_not_found",
                "Model resource does not exist",
                false,
            )
        })?;
    Ok(Json(AdminModelResourceView::from(&resource)))
}

async fn validate_model_resource_endpoint(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> std::result::Result<Json<ModelResourceValidationResponse>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    let resource = service
        .inner
        .storage
        .lock()
        .await
        .model_resource(&id, None)
        .map_err(admin_storage_error)?
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::NOT_FOUND,
                "admin",
                "model_resource_not_found",
                "Model resource does not exist",
                false,
            )
        })?;
    validate_model_resource(&resource).map_err(|_| {
        GatewayApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "admin",
            "invalid_model_resource",
            "Model resource endpoint, schema, or authentication is invalid",
            false,
        )
    })?;
    validate_resource_endpoint_dns(&resource.endpoint.base_url)
        .await
        .map_err(|_| {
            GatewayApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "admin",
                "invalid_model_resource_endpoint",
                "Model resource endpoint does not resolve to an approved network",
                false,
            )
        })?;
    let response = ModelResourceValidationResponse {
        model_resource_id: resource.id.clone(),
        revision: resource.revision,
        valid: true,
        checks: vec!["schema".to_string(), "url".to_string(), "dns".to_string()],
    };
    service
        .inner
        .storage
        .lock()
        .await
        .audit_event(
            "model_resource.validated",
            &resource.id,
            &serde_json::json!({
                "revision": resource.revision,
                "checks": &response.checks,
            }),
        )
        .map_err(admin_storage_error)?;
    Ok(Json(response))
}

async fn readiness_model_resource(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(revision): Json<AdminRevisionRequest>,
) -> std::result::Result<Json<ModelResourceReadinessResponse>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let resource = service
        .inner
        .storage
        .lock()
        .await
        .model_resource(&id, Some(revision.expected_revision))
        .map_err(admin_storage_error)?
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::NOT_FOUND,
                "admin",
                "model_resource_not_found",
                "Model resource revision does not exist",
                false,
            )
        })?;
    let reservation = reserve_admin_write::<ModelResourceReadinessResponse>(
        &service,
        &headers,
        &format!(
            "model-resource.readiness.{}:{}",
            resource.id, resource.revision
        ),
        &revision,
    )
    .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(response) = reservation else {
            unreachable!()
        };
        return Ok(Json(response));
    };
    let request = readiness_probe_request(&resource);
    let result = async {
        if !service.resource_circuit_allows(&resource, &request).await? {
            return Err(GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "admin",
                "provider_unavailable",
                "Model resource circuit is open",
                true,
            ));
        }
        let _permit = service
            .acquire_resource_bulkhead(&resource, &request)
            .await?;
        let provider = match service.call_resource(&resource, &request).await {
            Ok(provider) => provider,
            Err(error) => {
                if error.is_retryable_upstream() {
                    service
                        .record_resource_retryable_failure(&resource, &request)
                        .await?;
                }
                return Err(error);
            }
        };
        service.record_resource_success(&resource, &request).await?;
        Ok(ModelResourceReadinessResponse {
            model_resource_id: resource.id.clone(),
            revision: resource.revision,
            ready: true,
            provider_request_id: provider.provider_request_id,
            usage: provider.usage,
        })
    }
    .await;
    match result {
        Ok(response) => {
            service
                .inner
                .storage
                .lock()
                .await
                .audit_admin_operation(
                    "model_resource.readiness_succeeded",
                    &resource.id,
                    &change.operator_id,
                    &change.reason,
                    &change.change_reference,
                )
                .map_err(admin_storage_error)?;
            service
                .inner
                .storage
                .lock()
                .await
                .audit_event(
                    "model_resource.readiness_observed",
                    &resource.id,
                    &serde_json::json!({
                        "revision": response.revision,
                        "providerRequestId": &response.provider_request_id,
                        "usage": &response.usage,
                    }),
                )
                .map_err(admin_storage_error)?;
            complete_admin_write(&service, &key, &response).await?;
            Ok(Json(response))
        }
        Err(error) => {
            let _ = service.inner.storage.lock().await.audit_event(
                "model_resource.readiness_failed",
                &resource.id,
                &serde_json::json!({
                    "revision": resource.revision,
                    "code": error.code(),
                }),
            );
            discard_admin_write(&service, &key).await;
            Err(error)
        }
    }
}

async fn create_model_resource(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Json(write): Json<AdminModelResourceWrite>,
) -> std::result::Result<Json<AdminModelResourceView>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let reservation = reserve_admin_write::<AdminModelResourceView>(
        &service,
        &headers,
        "model-resource.write",
        &write,
    )
    .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(resource) = reservation else {
            unreachable!()
        };
        return Ok(Json(resource));
    };
    let result = async {
        validate_model_resource(&write.resource).map_err(|_| {
            GatewayApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "admin",
                "invalid_model_resource",
                "Model resource endpoint, schema, or authentication is invalid",
                false,
            )
        })?;
        if let Some(api_key) = write.api_key.as_deref() {
            service
                .write_secret_ref(&write.resource.auth.secret_ref, api_key)
                .await
                .map_err(|_| {
                    GatewayApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "admin",
                        "invalid_secret_ref",
                        "Admin API could not write the API key to the approved secret backend",
                        false,
                    )
                })?;
        }
        let resource = service
            .inner
            .storage
            .lock()
            .await
            .save_model_resource(write.resource, write.expected_revision)
            .map_err(admin_storage_error)?;
        service
            .inner
            .storage
            .lock()
            .await
            .audit_admin_operation(
                "model_resource.admin_write",
                &resource.id,
                &change.operator_id,
                &change.reason,
                &change.change_reference,
            )
            .map_err(admin_storage_error)?;
        refresh_configuration(&service)
            .await
            .map_err(admin_storage_error)?;
        Ok(AdminModelResourceView::from(&resource))
    }
    .await;
    match result {
        Ok(resource) => {
            complete_admin_write(&service, &key, &resource).await?;
            Ok(Json(resource))
        }
        Err(error) => {
            discard_admin_write(&service, &key).await;
            Err(error)
        }
    }
}

async fn enable_model_resource(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(revision): Json<AdminRevisionRequest>,
) -> std::result::Result<Json<AdminModelResourceView>, GatewayApiError> {
    set_model_resource_enabled(service, headers, id, revision.expected_revision, true).await
}

async fn disable_model_resource(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(revision): Json<AdminRevisionRequest>,
) -> std::result::Result<Json<AdminModelResourceView>, GatewayApiError> {
    set_model_resource_enabled(service, headers, id, revision.expected_revision, false).await
}

async fn set_model_resource_enabled(
    service: GatewayService,
    headers: HeaderMap,
    id: String,
    expected_revision: u64,
    enabled: bool,
) -> std::result::Result<Json<AdminModelResourceView>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let payload = AdminRevisionRequest { expected_revision };
    let operation = format!(
        "model-resource.{}.{}",
        if enabled { "enable" } else { "disable" },
        id
    );
    let reservation =
        reserve_admin_write::<AdminModelResourceView>(&service, &headers, &operation, &payload)
            .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(resource) = reservation else {
            unreachable!()
        };
        return Ok(Json(resource));
    };
    let result = async {
        let resource = service
            .inner
            .storage
            .lock()
            .await
            .set_model_resource_enabled(&id, enabled, expected_revision)
            .map_err(admin_storage_error)?;
        service
            .inner
            .storage
            .lock()
            .await
            .audit_admin_operation(
                if enabled {
                    "model_resource.enabled"
                } else {
                    "model_resource.disabled"
                },
                &resource.id,
                &change.operator_id,
                &change.reason,
                &change.change_reference,
            )
            .map_err(admin_storage_error)?;
        refresh_configuration(&service)
            .await
            .map_err(admin_storage_error)?;
        Ok(AdminModelResourceView::from(&resource))
    }
    .await;
    match result {
        Ok(resource) => {
            complete_admin_write(&service, &key, &resource).await?;
            Ok(Json(resource))
        }
        Err(error) => {
            discard_admin_write(&service, &key).await;
            Err(error)
        }
    }
}

async fn list_model_selection_policies(
    State(service): State<GatewayService>,
    headers: HeaderMap,
) -> std::result::Result<Json<Vec<ModelSelectionPolicy>>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    Ok(Json(
        service
            .inner
            .storage
            .lock()
            .await
            .current_model_selection_policies()
            .map_err(admin_storage_error)?,
    ))
}

async fn get_model_selection_policy(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> std::result::Result<Json<ModelSelectionPolicy>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    let policy = service
        .inner
        .storage
        .lock()
        .await
        .model_selection_policy(&id, query.revision)
        .map_err(admin_storage_error)?
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::NOT_FOUND,
                "admin",
                "model_selection_policy_not_found",
                "Model selection policy does not exist",
                false,
            )
        })?;
    Ok(Json(policy))
}

async fn create_model_selection_policy(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Json(write): Json<AdminModelSelectionPolicyWrite>,
) -> std::result::Result<Json<ModelSelectionPolicy>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let reservation = reserve_admin_write::<ModelSelectionPolicy>(
        &service,
        &headers,
        "model-selection-policy.write",
        &write,
    )
    .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(policy) = reservation else {
            unreachable!()
        };
        return Ok(Json(policy));
    };
    let result = async {
        let resources = service
            .inner
            .storage
            .lock()
            .await
            .current_model_resources()
            .map_err(admin_storage_error)?;
        validate_model_selection_policy(&write.policy, &resources).map_err(|_| {
            GatewayApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "admin",
                "invalid_model_selection_policy",
                "Model selection policy refers to an unknown resource or has an invalid schema",
                false,
            )
        })?;
        let policy = service
            .inner
            .storage
            .lock()
            .await
            .save_model_selection_policy(write.policy, write.expected_revision)
            .map_err(admin_storage_error)?;
        service
            .inner
            .storage
            .lock()
            .await
            .audit_admin_operation(
                "model_selection_policy.admin_write",
                &policy.id,
                &change.operator_id,
                &change.reason,
                &change.change_reference,
            )
            .map_err(admin_storage_error)?;
        refresh_configuration(&service)
            .await
            .map_err(admin_storage_error)?;
        Ok(policy)
    }
    .await;
    match result {
        Ok(policy) => {
            complete_admin_write(&service, &key, &policy).await?;
            Ok(Json(policy))
        }
        Err(error) => {
            discard_admin_write(&service, &key).await;
            Err(error)
        }
    }
}

async fn activate_model_selection_policy(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(activation): Json<AdminPolicyActivationRequest>,
) -> std::result::Result<Json<ModelSelectionPolicy>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let operation = format!("model-selection-policy.activate.{id}");
    let reservation =
        reserve_admin_write::<ModelSelectionPolicy>(&service, &headers, &operation, &activation)
            .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(policy) = reservation else {
            unreachable!()
        };
        return Ok(Json(policy));
    };
    let result = async {
        let resources = service
            .inner
            .storage
            .lock()
            .await
            .current_model_resources()
            .map_err(admin_storage_error)?;
        let target = service
            .inner
            .storage
            .lock()
            .await
            .model_selection_policy(&id, Some(activation.revision_to_activate))
            .map_err(admin_storage_error)?
            .ok_or_else(|| {
                GatewayApiError::new(
                    StatusCode::NOT_FOUND,
                    "admin",
                    "model_selection_policy_not_found",
                    "Model selection policy revision does not exist",
                    false,
                )
            })?;
        validate_model_selection_policy(&target, &resources).map_err(|_| {
            GatewayApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "admin",
                "invalid_model_selection_policy",
                "Model selection policy is no longer valid against current resources",
                false,
            )
        })?;
        let policy = service
            .inner
            .storage
            .lock()
            .await
            .activate_model_selection_policy(
                &id,
                activation.revision_to_activate,
                activation.expected_revision,
            )
            .map_err(admin_storage_error)?;
        service
            .inner
            .storage
            .lock()
            .await
            .audit_admin_operation(
                "model_selection_policy.admin_activated",
                &policy.id,
                &change.operator_id,
                &change.reason,
                &change.change_reference,
            )
            .map_err(admin_storage_error)?;
        refresh_configuration(&service)
            .await
            .map_err(admin_storage_error)?;
        Ok(policy)
    }
    .await;
    match result {
        Ok(policy) => {
            complete_admin_write(&service, &key, &policy).await?;
            Ok(Json(policy))
        }
        Err(error) => {
            discard_admin_write(&service, &key).await;
            Err(error)
        }
    }
}

async fn list_model_executions(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Query(query): Query<ExecutionQuery>,
) -> std::result::Result<Json<Vec<ModelExecutionSummary>>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    Ok(Json(
        service
            .inner
            .storage
            .lock()
            .await
            .execution_snapshots_for_run(&query.run_id)
            .map_err(admin_storage_error)?,
    ))
}

async fn list_audit_events(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Query(query): Query<AuditEventsQuery>,
) -> std::result::Result<Json<Vec<AdminAuditEventView>>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 100 || query.before_id.is_some_and(|id| id <= 0) {
        return Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            "admin",
            "invalid_audit_query",
            "Audit event pagination parameters are invalid",
            false,
        ));
    }
    let events = service
        .inner
        .storage
        .lock()
        .await
        .audit_events(
            query.event_type.as_deref(),
            query.subject_id.as_deref(),
            query.before_id,
            limit,
        )
        .map_err(admin_storage_error)?
        .into_iter()
        .map(AdminAuditEventView::from)
        .collect();
    Ok(Json(events))
}

async fn reconcile_gitops_configuration(
    State(service): State<GatewayService>,
    headers: HeaderMap,
) -> std::result::Result<Json<GitopsReconcileReport>, GatewayApiError> {
    authorize_admin(&service.inner.config, &headers)?;
    require_admin_idempotency(&headers)?;
    let change = require_admin_change_context(&headers)?;
    let (config, config_digest) = service.read_gitops_config_file().map_err(|_| {
        GatewayApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "admin",
            "gitops_configuration_invalid",
            "Mounted GitOps configuration is unavailable or invalid",
            false,
        )
    })?;
    let payload = GitopsReconcileRequest {
        config_digest: config_digest.clone(),
    };
    let reservation = reserve_admin_write::<GitopsReconcileReport>(
        &service,
        &headers,
        "gitops-configuration.reconcile",
        &payload,
    )
    .await?;
    let AdminWriteReservation::Reserved { key } = reservation else {
        let AdminWriteReservation::Completed(report) = reservation else {
            unreachable!()
        };
        return Ok(Json(report));
    };
    let result = service.reconcile_gitops_config(config, config_digest).await;
    match result {
        Ok(report) => {
            service
                .inner
                .storage
                .lock()
                .await
                .audit_admin_operation(
                    "gitops_configuration.admin_reconciled",
                    "provider-gateway",
                    &change.operator_id,
                    &change.reason,
                    &change.change_reference,
                )
                .map_err(admin_storage_error)?;
            complete_admin_write(&service, &key, &report).await?;
            Ok(Json(report))
        }
        Err(_) => {
            discard_admin_write(&service, &key).await;
            Err(GatewayApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "admin",
                "gitops_configuration_reconcile_failed",
                "GitOps configuration could not be reconciled",
                true,
            ))
        }
    }
}

fn redact_audit_metadata(value: Value) -> Value {
    const SENSITIVE_KEYS: &[&str] = &[
        "apikey",
        "authorization",
        "baseurl",
        "cookie",
        "endpoint",
        "secretref",
        "systemprompt",
    ];
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_audit_metadata).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| {
                    (!SENSITIVE_KEYS.contains(&key.to_ascii_lowercase().as_str()))
                        .then(|| (key, redact_audit_metadata(value)))
                })
                .collect(),
        ),
        value => value,
    }
}

async fn refresh_configuration(service: &GatewayService) -> Result<()> {
    let (resources, policies) = {
        let storage = service.inner.storage.lock().await;
        (
            storage.current_model_resources()?,
            storage.current_model_selection_policies()?,
        )
    };
    let resources = resources
        .into_iter()
        .map(|resource| (resource.id.clone(), resource))
        .collect();
    *service.inner.resources.write().await = resources;
    *service.inner.policies.write().await = policies;
    Ok(())
}

fn write_file_secret_ref(secret_ref: &str, api_key: &str) -> Result<()> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("API key must not be empty"));
    }
    let path = secret_ref
        .strip_prefix("file:")
        .ok_or_else(|| anyhow!("only file: secret references are supported by this backend"))?;
    let path = std::path::Path::new(path);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("secret path has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("secret"),
        std::process::id()
    ));
    fs::write(&temporary, api_key)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn authorize_admin(
    config: &GatewayConfig,
    headers: &HeaderMap,
) -> std::result::Result<(), GatewayApiError> {
    let Some(expected) = &config.admin_bearer_token else {
        return Err(GatewayApiError::new(
            StatusCode::UNAUTHORIZED,
            "admin",
            "admin_authentication_failed",
            "Admin API is not configured",
            false,
        ));
    };
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if actual == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(GatewayApiError::new(
            StatusCode::UNAUTHORIZED,
            "admin",
            "admin_authentication_failed",
            "Admin identity is invalid",
            false,
        ))
    }
}

fn require_admin_idempotency(headers: &HeaderMap) -> std::result::Result<(), GatewayApiError> {
    if headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty())
    {
        Ok(())
    } else {
        Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            "admin",
            "idempotency_key_required",
            "Admin write operations require Idempotency-Key",
            false,
        ))
    }
}

fn require_admin_change_context(
    headers: &HeaderMap,
) -> std::result::Result<AdminChangeContext, GatewayApiError> {
    let field = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    match (
        field("x-operator-id"),
        field("x-change-reason"),
        field("x-change-reference"),
    ) {
        (Some(operator_id), Some(reason), Some(change_reference)) => Ok(AdminChangeContext {
            operator_id,
            reason,
            change_reference,
        }),
        _ => Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            "admin",
            "admin_change_context_required",
            "Admin write operations require x-operator-id, x-change-reason, and x-change-reference",
            false,
        )),
    }
}

fn admin_storage_error(error: anyhow::Error) -> GatewayApiError {
    let conflict = error.to_string().contains("revision conflict");
    GatewayApiError::new(
        if conflict {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        "admin",
        if conflict {
            "admin_revision_conflict"
        } else {
            "gateway_storage_unavailable"
        },
        if conflict {
            "The requested revision is no longer current"
        } else {
            "Gateway durable state is temporarily unavailable"
        },
        !conflict,
    )
}

async fn ready_handler(State(service): State<GatewayService>) -> StatusCode {
    if !service.is_accepting_turns() {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let storage_ready = {
        let storage = service.inner.storage.lock().await;
        storage.current_model_resources().is_ok()
            && storage.current_model_selection_policies().is_ok()
    };
    if !storage_ready {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let resources = service.inner.resources.read().await.clone();
    let policies = service.inner.policies.read().await.clone();
    for policy in &policies {
        for candidate in &policy.candidates {
            if let Some(resource) = resources.get(&candidate.model_resource_id) {
                if resource.enabled
                    && service
                        .resolve_secret_ref(&resource.auth.secret_ref)
                        .await
                        .is_ok()
                {
                    return StatusCode::OK;
                }
            }
        }
    }
    StatusCode::SERVICE_UNAVAILABLE
}

async fn metrics_handler(State(service): State<GatewayService>) -> Response {
    let mut body = service.inner.metrics.lock().await.render_prometheus();
    let health = service
        .inner
        .storage
        .lock()
        .await
        .resource_health_states()
        .unwrap_or_default()
        .into_iter()
        .map(|state| {
            (
                format!(
                    "{}:{}",
                    state.model_resource_id, state.model_resource_revision
                ),
                state,
            )
        })
        .collect::<HashMap<String, ResourceHealthState>>();
    let resources = service.inner.resources.read().await;
    let bulkheads = service.inner.bulkheads.lock().await;
    for resource in resources.values() {
        let key = format!("{}:{}", resource.id, resource.revision);
        let circuit_state = health
            .get(&key)
            .map(|state| state.circuit_state.as_str())
            .unwrap_or("closed");
        let circuit_value = match circuit_state {
            "open" => 2,
            "half_open" => 1,
            _ => 0,
        };
        let queue_depth = bulkheads
            .get(&key)
            .map(|semaphore| {
                resource
                    .defaults
                    .max_concurrent_requests
                    .saturating_sub(semaphore.available_permits())
            })
            .unwrap_or_default();
        let labels = vec![("model_resource", resource.id.clone())];
        let _ = writeln!(
            body,
            "provider_gateway_circuit_state{} {}",
            prometheus_labels(&labels),
            circuit_value
        );
        let _ = writeln!(
            body,
            "provider_gateway_queue_depth{} {}",
            prometheus_labels(&labels),
            queue_depth
        );
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn turn_handler(
    State(service): State<GatewayService>,
    headers: HeaderMap,
    Json(request): Json<GatewayTurnRequest>,
) -> std::result::Result<Json<GatewayTurnResponse>, GatewayApiError> {
    authorize(&service.inner.config, &headers, &request.request_id)?;
    if !service.is_accepting_turns() {
        return Err(GatewayApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            request.request_id,
            "gateway_draining",
            "Gateway is draining and is not accepting new model turns",
            true,
        ));
    }
    let started = Instant::now();
    let result = service.execute_turn(request.clone()).await;
    service
        .record_turn_metric(&request, &result, started.elapsed())
        .await;
    result.map(Json)
}

fn authorize(
    config: &GatewayConfig,
    headers: &HeaderMap,
    request_id: &str,
) -> std::result::Result<(), GatewayApiError> {
    let Some(expected) = &config.runtime_bearer_token else {
        return Ok(());
    };
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if actual == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(GatewayApiError::new(
            StatusCode::UNAUTHORIZED,
            request_id,
            "runtime_authentication_failed",
            "Runtime service identity is invalid",
            false,
        ))
    }
}

fn validate_config(config: &GatewayConfig) -> Result<()> {
    if config.resources.is_empty() {
        return Err(anyhow!(
            "provider gateway requires at least one model resource"
        ));
    }
    if config.policies.is_empty() {
        return Err(anyhow!(
            "provider gateway requires at least one model selection policy"
        ));
    }
    let mut ids = HashSet::new();
    for resource in &config.resources {
        validate_model_resource(resource)?;
        if !ids.insert(resource.id.clone()) {
            return Err(anyhow!("duplicate model resource id: {}", resource.id));
        }
    }
    for policy in &config.policies {
        validate_model_selection_policy(policy, &config.resources)?;
    }
    Ok(())
}

fn validate_model_resource(resource: &ModelResource) -> Result<()> {
    if resource.schema_version != MODEL_RESOURCE_SCHEMA
        || resource.id.trim().is_empty()
        || resource.display_name.trim().is_empty()
        || resource.physical_model.trim().is_empty()
        || resource.auth.auth_type != "bearer"
        || resource.auth.secret_ref.trim().is_empty()
        || resource.defaults.max_attempts == 0
        || resource.defaults.max_attempts > 3
        || resource.defaults.max_concurrent_requests == 0
        || resource.defaults.max_concurrent_requests > 1_000
    {
        return Err(anyhow!("model resource has invalid required fields"));
    }
    if let Some(name) = resource.auth.secret_ref.strip_prefix("db:") {
        validate_database_secret_name(name)?;
    } else if !resource.auth.secret_ref.starts_with("file:")
        && !resource.auth.secret_ref.starts_with("env:")
    {
        return Err(anyhow!(
            "model resource secret reference backend is unsupported"
        ));
    }
    let url = reqwest::Url::parse(&resource.endpoint.base_url)
        .context("model resource base URL is invalid")?;
    let local_test_host = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    let allow_loopback = cfg!(test)
        || env::var("PROVIDER_GATEWAY_ALLOW_LOOPBACK")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("model resource endpoint host is required"))?;
    if (url.scheme() != "https" && !local_test_host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || resource.endpoint.chat_completions_path.trim().is_empty()
        || !resource.endpoint.chat_completions_path.starts_with('/')
        || (!allow_loopback && unsafe_endpoint_host(host))
    {
        return Err(anyhow!("model resource endpoint violates network policy"));
    }
    Ok(())
}

fn unsafe_endpoint_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return true;
    }
    let Ok(ip) = host.parse::<IpAddr>() else {
        return false;
    };
    unsafe_endpoint_ip(ip)
}

fn unsafe_endpoint_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || is_ipv6_link_local(ip)
                || is_ipv6_unique_local(ip)
        }
    }
}

async fn validate_resource_endpoint_dns(base_url: &str) -> Result<()> {
    let url = reqwest::Url::parse(base_url).context("model resource base URL is invalid")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("model resource endpoint host is required"))?;
    let allow_loopback = cfg!(test)
        || env::var("PROVIDER_GATEWAY_ALLOW_LOOPBACK")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    if allow_loopback {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("resolving provider endpoint {host}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| unsafe_endpoint_ip(address.ip()))
    {
        return Err(anyhow!("provider endpoint resolves to a forbidden network"));
    }
    Ok(())
}

fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn ordered_automatic_candidates(
    policy: &ModelSelectionPolicy,
    resources: &BTreeMap<String, ModelResource>,
    selection_key: &str,
) -> Vec<ModelResource> {
    let mut candidates = policy.candidates.clone();
    candidates.sort_by_key(|candidate| candidate.priority);
    let Some(first_priority) = candidates.first().map(|candidate| candidate.priority) else {
        return Vec::new();
    };
    let primary_candidates = candidates
        .iter()
        .filter(|candidate| candidate.priority == first_priority)
        .filter(|candidate| resources.contains_key(&candidate.model_resource_id))
        .collect::<Vec<_>>();
    let primary_id = weighted_candidate_id(&primary_candidates, selection_key);
    let mut ordered = primary_id
        .as_deref()
        .and_then(|id| resources.get(id))
        .cloned()
        .into_iter()
        .collect::<Vec<_>>();
    ordered.extend(
        candidates
            .into_iter()
            .filter(|candidate| primary_id.as_deref() != Some(candidate.model_resource_id.as_str()))
            .filter_map(|candidate| resources.get(&candidate.model_resource_id).cloned()),
    );
    ordered
}

fn weighted_candidate_id(candidates: &[&ModelCandidate], selection_key: &str) -> Option<String> {
    let total_weight = candidates
        .iter()
        .map(|candidate| u64::from(candidate.weight))
        .sum::<u64>();
    if total_weight == 0 {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(selection_key.as_bytes());
    let digest = hasher.finalize();
    let mut bucket_bytes = [0u8; 8];
    bucket_bytes.copy_from_slice(&digest[..8]);
    let mut bucket = u64::from_be_bytes(bucket_bytes) % total_weight;
    for candidate in candidates {
        let weight = u64::from(candidate.weight);
        if bucket < weight {
            return Some(candidate.model_resource_id.clone());
        }
        bucket -= weight;
    }
    None
}

fn validate_model_selection_policy(
    policy: &ModelSelectionPolicy,
    resources: &[ModelResource],
) -> Result<()> {
    if policy.schema_version != MODEL_SELECTION_POLICY_SCHEMA || policy.id.trim().is_empty() {
        return Err(anyhow!(
            "model selection policy has invalid required fields"
        ));
    }
    if policy
        .scope
        .workspace_ids
        .iter()
        .any(|id| !is_workspace_namespace(id))
        || policy
            .scope
            .project_ids
            .iter()
            .any(|id| id.trim().is_empty())
    {
        return Err(anyhow!(
            "model selection policy scope contains an invalid identifier"
        ));
    }
    let allowed_phases = ["brief", "build", "repair", "review", "edit", "export"];
    let mut configured_phases = HashSet::new();
    if policy.applies_to.phases.iter().any(|phase| {
        !allowed_phases.contains(&phase.as_str()) || !configured_phases.insert(phase.as_str())
    }) {
        return Err(anyhow!(
            "model selection policy has an invalid or duplicate phase selector"
        ));
    }
    let mut configured_profiles = HashSet::new();
    if policy
        .applies_to
        .agent_profiles
        .iter()
        .any(|profile| profile.trim().is_empty() || !configured_profiles.insert(profile.as_str()))
    {
        return Err(anyhow!(
            "model selection policy has an invalid or duplicate agent profile selector"
        ));
    }
    let known_ids = resources
        .iter()
        .map(|resource| resource.id.as_str())
        .collect::<HashSet<_>>();
    let mut candidate_ids = HashSet::new();
    for candidate in &policy.candidates {
        if candidate.model_resource_id.trim().is_empty()
            || candidate.weight == 0
            || !candidate_ids.insert(candidate.model_resource_id.as_str())
        {
            return Err(anyhow!(
                "policy {} has an invalid or duplicate automatic candidate",
                policy.id
            ));
        }
    }
    for resource_id in policy
        .candidates
        .iter()
        .map(|candidate| candidate.model_resource_id.as_str())
        .chain(
            policy
                .direct_selection
                .allowed_model_resource_ids
                .iter()
                .map(String::as_str),
        )
    {
        if !known_ids.contains(resource_id) {
            return Err(anyhow!(
                "policy {} refers to unknown model resource {}",
                policy.id,
                resource_id
            ));
        }
    }
    Ok(())
}

fn validate_turn_request(request: &GatewayTurnRequest) -> std::result::Result<(), GatewayApiError> {
    let request_id = &request.request_id;
    if request.schema_version != TURN_REQUEST_SCHEMA
        || request.request_id.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || !is_workspace_namespace(&request.scope.workspace_id)
        || request.scope.project_id.trim().is_empty()
        || request.scope.run_id.trim().is_empty()
        || request.scope.agent_profile.trim().is_empty()
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request_id,
            "invalid_turn_request",
            "Turn request is missing required identity fields or schema version",
            false,
        ));
    }
    if !["brief", "build", "repair", "review", "edit", "export"]
        .contains(&request.scope.phase.as_str())
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request_id,
            "invalid_turn_request",
            "Turn request phase is not supported",
            false,
        ));
    }
    let deadline = DateTime::parse_from_rfc3339(&request.deadline_at).map_err(|_| {
        GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request_id,
            "invalid_turn_request",
            "deadlineAt must be an RFC3339 timestamp",
            false,
        )
    })?;
    if deadline.with_timezone(&Utc) <= Utc::now() {
        return Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request_id,
            "invalid_turn_request",
            "deadlineAt must be in the future",
            false,
        ));
    }
    if request.input.system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES
        || request.input.messages.len() > MAX_MESSAGE_COUNT
        || request.input.tools.len() + request.input.deferred_tools.len() > MAX_TOOL_COUNT
        || serde_json::to_vec(&request.input).map_or(true, |body| body.len() > MAX_TURN_BODY_BYTES)
        || request
            .input
            .messages
            .iter()
            .any(|message| !json_within_limits(message, 0))
        || request
            .input
            .tools
            .iter()
            .chain(&request.input.deferred_tools)
            .any(|tool| {
                tool.name.trim().is_empty()
                    || !json_within_limits(&tool.input_schema, 0)
                    || tool
                        .input_json_schema
                        .as_ref()
                        .is_some_and(|schema| !json_within_limits(schema, 0))
                    || serde_json::to_vec(
                        &tool
                            .input_json_schema
                            .as_ref()
                            .unwrap_or(&tool.input_schema),
                    )
                    .map_or(true, |schema| schema.len() > MAX_TOOL_SCHEMA_BYTES)
            })
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request_id,
            "invalid_turn_request",
            "Turn request exceeds configured size limits",
            false,
        ));
    }
    Ok(())
}

fn is_workspace_namespace(value: &str) -> bool {
    let bytes = value.as_bytes();
    (4..=63).contains(&bytes.len())
        && value.starts_with("ws-")
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn provider_call_timeout(
    resource: &ModelResource,
    request: &GatewayTurnRequest,
) -> std::result::Result<Duration, GatewayApiError> {
    Ok(Duration::from_millis(resource.defaults.request_timeout_ms)
        .min(remaining_turn_deadline(request)?))
}

fn remaining_turn_deadline(
    request: &GatewayTurnRequest,
) -> std::result::Result<Duration, GatewayApiError> {
    let deadline = DateTime::parse_from_rfc3339(&request.deadline_at).map_err(|_| {
        GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request.request_id.clone(),
            "invalid_turn_request",
            "deadlineAt must be an RFC3339 timestamp",
            false,
        )
    })?;
    let remaining = deadline.with_timezone(&Utc) - Utc::now();
    let remaining = remaining.to_std().map_err(|_| {
        GatewayApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            request.request_id.clone(),
            "provider_timeout",
            "Turn request deadline elapsed before the Provider call",
            true,
        )
    })?;
    if remaining.is_zero() {
        return Err(GatewayApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            request.request_id.clone(),
            "provider_timeout",
            "Turn request deadline elapsed before the Provider call",
            true,
        ));
    }
    Ok(remaining)
}

fn provider_retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1_000).min(60_000))
}

fn provider_retry_delay(
    request: &GatewayTurnRequest,
    attempt: u32,
    error: &GatewayApiError,
) -> Duration {
    if let Some(retry_after_ms) = error.retry_after_ms() {
        return Duration::from_millis(retry_after_ms);
    }
    let exponent = attempt.saturating_sub(1).min(4);
    let base_ms = 100u64.saturating_mul(1u64 << exponent);
    let mut hasher = Sha256::new();
    hasher.update(request.idempotency_key.as_bytes());
    hasher.update(attempt.to_be_bytes());
    let digest = hasher.finalize();
    let jitter_ceiling = (base_ms / 4).max(1);
    let jitter = u64::from(digest[0]) % jitter_ceiling;
    Duration::from_millis(base_ms + jitter)
}

fn json_within_limits(value: &Value, depth: usize) -> bool {
    if depth > MAX_JSON_DEPTH {
        return false;
    }
    match value {
        Value::String(value) => value.len() <= MAX_JSON_STRING_BYTES,
        Value::Array(values) => values
            .iter()
            .all(|value| json_within_limits(value, depth + 1)),
        Value::Object(values) => values.iter().all(|(key, value)| {
            key.len() <= MAX_JSON_STRING_BYTES && json_within_limits(value, depth + 1)
        }),
        _ => true,
    }
}

fn policy_matches_turn(policy: &ModelSelectionPolicy, scope: &TurnScope) -> bool {
    let project_matches = policy.scope.project_ids.is_empty()
        || policy
            .scope
            .project_ids
            .iter()
            .any(|project_id| project_id == &scope.project_id);
    let workspace_matches = policy.scope.workspace_ids.is_empty()
        || policy
            .scope
            .workspace_ids
            .iter()
            .any(|workspace_id| workspace_id == &scope.workspace_id);
    let phase_matches = policy.applies_to.phases.is_empty()
        || policy
            .applies_to
            .phases
            .iter()
            .any(|phase| phase == &scope.phase);
    let profile_matches = policy.applies_to.agent_profiles.is_empty()
        || policy
            .applies_to
            .agent_profiles
            .iter()
            .any(|profile| profile == &scope.agent_profile);
    workspace_matches && project_matches && phase_matches && profile_matches
}

fn capabilities_match(resource: &ModelResource, required: &RequiredCapabilities) -> bool {
    (!required.tool_calls || resource.capabilities.tool_calls)
        && (!required.strict_tool_schema || resource.capabilities.strict_tool_schema)
        && (!required.streaming || resource.capabilities.streaming)
        && (!required.vision || resource.capabilities.vision)
}

fn build_turn_response(
    request: &GatewayTurnRequest,
    policy: &ModelSelectionPolicy,
    resource: &ModelResource,
    selection_reason: &str,
    automatic_switch: AutomaticSwitchSummary,
    provider: ProviderTurn,
) -> GatewayTurnResponse {
    let response_type = if provider.tool_calls.is_empty() {
        "text".to_string()
    } else {
        "tool_calls".to_string()
    };
    let provider_request_id = provider.provider_request_id.clone();
    let provider_attempt_count = provider.attempt_count;
    GatewayTurnResponse {
        schema_version: TURN_RESPONSE_SCHEMA.to_string(),
        request_id: request.request_id.clone(),
        response_type,
        tool_calls: provider.tool_calls,
        text: provider.text,
        finish_reason: provider.finish_reason,
        model_execution: ModelExecutionSummary {
            id: format!("model-execution-{}", request.request_id),
            model_resource_id: resource.id.clone(),
            model_resource_revision: resource.revision,
            provider_id: resource.id.clone(),
            physical_model: resource.physical_model.clone(),
            selection_policy_id: policy.id.clone(),
            selection_policy_revision: policy.revision,
            capability_snapshot_hash: capability_hash(&resource.capabilities),
            selection_reason: if automatic_switch.used {
                "automatic_switch".to_string()
            } else {
                selection_reason.to_string()
            },
            automatic_switch,
            provider_request_id: provider_request_id.clone(),
            provider_attempt_count,
        },
        usage: provider.usage,
        provider: ProviderMetadata {
            request_id: provider_request_id,
            attempt_count: provider_attempt_count,
        },
    }
}

fn readiness_probe_request(resource: &ModelResource) -> GatewayTurnRequest {
    GatewayTurnRequest {
        schema_version: TURN_REQUEST_SCHEMA.to_string(),
        request_id: format!("readiness-{}-{}", resource.id, resource.revision),
        idempotency_key: format!("readiness-{}-{}", resource.id, resource.revision),
        deadline_at: (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        scope: TurnScope {
            workspace_id: "ws-provider-admin".to_string(),
            project_id: "provider-gateway-readiness".to_string(),
            run_id: format!("readiness-{}-{}", resource.id, resource.revision),
            turn: 1,
            phase: "readiness".to_string(),
            agent_profile: "gateway-readiness".to_string(),
        },
        routing: TurnRouting {
            model_resource_id: Some(resource.id.clone()),
            required_capabilities: RequiredCapabilities::default(),
        },
        input: TurnInput {
            system_prompt: "Reply with READY.".to_string(),
            messages: vec![],
            tools: vec![],
            deferred_tools: vec![],
        },
    }
}

fn capability_hash(capabilities: &ProviderCapabilities) -> String {
    sha256_json(capabilities).unwrap_or_default()
}

fn sha256_json(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn same_revisioned_configuration<T: Serialize>(current: &T, desired: &T) -> bool {
    let Ok(mut current) = serde_json::to_value(current) else {
        return false;
    };
    let Ok(mut desired) = serde_json::to_value(desired) else {
        return false;
    };
    if let Value::Object(object) = &mut current {
        object.remove("revision");
    }
    if let Value::Object(object) = &mut desired {
        object.remove("revision");
    }
    current == desired
}

fn idempotency_request_hash(request: &GatewayTurnRequest) -> Result<String> {
    let mut value = serde_json::to_value(request)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("turn request must serialize to an object"))?;
    // Retries receive a fresh request id and deadline, while the logical turn
    // input must remain identical for an idempotency key to be reused.
    object.remove("requestId");
    object.remove("deadlineAt");
    sha256_json(&value)
}

fn estimate_input_tokens(input: &TurnInput) -> u64 {
    serde_json::to_vec(input)
        .map(|bytes| ((bytes.len() as u64).saturating_add(3)) / 4)
        .unwrap_or_default()
}

fn resolve_local_secret_ref(secret_ref: &str) -> Result<String> {
    if let Some(variable) = secret_ref.strip_prefix("env:") {
        return env::var(variable)
            .with_context(|| format!("reading secret reference env:{variable}"));
    }
    if let Some(path) = secret_ref.strip_prefix("file:") {
        return fs::read_to_string(path)
            .with_context(|| format!("reading secret reference file:{path}"))
            .map(|value| value.trim().to_string());
    }
    Err(anyhow!("unsupported secret reference backend"))
}

fn validate_database_secret_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(anyhow!("database secret name is invalid"));
    }
    Ok(())
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn openai_request_body(
    request: &GatewayTurnRequest,
    resource: &ModelResource,
    tool_aliases: &ProviderToolAliasMap,
) -> std::result::Result<Value, GatewayApiError> {
    let messages = normalize_messages(request, tool_aliases).map_err(|message| {
        GatewayApiError::new(
            StatusCode::BAD_REQUEST,
            request.request_id.clone(),
            "invalid_turn_request",
            message,
            false,
        )
    })?;
    let tools = request
        .input
        .tools
        .iter()
        .map(|tool| {
            let provider_name = tool_aliases.provider_name(&tool.name).ok_or_else(|| {
                GatewayApiError::new(
                    StatusCode::BAD_REQUEST,
                    request.request_id.clone(),
                    "invalid_turn_request",
                    "Tool alias mapping is incomplete",
                    false,
                )
            })?;
            Ok(json!({
                "type": "function",
                "function": {
                    "name": provider_name,
                    "parameters": tool.input_json_schema.as_ref().unwrap_or(&tool.input_schema),
                    "strict": resource.capabilities.strict_tool_schema,
                }
            }))
        })
        .collect::<std::result::Result<Vec<_>, GatewayApiError>>()?;
    let mut body = json!({
        "model": resource.physical_model,
        "messages": messages,
        "tools": tools,
    });
    if let Some(temperature) = resource.defaults.temperature {
        body["temperature"] = json!(temperature);
    }
    if resource.endpoint.base_url.contains("deepseek.com") {
        body["thinking"] = json!({ "type": "disabled" });
    }
    Ok(body)
}

fn normalize_messages(
    request: &GatewayTurnRequest,
    tool_aliases: &ProviderToolAliasMap,
) -> std::result::Result<Vec<Value>, String> {
    let mut messages = vec![json!({ "role": "system", "content": request.input.system_prompt })];
    let mut pending_tool_call_ids = HashSet::new();
    let mut index = 0;
    while index < request.input.messages.len() {
        let message = &request.input.messages[index];
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "Every message requires a role".to_string())?;
        if role == "assistant" {
            if let Some(tool_calls) = message.get("toolCalls").and_then(Value::as_array) {
                pending_tool_call_ids.clear();
                let tool_calls = tool_calls
                    .iter()
                    .map(|call| {
                        let id = call
                            .get("id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "assistant tool call requires id".to_string())?;
                        let name = call
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "assistant tool call requires name".to_string())?;
                        let provider_name = tool_aliases
                            .provider_name(name)
                            .map(str::to_string)
                            .unwrap_or_else(|| provider_tool_alias(name));
                        let input = call.get("input").cloned().unwrap_or_else(|| json!({}));
                        pending_tool_call_ids.insert(id.to_string());
                        Ok(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": provider_name, "arguments": serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()) }
                        }))
                    })
                    .collect::<std::result::Result<Vec<_>, String>>()?;
                messages.push(json!({ "role": "assistant", "tool_calls": tool_calls }));
                index += 1;
                continue;
            }
        }
        if role == "tool" {
            let call_id = message
                .get("toolUseId")
                .and_then(Value::as_str)
                .ok_or_else(|| "tool message requires toolUseId".to_string())?;
            if !pending_tool_call_ids.contains(call_id) {
                let mut orphan_calls = Vec::new();
                let mut orphan_index = index;
                while orphan_index < request.input.messages.len() {
                    let orphan = &request.input.messages[orphan_index];
                    if orphan.get("role").and_then(Value::as_str) != Some("tool") {
                        break;
                    }
                    let id = orphan
                        .get("toolUseId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "tool message requires toolUseId".to_string())?;
                    let name = orphan
                        .get("toolName")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            "orphaned compacted tool message requires toolName".to_string()
                        })?;
                    let provider_name = tool_aliases
                        .provider_name(name)
                        .map(str::to_string)
                        .unwrap_or_else(|| provider_tool_alias(name));
                    orphan_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": provider_name,
                            "arguments": "{}"
                        }
                    }));
                    pending_tool_call_ids.insert(id.to_string());
                    orphan_index += 1;
                }
                messages.push(json!({ "role": "assistant", "tool_calls": orphan_calls }));
            }
            let content = message.get("content").cloned().unwrap_or(Value::Null);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content_to_string(&content),
            }));
            pending_tool_call_ids.remove(call_id);
            index += 1;
            continue;
        }
        pending_tool_call_ids.clear();
        let content = message
            .get("content")
            .or_else(|| message.get("text"))
            .ok_or_else(|| "message requires content or text".to_string())?;
        messages.push(json!({ "role": role, "content": content_to_string(content) }));
        index += 1;
    }
    Ok(messages)
}

fn content_to_string(content: &Value) -> String {
    content
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| serde_json::to_string(content).unwrap_or_default())
}

fn provider_rejection_detail(body: &str) -> Option<String> {
    const MAX_DETAIL_CHARS: usize = 320;
    let value = serde_json::from_str::<Value>(body).ok()?;
    let error = value.get("error").unwrap_or(&value);
    let mut parts = Vec::new();
    for key in ["type", "code", "message"] {
        if let Some(value) = error.get(key).and_then(Value::as_str) {
            let sanitized = redact_provider_error_secrets(
                &value
                    .chars()
                    .filter(|character| !character.is_control())
                    .collect::<String>(),
            );
            if !sanitized.trim().is_empty() {
                parts.push(format!("{key}={sanitized}"));
            }
        }
    }
    let mut detail = parts.join(" ");
    if detail.is_empty() {
        return None;
    }
    if detail.chars().count() > MAX_DETAIL_CHARS {
        detail = detail.chars().take(MAX_DETAIL_CHARS).collect();
        detail.push('…');
    }
    Some(detail)
}

fn redact_provider_error_secrets(value: &str) -> String {
    let mut redacted = redact_token_after_prefix(value, "sk-", "[REDACTED_API_KEY]", true);
    for prefix in ["bearer ", "api_key=", "api-key=", "apikey="] {
        redacted = redact_token_after_prefix(&redacted, prefix, "[REDACTED]", false);
    }
    redacted
}

fn redact_token_after_prefix(
    value: &str,
    prefix: &str,
    replacement: &str,
    include_prefix: bool,
) -> String {
    let lowercase = value.to_ascii_lowercase();
    let prefix_lowercase = prefix.to_ascii_lowercase();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some(relative) = lowercase[cursor..].find(&prefix_lowercase) {
        let start = cursor + relative;
        output.push_str(&value[cursor..start]);
        let token_start = start + prefix.len();
        let mut token_end = token_start;
        for (offset, character) in value[token_start..].char_indices() {
            if character.is_whitespace()
                || character.is_control()
                || matches!(character, '"' | '\'' | ',' | ';' | '&' | ')' | ']' | '}')
            {
                break;
            }
            token_end = token_start + offset + character.len_utf8();
        }
        if token_end == token_start {
            output.push_str(&value[start..token_start]);
            cursor = token_start;
            continue;
        }
        if !include_prefix {
            output.push_str(&value[start..token_start]);
        }
        output.push_str(replacement);
        cursor = token_end;
    }
    output.push_str(&value[cursor..]);
    output
}

#[derive(Debug, Clone)]
struct ProviderToolAliasMap {
    runtime_to_provider: HashMap<String, String>,
    provider_to_runtime: HashMap<String, String>,
}

impl ProviderToolAliasMap {
    fn from_request(request: &GatewayTurnRequest) -> std::result::Result<Self, GatewayApiError> {
        let mut runtime_to_provider = HashMap::new();
        let mut provider_to_runtime = HashMap::new();
        for tool in request
            .input
            .tools
            .iter()
            .chain(&request.input.deferred_tools)
        {
            if runtime_to_provider.contains_key(&tool.name) {
                continue;
            }
            let provider_name = provider_tool_alias(&tool.name);
            if let Some(existing) = provider_to_runtime.get(&provider_name) {
                if existing != &tool.name {
                    return Err(GatewayApiError::new(
                        StatusCode::BAD_REQUEST,
                        request.request_id.clone(),
                        "invalid_turn_request",
                        "Tool aliases are not unique for this request",
                        false,
                    ));
                }
            }
            runtime_to_provider.insert(tool.name.clone(), provider_name.clone());
            provider_to_runtime.insert(provider_name, tool.name.clone());
        }
        for message in &request.input.messages {
            let historical_names = message
                .get("toolCalls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|call| call.get("name").and_then(Value::as_str))
                .chain(message.get("toolName").and_then(Value::as_str));
            for historical_name in historical_names {
                let provider_name = provider_tool_alias(historical_name);
                if let Some(existing) = provider_to_runtime.get(&provider_name) {
                    if existing != historical_name {
                        return Err(GatewayApiError::new(
                            StatusCode::BAD_REQUEST,
                            request.request_id.clone(),
                            "invalid_turn_request",
                            "Historical tool aliases are not unique for this request",
                            false,
                        ));
                    }
                } else {
                    provider_to_runtime.insert(provider_name, historical_name.to_string());
                }
            }
        }
        Ok(Self {
            runtime_to_provider,
            provider_to_runtime,
        })
    }

    fn provider_name(&self, runtime_name: &str) -> Option<&str> {
        self.runtime_to_provider
            .get(runtime_name)
            .map(String::as_str)
    }

    fn runtime_name<'a>(&'a self, provider_name: &'a str) -> Option<&'a str> {
        self.provider_to_runtime
            .get(provider_name)
            .map(String::as_str)
            .or_else(|| {
                self.runtime_to_provider
                    .contains_key(provider_name)
                    .then_some(provider_name)
            })
            .or_else(|| {
                let compatible_name = provider_name
                    .split_once("__")
                    .filter(|(_, suffix)| {
                        (8..=64).contains(&suffix.len())
                            && suffix
                                .chars()
                                .all(|character| character.is_ascii_hexdigit())
                    })
                    .map(|(prefix, _)| prefix)
                    .unwrap_or(provider_name);
                let mut matches = self
                    .runtime_to_provider
                    .keys()
                    .filter(|runtime_name| provider_tool_prefix(runtime_name) == compatible_name);
                let candidate = matches.next()?;
                matches.next().is_none().then_some(candidate.as_str())
            })
    }
}

/// Preserve a readable portion of the Runtime tool name while appending a
/// stable digest. The digest makes names that normalize to the same provider
/// prefix distinct, and the final alias remains within the common 64-character
/// OpenAI-compatible function-name limit.
fn provider_tool_alias(runtime_name: &str) -> String {
    const HASH_HEX_BYTES: usize = 16;

    let prefix = provider_tool_prefix(runtime_name);
    let digest = format!("{:x}", Sha256::digest(runtime_name.as_bytes()));
    format!("{prefix}__{}", &digest[..HASH_HEX_BYTES])
}

fn provider_tool_prefix(runtime_name: &str) -> String {
    const MAX_PREFIX_BYTES: usize = 45;

    let mut prefix = runtime_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    prefix.truncate(MAX_PREFIX_BYTES);
    let prefix = prefix.trim_matches('_');
    if prefix.is_empty() {
        "tool".to_string()
    } else {
        prefix.to_string()
    }
}

fn request_tool_names(request: &GatewayTurnRequest) -> HashSet<&str> {
    request
        .input
        .tools
        .iter()
        .chain(&request.input.deferred_tools)
        .map(|tool| tool.name.as_str())
        .collect()
}

#[derive(Debug, Clone)]
struct ProviderTurn {
    tool_calls: Vec<GatewayToolCall>,
    text: Option<String>,
    finish_reason: String,
    usage: Usage,
    provider_request_id: Option<String>,
    attempt_count: u32,
}

fn parse_openai_response(
    value: Value,
    provider_request_id: Option<String>,
    request: &GatewayTurnRequest,
    tool_aliases: &ProviderToolAliasMap,
) -> std::result::Result<ProviderTurn, GatewayApiError> {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::BAD_GATEWAY,
                request.request_id.clone(),
                "provider_response_invalid",
                "Selected provider returned no choices",
                true,
            )
            .with_provider_request_id(provider_request_id.clone())
        })?;
    let message = choice.get("message").ok_or_else(|| {
        GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_invalid",
            "Selected provider response did not include a message",
            true,
        )
        .with_provider_request_id(provider_request_id.clone())
    })?;
    let mut tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    let id = call.get("id").and_then(Value::as_str).ok_or_else(|| {
                        GatewayApiError::new(
                            StatusCode::BAD_GATEWAY,
                            request.request_id.clone(),
                            "provider_response_invalid",
                            "Selected provider returned a tool call without id",
                            true,
                        )
                    })?;
                    let provider_name = call
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            GatewayApiError::new(
                                StatusCode::BAD_GATEWAY,
                                request.request_id.clone(),
                                "provider_response_invalid",
                                "Selected provider returned a tool call without name",
                                true,
                            )
                        })?;
                    let name = tool_aliases.runtime_name(provider_name).ok_or_else(|| {
                        let returned_name = provider_name
                            .chars()
                            .filter(|character| !character.is_control())
                            .take(96)
                            .collect::<String>();
                        GatewayApiError::new(
                            StatusCode::BAD_GATEWAY,
                            request.request_id.clone(),
                            "provider_tool_policy_violation",
                            format!(
                                "Selected provider returned an unknown tool call: {returned_name}"
                            ),
                            false,
                        )
                    })?;
                    let raw_arguments = call
                        .get("function")
                        .and_then(|function| function.get("arguments"))
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    if raw_arguments.len() > MAX_JSON_STRING_BYTES {
                        return Err(GatewayApiError::new(
                            StatusCode::BAD_GATEWAY,
                            request.request_id.clone(),
                            "provider_response_too_large",
                            "Selected provider returned oversized tool arguments",
                            false,
                        ));
                    }
                    let input = serde_json::from_str::<Value>(raw_arguments).map_err(|_| {
                        GatewayApiError::new(
                            StatusCode::BAD_GATEWAY,
                            request.request_id.clone(),
                            "provider_response_invalid",
                            "Selected provider returned invalid tool arguments",
                            true,
                        )
                    })?;
                    Ok(GatewayToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        input,
                    })
                })
                .collect::<std::result::Result<Vec<_>, GatewayApiError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let allowed_tool_names = request_tool_names(request);
    let mut tool_call_ids = HashSet::new();
    let unavailable_historical_tool_requested = tool_calls
        .iter()
        .any(|call| !allowed_tool_names.contains(call.name.as_str()));
    if unavailable_historical_tool_requested {
        tool_calls.clear();
    }
    if tool_calls
        .iter()
        .any(|call| !tool_call_ids.insert(call.id.as_str()))
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_invalid",
            "Selected provider returned duplicate tool call ids",
            true,
        )
        .with_provider_request_id(provider_request_id.clone()));
    }
    if tool_calls
        .iter()
        .any(|call| !json_within_limits(&call.input, 0))
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_too_large",
            "Selected provider returned oversized tool call input",
            false,
        )
        .with_provider_request_id(provider_request_id.clone()));
    }
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.is_empty());
    let text = if unavailable_historical_tool_requested {
        Some(
            "[runtime_tool_policy_recovery] The requested observation tool is no longer available because its repair budget is exhausted. Use only the currently available source-mutation tools, then call preview.publish."
                .to_string(),
        )
    } else {
        text
    };
    if text
        .as_ref()
        .is_some_and(|value| value.len() > MAX_JSON_STRING_BYTES)
    {
        return Err(GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_too_large",
            "Selected provider returned oversized text",
            false,
        )
        .with_provider_request_id(provider_request_id.clone()));
    }
    if tool_calls.is_empty() && text.is_none() {
        return Err(GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_invalid",
            "Selected provider returned neither text nor tool calls",
            true,
        ));
    }
    let usage = value.get("usage").ok_or_else(|| {
        GatewayApiError::new(
            StatusCode::BAD_GATEWAY,
            request.request_id.clone(),
            "provider_response_invalid",
            "Selected provider response did not include token usage",
            true,
        )
        .with_provider_request_id(provider_request_id.clone())
    })?;
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::BAD_GATEWAY,
                request.request_id.clone(),
                "provider_response_invalid",
                "Selected provider response did not include valid input token usage",
                true,
            )
            .with_provider_request_id(provider_request_id.clone())
        })?;
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GatewayApiError::new(
                StatusCode::BAD_GATEWAY,
                request.request_id.clone(),
                "provider_response_invalid",
                "Selected provider response did not include valid output token usage",
                true,
            )
            .with_provider_request_id(provider_request_id.clone())
        })?;
    Ok(ProviderTurn {
        tool_calls,
        text,
        finish_reason: if unavailable_historical_tool_requested {
            "tool_policy_recovery".to_string()
        } else {
            choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("stop")
                .to_string()
        },
        usage: Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens: usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        },
        provider_request_id,
        attempt_count: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::State,
        http::Request,
        routing::post,
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tower::ServiceExt;

    fn resource(id: &str, endpoint: String) -> ModelResource {
        ModelResource {
            schema_version: MODEL_RESOURCE_SCHEMA.to_string(),
            id: id.to_string(),
            display_name: id.to_string(),
            kind: ModelResourceKind::OpenaiCompatible,
            enabled: true,
            revision: 1,
            endpoint: ProviderEndpoint {
                base_url: endpoint,
                chat_completions_path: "/v1/chat/completions".to_string(),
            },
            auth: ProviderAuth {
                auth_type: "bearer".to_string(),
                secret_ref: "env:PROVIDER_GATEWAY_TEST_KEY".to_string(),
            },
            physical_model: "test-model".to_string(),
            capabilities: ProviderCapabilities {
                tool_calls: true,
                strict_tool_schema: true,
                streaming: false,
                vision: false,
            },
            defaults: ModelDefaults::default(),
        }
    }

    fn policy() -> ModelSelectionPolicy {
        ModelSelectionPolicy {
            schema_version: MODEL_SELECTION_POLICY_SCHEMA.to_string(),
            id: "policy-1".to_string(),
            revision: 1,
            scope: PolicyScope {
                workspace_ids: vec!["ws-one".to_string()],
                project_ids: vec!["project-1".to_string()],
            },
            applies_to: PolicyApplicability::default(),
            candidates: vec![ModelCandidate {
                model_resource_id: "allowed".to_string(),
                priority: 10,
                weight: 100,
            }],
            automatic_switch: AutomaticSwitchPolicy::default(),
            direct_selection: DirectSelectionPolicy {
                allowed_model_resource_ids: vec!["allowed".to_string()],
            },
            limits: ModelSelectionLimits::default(),
        }
    }

    fn request(resource_id: Option<&str>) -> GatewayTurnRequest {
        GatewayTurnRequest {
            schema_version: TURN_REQUEST_SCHEMA.to_string(),
            request_id: "request-1".to_string(),
            idempotency_key: "run-1:turn-1".to_string(),
            deadline_at: (Utc::now() + chrono::Duration::minutes(2)).to_rfc3339(),
            scope: TurnScope {
                workspace_id: "ws-one".to_string(),
                project_id: "project-1".to_string(),
                run_id: "run-1".to_string(),
                turn: 1,
                phase: "build".to_string(),
                agent_profile: "website-builder".to_string(),
            },
            routing: TurnRouting {
                model_resource_id: resource_id.map(ToOwned::to_owned),
                required_capabilities: RequiredCapabilities {
                    tool_calls: true,
                    strict_tool_schema: true,
                    ..Default::default()
                },
            },
            input: TurnInput {
                system_prompt: "Build a website".to_string(),
                messages: vec![],
                tools: vec![],
                deferred_tools: vec![],
            },
        }
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            input_json_schema: None,
        }
    }

    #[test]
    fn provider_tool_aliases_are_legal_bounded_and_collision_resistant() {
        let dotted = provider_tool_alias("project.write_page");
        let underscored = provider_tool_alias("project_write_page");
        let unicode = provider_tool_alias("文档.写入");

        for alias in [&dotted, &underscored, &unicode] {
            assert!(alias.len() <= 64);
            assert!(alias.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            ));
        }
        assert_ne!(dotted, underscored);
        assert_ne!(dotted, unicode);
    }

    #[test]
    fn provider_rejection_detail_extracts_bounded_non_secret_fields() {
        let detail = provider_rejection_detail(
            r#"{"error":{"type":"invalid_request_error","code":"bad_tool_history","message":"invalid sk-example-secret tool history"}}"#,
        )
        .unwrap();

        assert!(detail.contains("type=invalid_request_error"));
        assert!(detail.contains("code=bad_tool_history"));
        assert!(detail.contains("[REDACTED_API_KEY]"));
        assert!(!detail.contains("sk-example-secret"));
        assert!(!detail.contains("example-secret"));
    }

    #[test]
    fn provider_rejection_detail_redacts_bearer_and_query_credentials() {
        let detail = provider_rejection_detail(
            r#"{"error":{"message":"Authorization: Bearer top.secret.value api_key=query-secret&mode=test"}}"#,
        )
        .unwrap();

        assert!(!detail.contains("top.secret.value"));
        assert!(!detail.contains("query-secret"));
        assert!(detail.contains("Bearer [REDACTED]"));
        assert!(detail.contains("api_key=[REDACTED]"));
    }

    #[test]
    fn provider_tool_aliases_round_trip_across_definitions_history_and_response() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("project.write_page"), tool("project_write_page")];
        turn_request.input.messages = vec![json!({
            "role": "assistant",
            "toolCalls": [{
                "id": "call-history",
                "name": "project.write_page",
                "input": { "title": "Hello" }
            }]
        })];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let dotted_alias = aliases.provider_name("project.write_page").unwrap();
        let underscored_alias = aliases.provider_name("project_write_page").unwrap();
        assert_ne!(dotted_alias, underscored_alias);

        let body = openai_request_body(
            &turn_request,
            &resource("allowed", "http://example.test".into()),
            &aliases,
        )
        .unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], dotted_alias);
        assert_eq!(body["tools"][1]["function"]["name"], underscored_alias);
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["name"],
            dotted_alias
        );

        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": dotted_alias,
                            "arguments": "{\"title\":\"Hello\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });
        let parsed = parse_openai_response(response, None, &turn_request, &aliases).unwrap();
        assert_eq!(parsed.tool_calls[0].name, "project.write_page");
        assert_eq!(parsed.tool_calls[0].input, json!({ "title": "Hello" }));
    }

    #[test]
    fn historical_tool_calls_do_not_require_the_tool_to_remain_available() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("fs.write")];
        turn_request.input.messages = vec![
            json!({
                "role": "assistant",
                "toolCalls": [{
                    "id": "call-old-read",
                    "name": "fs.read",
                    "input": { "path": "project/page.tsx" }
                }]
            }),
            json!({
                "role": "tool",
                "toolUseId": "call-old-read",
                "toolName": "fs.read",
                "content": { "text": "historical source" }
            }),
        ];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();

        let body = openai_request_body(
            &turn_request,
            &resource("allowed", "http://example.test".into()),
            &aliases,
        )
        .unwrap();

        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["name"],
            provider_tool_alias("fs.read")
        );
        assert_eq!(body["messages"][2]["tool_call_id"], "call-old-read");
    }

    #[test]
    fn unavailable_historical_tool_call_becomes_safe_text_recovery() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("fs.write")];
        turn_request.input.messages = vec![
            json!({
                "role": "assistant",
                "toolCalls": [{
                    "id": "call-old-read",
                    "name": "fs.read",
                    "input": { "path": "project/page.tsx" }
                }]
            }),
            json!({
                "role": "tool",
                "toolUseId": "call-old-read",
                "toolName": "fs.read",
                "content": { "text": "historical source" }
            }),
        ];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-unavailable-read",
                        "function": {
                            "name": provider_tool_alias("fs.read"),
                            "arguments": "{\"path\":\"project/page.tsx\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let parsed = parse_openai_response(response, None, &turn_request, &aliases).unwrap();

        assert!(parsed.tool_calls.is_empty());
        assert_eq!(parsed.finish_reason, "tool_policy_recovery");
        assert!(parsed
            .text
            .as_deref()
            .unwrap()
            .contains("Use only the currently available source-mutation tools"));
    }

    #[test]
    fn deepseek_requests_disable_stateful_thinking_for_tool_history() {
        let turn_request = request(Some("allowed"));
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let mut deepseek = resource("allowed", "https://api.deepseek.com".into());
        deepseek.physical_model = "deepseek-v4-pro".to_string();

        let body = openai_request_body(&turn_request, &deepseek, &aliases).unwrap();

        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn compacted_orphan_tool_results_get_a_minimal_preceding_assistant_call() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("fs.read"), tool("fs.list")];
        turn_request.input.messages = vec![
            json!({
                "role": "tool",
                "toolUseId": "call-read",
                "toolName": "fs.read",
                "content": {"path":"project/src/index.astro","text":"hello"}
            }),
            json!({
                "role": "tool",
                "toolUseId": "call-list",
                "toolName": "fs.list",
                "content": {"entries":[]}
            }),
            json!({"role":"assistant","content":"Continuing after compaction."}),
        ];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();

        let messages = normalize_messages(&turn_request, &aliases).unwrap();

        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call-read");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call-list");
        assert_eq!(messages[4]["role"], "assistant");
    }

    #[test]
    fn provider_response_accepts_an_exact_requested_runtime_tool_name() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("project.init")];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": "project.init",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let parsed = parse_openai_response(response, None, &turn_request, &aliases).unwrap();

        assert_eq!(parsed.tool_calls[0].name, "project.init");
    }

    #[test]
    fn provider_response_accepts_a_unique_compatible_tool_prefix() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("project.init"), tool("preview.publish")];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": "project_init",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let parsed = parse_openai_response(response, None, &turn_request, &aliases).unwrap();

        assert_eq!(parsed.tool_calls[0].name, "project.init");
    }

    #[test]
    fn provider_response_accepts_a_unique_prefix_with_a_mistyped_hash() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("content.read_source")];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": "content_read_source__31eae3fbda93af37",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let parsed = parse_openai_response(response, None, &turn_request, &aliases).unwrap();

        assert_eq!(parsed.tool_calls[0].name, "content.read_source");
    }

    #[test]
    fn provider_response_rejects_an_ambiguous_compatible_tool_prefix() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("project.write.page"), tool("project_write.page")];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": "project_write_page",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let error = parse_openai_response(response, None, &turn_request, &aliases).unwrap_err();

        assert_eq!(error.code(), "provider_tool_policy_violation");
        assert!(!error.is_retryable_upstream());
    }

    #[test]
    fn provider_response_still_rejects_an_unrequested_runtime_tool_name() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.tools = vec![tool("project.init")];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-provider",
                        "function": {
                            "name": "project.delete",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        });

        let error = parse_openai_response(response, None, &turn_request, &aliases).unwrap_err();

        assert_eq!(error.code(), "provider_tool_policy_violation");
        assert!(!error.is_retryable_upstream());
    }

    #[test]
    fn provider_retry_classification_matrix_is_fail_closed_for_policy_errors() {
        for (code, retryable, expected) in [
            ("provider_unavailable", true, true),
            ("provider_timeout", true, true),
            ("provider_rate_limited", true, true),
            ("provider_response_invalid", true, true),
            ("provider_response_invalid", false, false),
            ("provider_tool_policy_violation", true, false),
            ("provider_tool_policy_violation", false, false),
            ("provider_response_too_large", true, false),
            ("provider_response_too_large", false, false),
        ] {
            let error = GatewayApiError::new(
                StatusCode::BAD_GATEWAY,
                "classification-request",
                code,
                "low sensitivity test error",
                retryable,
            );
            assert_eq!(
                error.is_retryable_upstream(),
                expected,
                "unexpected retry classification for {code} retryable={retryable}"
            );
        }
    }

    #[test]
    fn assistant_history_preserves_tools_missing_from_the_current_request() {
        let mut turn_request = request(Some("allowed"));
        turn_request.input.messages = vec![json!({
            "role": "assistant",
            "toolCalls": [{
                "id": "call-history",
                "name": "project.write_page",
                "input": {}
            }]
        })];
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let messages = normalize_messages(&turn_request, &aliases).unwrap();
        let historical_name = messages[1]["tool_calls"][0]["function"]["name"]
            .as_str()
            .unwrap();

        assert_eq!(historical_name, provider_tool_alias("project.write_page"));
        assert_eq!(
            aliases
                .provider_to_runtime
                .get(historical_name)
                .map(String::as_str),
            Some("project.write_page")
        );
        assert!(!aliases
            .runtime_to_provider
            .contains_key("project.write_page"));
    }

    #[tokio::test]
    #[ignore = "requires DEEPSEEK_API_KEY and an approved real-provider run"]
    async fn real_deepseek_v4_pro_turn_returns_token_usage_through_gateway() {
        if env::var("RUNTIME_PROVIDER_APPROVAL_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_none()
        {
            panic!("RUNTIME_PROVIDER_APPROVAL_ID is required for a real-provider gate");
        }
        let resource = ModelResource {
            schema_version: MODEL_RESOURCE_SCHEMA.to_string(),
            id: "deepseek-v4-pro-real-gate".to_string(),
            display_name: "DeepSeek V4 Pro real gate".to_string(),
            kind: ModelResourceKind::OpenaiCompatible,
            enabled: true,
            revision: 1,
            endpoint: ProviderEndpoint {
                base_url: "https://api.deepseek.com".to_string(),
                chat_completions_path: "/v1/chat/completions".to_string(),
            },
            auth: ProviderAuth {
                auth_type: "bearer".to_string(),
                secret_ref: "env:DEEPSEEK_API_KEY".to_string(),
            },
            physical_model: "deepseek-v4-pro".to_string(),
            capabilities: ProviderCapabilities {
                tool_calls: false,
                strict_tool_schema: false,
                streaming: false,
                vision: false,
            },
            defaults: ModelDefaults::default(),
        };
        let policy = ModelSelectionPolicy {
            schema_version: MODEL_SELECTION_POLICY_SCHEMA.to_string(),
            id: "deepseek-real-gate".to_string(),
            revision: 1,
            scope: PolicyScope::default(),
            applies_to: PolicyApplicability::default(),
            candidates: vec![ModelCandidate {
                model_resource_id: resource.id.clone(),
                priority: 1,
                weight: 1,
            }],
            automatic_switch: AutomaticSwitchPolicy::default(),
            direct_selection: DirectSelectionPolicy {
                allowed_model_resource_ids: vec![resource.id.clone()],
            },
            limits: ModelSelectionLimits::default(),
        };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource.clone()],
            policies: vec![policy],
        })
        .unwrap();
        let mut turn = request(Some(&resource.id));
        turn.request_id = "real-deepseek-v4-pro-gate".to_string();
        turn.idempotency_key = "real-deepseek-v4-pro-gate:1".to_string();
        turn.routing.required_capabilities = RequiredCapabilities::default();
        turn.input = TurnInput {
            system_prompt: "Reply exactly with READY.".to_string(),
            messages: vec![json!({"role": "user", "content": "READY"})],
            tools: vec![],
            deferred_tools: vec![],
        };
        let response = service.execute_turn(turn).await.unwrap();
        assert!(response
            .text
            .as_deref()
            .is_some_and(|text| !text.is_empty()));
        assert!(response.usage.input_tokens > 0);
        assert!(response.usage.output_tokens > 0);
        assert_eq!(response.model_execution.physical_model, "deepseek-v4-pro");
        assert_eq!(response.model_execution.model_resource_id, resource.id);
        assert!(response.provider.request_id.is_some());
        eprintln!(
            "real_provider_gate model_resource={} input_tokens={} output_tokens={} provider_request_id_present={}",
            response.model_execution.model_resource_id,
            response.usage.input_tokens,
            response.usage.output_tokens,
            response.provider.request_id.is_some(),
        );
    }

    #[tokio::test]
    async fn rejects_explicit_model_resource_outside_policy_allowlist() {
        let config = GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![
                resource("allowed", "http://localhost:1".to_string()),
                resource("other", "http://localhost:1".to_string()),
            ],
            policies: vec![policy()],
        };
        let service = GatewayService::new(config).unwrap();
        let error = service
            .execute_turn(request(Some("other")))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "model_resource_not_allowed");
    }

    #[tokio::test]
    async fn returns_complete_execution_snapshot_and_reuses_idempotent_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let config = GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        };
        let service = GatewayService::new(config).unwrap();
        let first = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();
        let second = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first, second);
        assert_eq!(first.model_execution.id, "model-execution-request-1");
        assert_eq!(first.model_execution.selection_reason, "explicit_resource");
        assert!(!first.model_execution.automatic_switch.used);
        assert_eq!(
            first.provider.request_id.as_deref(),
            Some("mock-provider-response-id")
        );
    }

    #[tokio::test]
    async fn workspace_scoped_policy_overrides_a_platform_default() {
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(Arc::new(AtomicUsize::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };

        let mut platform_default = policy();
        platform_default.id = "platform-default".to_string();
        platform_default.scope.workspace_ids.clear();
        platform_default.scope.project_ids.clear();
        let mut workspace_policy = policy();
        workspace_policy.id = "workspace-override".to_string();
        workspace_policy.scope.project_ids.clear();
        workspace_policy.candidates = vec![ModelCandidate {
            model_resource_id: "workspace-model".to_string(),
            priority: 10,
            weight: 100,
        }];
        workspace_policy.direct_selection.allowed_model_resource_ids =
            vec!["workspace-model".to_string()];
        let mut turn_request = request(Some("workspace-model"));
        turn_request.idempotency_key = "workspace-policy-turn".to_string();
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![
                resource("allowed", format!("http://{address}")),
                resource("workspace-model", format!("http://{address}")),
            ],
            policies: vec![platform_default, workspace_policy],
        })
        .unwrap();

        let response = service.execute_turn(turn_request).await.unwrap();
        assert_eq!(
            response.model_execution.selection_policy_id,
            "workspace-override"
        );
        assert_eq!(
            response.model_execution.model_resource_id,
            "workspace-model"
        );
    }

    #[tokio::test]
    async fn phase_scoped_policy_selects_a_model_without_binding_the_run() {
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(Arc::new(AtomicUsize::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };

        let mut default_policy = policy();
        default_policy.id = "all-phases".to_string();
        let mut edit_policy = policy();
        edit_policy.id = "edit-only".to_string();
        edit_policy.applies_to.phases = vec!["edit".to_string()];
        edit_policy.candidates = vec![ModelCandidate {
            model_resource_id: "edit-model".to_string(),
            priority: 10,
            weight: 100,
        }];
        edit_policy.direct_selection.allowed_model_resource_ids = vec!["edit-model".to_string()];
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![
                resource("allowed", format!("http://{address}")),
                resource("edit-model", format!("http://{address}")),
            ],
            policies: vec![default_policy, edit_policy],
        })
        .unwrap();

        let mut build = request(None);
        build.idempotency_key = "build-auto-selection".to_string();
        let build_response = service.execute_turn(build).await.unwrap();
        assert_eq!(
            build_response.model_execution.selection_policy_id,
            "all-phases"
        );
        assert_eq!(build_response.model_execution.model_resource_id, "allowed");

        let mut edit = request(None);
        edit.scope.phase = "edit".to_string();
        edit.scope.agent_profile = "website-editor".to_string();
        edit.idempotency_key = "edit-auto-selection".to_string();
        let edit_response = service.execute_turn(edit).await.unwrap();
        assert_eq!(
            edit_response.model_execution.selection_policy_id,
            "edit-only"
        );
        assert_eq!(
            edit_response.model_execution.model_resource_id,
            "edit-model"
        );
    }

    #[tokio::test]
    async fn metrics_endpoint_exposes_low_cardinality_turn_and_usage_metrics() {
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(Arc::new(AtomicUsize::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        })
        .unwrap();
        let turn = request(Some("allowed"));
        let turn_body = serde_json::to_string(&turn).unwrap();
        let gateway = router(service);
        let response = gateway
            .clone()
            .oneshot(
                Request::post("/v1/agent/turn")
                    .header("content-type", "application/json")
                    .body(Body::from(turn_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let replay = gateway
            .clone()
            .oneshot(
                Request::post("/v1/agent/turn")
                    .header("content-type", "application/json")
                    .body(Body::from(turn_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let response = gateway
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("provider_gateway_turn_total"));
        assert!(body.contains("provider_gateway_upstream_attempt_total"));
        assert!(body.contains("provider_gateway_input_tokens_total"));
        assert!(body.contains("provider_gateway_output_tokens_total"));
        assert!(body.contains("provider_gateway_idempotency_hit_total{state=\"completed\"} 1"));
        assert!(body.contains("provider_gateway_circuit_state{model_resource=\"allowed\"} 0"));
        assert!(body.contains("provider_gateway_queue_depth{model_resource=\"allowed\"} 0"));
        assert!(!body.contains("project-1"));
        assert!(!body.contains("run-1"));
        assert!(!body.contains("request-1"));
    }

    #[tokio::test]
    async fn automatically_switches_only_to_an_allowed_policy_candidate() {
        let app = Router::new().route("/v1/chat/completions", post(rate_limited_then_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };

        let mut rate_limited = resource("rate-limited", format!("http://{address}"));
        rate_limited.physical_model = "rate-limited-model".to_string();
        let allowed = resource("allowed", format!("http://{address}"));
        let mut policy = policy();
        policy.candidates.insert(
            0,
            ModelCandidate {
                model_resource_id: "rate-limited".to_string(),
                priority: 1,
                weight: 100,
            },
        );
        policy.automatic_switch = AutomaticSwitchPolicy {
            enabled: true,
            allowed_reasons: vec!["provider_rate_limited".to_string()],
            max_model_switches_per_turn: 1,
        };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![rate_limited, allowed],
            policies: vec![policy],
        })
        .unwrap();

        let response = service.execute_turn(request(None)).await.unwrap();
        assert_eq!(response.model_execution.model_resource_id, "allowed");
        assert_eq!(
            response.model_execution.selection_reason,
            "automatic_switch"
        );
        assert_eq!(
            response
                .model_execution
                .automatic_switch
                .from_model_resource_id,
            Some("rate-limited".to_string())
        );
        assert_eq!(response.provider.attempt_count, 2);
    }

    #[tokio::test]
    async fn retries_a_transient_provider_failure_within_resource_limit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(unavailable_then_success))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let mut configured_resource = resource("allowed", format!("http://{address}"));
        configured_resource.defaults.max_attempts = 2;
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![configured_resource],
            policies: vec![policy()],
        })
        .unwrap();

        let response = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(response.provider.attempt_count, 2);
    }

    #[tokio::test]
    async fn retries_a_structurally_invalid_provider_response_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(invalid_response_then_success))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let mut configured_resource = resource("allowed", format!("http://{address}"));
        configured_resource.defaults.max_attempts = 2;
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![configured_resource],
            policies: vec![policy()],
        })
        .unwrap();

        let response = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(response.provider.attempt_count, 2);
    }

    #[tokio::test]
    async fn does_not_retry_an_unknown_provider_tool_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(unknown_tool_then_success))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let mut configured_resource = resource("allowed", format!("http://{address}"));
        configured_resource.defaults.max_attempts = 2;
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![configured_resource],
            policies: vec![policy()],
        })
        .unwrap();

        let error = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap_err();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(error.code(), "provider_tool_policy_violation");
        assert!(!error.is_retryable_upstream());
    }

    #[tokio::test]
    async fn preserves_provider_retry_after_for_runtime_backoff() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(always_rate_limited_with_retry_after),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        })
        .unwrap();
        let error = service
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_rate_limited");
        assert_eq!(error.retry_after_ms(), Some(2_000));
    }

    #[tokio::test]
    async fn request_deadline_clamps_the_resource_provider_timeout() {
        let app = Router::new().route("/v1/chat/completions", post(delayed_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let mut turn_request = request(Some("allowed"));
        turn_request.deadline_at = (Utc::now() + chrono::Duration::milliseconds(120)).to_rfc3339();
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        })
        .unwrap();

        let started = Instant::now();
        let error = service.execute_turn(turn_request).await.unwrap_err();
        assert_eq!(error.code(), "provider_timeout");
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[tokio::test]
    async fn opens_a_circuit_per_model_resource_revision_after_repeated_failures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(always_unavailable))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        })
        .unwrap();

        for turn in 1..=3 {
            let mut turn_request = request(Some("allowed"));
            turn_request.scope.turn = turn;
            turn_request.idempotency_key = format!("circuit-turn-{turn}");
            assert_eq!(
                service.execute_turn(turn_request).await.unwrap_err().code(),
                "provider_unavailable"
            );
        }
        let mut blocked_request = request(Some("allowed"));
        blocked_request.scope.turn = 4;
        blocked_request.idempotency_key = "circuit-turn-4".to_string();
        assert_eq!(
            service
                .execute_turn(blocked_request)
                .await
                .unwrap_err()
                .code(),
            "provider_unavailable"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn rejects_when_a_model_resource_bulkhead_is_full() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let app = Router::new()
            .route("/v1/chat/completions", post(slow_success))
            .with_state((started.clone(), release.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let mut configured_resource = resource("allowed", format!("http://{address}"));
        configured_resource.defaults.max_concurrent_requests = 1;
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![configured_resource],
            policies: vec![policy()],
        })
        .unwrap();

        let first_service = service.clone();
        let first =
            tokio::spawn(async move { first_service.execute_turn(request(Some("allowed"))).await });
        started.notified().await;
        let mut second_request = request(Some("allowed"));
        second_request.scope.turn = 2;
        second_request.idempotency_key = "bulkhead-turn-2".to_string();
        assert_eq!(
            service
                .execute_turn(second_request)
                .await
                .unwrap_err()
                .code(),
            "gateway_overloaded"
        );
        release.notify_one();
        assert!(first.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn rejects_a_project_turn_when_daily_input_quota_is_exhausted() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let first_request = request(Some("allowed"));
        let mut limited_policy = policy();
        limited_policy.limits.daily_input_tokens =
            Some(estimate_input_tokens(&first_request.input));
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![limited_policy],
        })
        .unwrap();

        service.execute_turn(first_request).await.unwrap();
        let mut second_request = request(Some("allowed"));
        second_request.scope.turn = 2;
        second_request.idempotency_key = "quota-turn-2".to_string();
        assert_eq!(
            service
                .execute_turn(second_request)
                .await
                .unwrap_err()
                .code(),
            "gateway_quota_exceeded"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn restart_reuses_persisted_result_and_execution_snapshot() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let database_path = std::env::temp_dir().join(format!(
            "provider-gateway-idempotency-{}-{}.db",
            std::process::id(),
            rand_suffix()
        ));
        let config = GatewayConfig {
            listen: default_listen(),
            database_url: Some(database_path.display().to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        };
        let first = GatewayService::new(config.clone())
            .unwrap()
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();
        let restarted = GatewayService::new(config).unwrap();
        let second = restarted
            .execute_turn(request(Some("allowed")))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first, second);
        let snapshot = restarted
            .inner
            .storage
            .lock()
            .await
            .execution_snapshot(&first.model_execution.id)
            .unwrap();
        assert_eq!(snapshot.unwrap().model_resource_id, "allowed");
        let _ = std::fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn expired_idempotency_result_is_not_reissued_to_the_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let database_path = std::env::temp_dir().join(format!(
            "provider-gateway-expired-idempotency-{}-{}.db",
            std::process::id(),
            rand_suffix()
        ));
        let config = GatewayConfig {
            listen: default_listen(),
            database_url: Some(database_path.display().to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        };
        let turn = request(Some("allowed"));
        GatewayService::new(config.clone())
            .unwrap()
            .execute_turn(turn.clone())
            .await
            .unwrap();
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .execute(
                "UPDATE turn_idempotency_records SET response_expires_at = 0",
                [],
            )
            .unwrap();

        let error = GatewayService::new(config)
            .unwrap()
            .execute_turn(turn)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::GONE);
        assert_eq!(error.code(), "idempotency_result_expired");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn separate_gateway_instances_wait_for_the_shared_idempotent_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(delayed_counted_success))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let database_path = std::env::temp_dir().join(format!(
            "provider-gateway-shared-wait-{}-{}.db",
            std::process::id(),
            rand_suffix()
        ));
        let config = GatewayConfig {
            listen: default_listen(),
            database_url: Some(database_path.display().to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        };
        let first = GatewayService::new(config.clone()).unwrap();
        let second = GatewayService::new(config).unwrap();
        let request = request(Some("allowed"));
        let first_call = tokio::spawn({
            let request = request.clone();
            async move { first.execute_turn(request).await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let second_result = second.execute_turn(request).await.unwrap();
        let first_result = first_call.await.unwrap().unwrap();
        assert_eq!(first_result, second_result);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn admin_can_store_a_model_resource_key_without_returning_it() {
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: Some("admin-token".to_string()),
            resources: vec![resource("allowed", "http://localhost:1".to_string())],
            policies: vec![policy()],
        })
        .unwrap();
        let mut created = resource("new-resource", "http://localhost:1".to_string());
        created.auth.secret_ref = "db:new-resource-key".to_string();
        let mut payload = serde_json::to_value(created).unwrap();
        payload["apiKey"] = json!("not-returned-or-persisted-in-resource");
        let payload = payload.to_string();
        let request = || {
            Request::post("/internal/provider-gateway/admin/v1/model-resources")
                .header("authorization", "Bearer admin-token")
                .header("idempotency-key", "admin-create-resource-1")
                .header("x-operator-id", "platform-admin-1")
                .header("x-change-reason", "add tested model resource")
                .header("x-change-reference", "CHG-123")
                .header("content-type", "application/json")
                .body(Body::from(payload.clone()))
                .unwrap()
        };
        let app = router(service.clone());
        let response = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let replay = app.oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay_body = to_bytes(replay.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_bytes(), replay_body.as_ref());
        assert!(!body.contains("not-returned-or-persisted-in-resource"));
        assert!(!body.contains("db:new-resource-key"));
        assert!(body.contains("secretConfigured"));
        assert_eq!(
            service
                .inner
                .storage
                .lock()
                .await
                .current_model_resources()
                .unwrap()
                .into_iter()
                .find(|resource| resource.id == "new-resource")
                .unwrap()
                .revision,
            1
        );
        let read = router(service.clone())
            .oneshot(
                Request::get(
                    "/internal/provider-gateway/admin/v1/model-resources/new-resource?revision=1",
                )
                .header("authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let read_body = String::from_utf8(
            to_bytes(read.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!read_body.contains("db:new-resource-key"));
        let validation = router(service.clone())
            .oneshot(
                Request::post(
                    "/internal/provider-gateway/admin/v1/model-resources/new-resource/validate",
                )
                .header("authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(validation.status(), StatusCode::OK);
        assert_eq!(
            service
                .resolve_secret_ref("db:new-resource-key")
                .await
                .unwrap(),
            "not-returned-or-persisted-in-resource"
        );
        let encrypted = service
            .inner
            .storage
            .lock()
            .await
            .encrypted_secret("new-resource-key")
            .unwrap()
            .unwrap();
        assert!(!encrypted.contains("not-returned-or-persisted-in-resource"));
        let audit = router(service.clone())
            .oneshot(
                Request::get(
                    "/internal/provider-gateway/admin/v1/audit-events?eventType=model_resource.saved&subjectId=new-resource",
                )
                .header("authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(audit.status(), StatusCode::OK);
        let audit_body = String::from_utf8(
            to_bytes(audit.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(audit_body.contains("model_resource.saved"));
        assert!(!audit_body.contains("not-returned-or-persisted-in-resource"));
        assert!(!audit_body.contains("db:new-resource-key"));
    }

    #[tokio::test]
    async fn admin_can_activate_a_historical_policy_revision_and_keep_revisions_monotonic() {
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: Some("admin-token".to_string()),
            resources: vec![resource("allowed", "http://localhost:1".to_string())],
            policies: vec![policy()],
        })
        .unwrap();
        let revision_two = service
            .inner
            .storage
            .lock()
            .await
            .save_model_selection_policy(policy(), Some(1))
            .unwrap();
        assert_eq!(revision_two.revision, 2);

        let activation = json!({ "expectedRevision": 2, "revisionToActivate": 1 }).to_string();
        let request = || {
            Request::post(
                "/internal/provider-gateway/admin/v1/model-selection-policies/policy-1/activate",
            )
            .header("authorization", "Bearer admin-token")
            .header("idempotency-key", "activate-policy-1")
            .header("x-operator-id", "platform-admin-1")
            .header("x-change-reason", "rollback invalid provider policy")
            .header("x-change-reference", "CHG-789")
            .header("content-type", "application/json")
            .body(Body::from(activation.clone()))
            .unwrap()
        };
        let app = router(service.clone());
        let response = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let current = service
            .inner
            .storage
            .lock()
            .await
            .model_selection_policy("policy-1", None)
            .unwrap()
            .unwrap();
        assert_eq!(current.revision, 1);
        let revision_three = service
            .inner
            .storage
            .lock()
            .await
            .save_model_selection_policy(policy(), Some(1))
            .unwrap();
        assert_eq!(revision_three.revision, 3);
    }

    #[tokio::test]
    async fn admin_can_reconcile_changed_gitops_model_resources_idempotently() {
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let config_path = std::env::temp_dir().join(format!(
            "provider-gateway-gitops-{}-{}.json",
            std::process::id(),
            rand_suffix()
        ));
        let previous_config_file = env::var("PROVIDER_GATEWAY_CONFIG_FILE").ok();
        unsafe { env::set_var("PROVIDER_GATEWAY_CONFIG_FILE", &config_path) };
        let initial_resource = resource("allowed", "http://localhost:1".to_string());
        let mut desired_resource = initial_resource.clone();
        desired_resource.physical_model = "gitops-updated-model".to_string();
        std::fs::write(
            &config_path,
            serde_json::to_string(&json!({
                "resources": [desired_resource],
                "policies": [policy()],
            }))
            .unwrap(),
        )
        .unwrap();
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: Some("admin-token".to_string()),
            resources: vec![initial_resource],
            policies: vec![policy()],
        })
        .unwrap();
        let request = || {
            Request::post("/internal/provider-gateway/admin/v1/configuration/reconcile")
                .header("authorization", "Bearer admin-token")
                .header("idempotency-key", "gitops-reconcile-1")
                .header("x-operator-id", "platform-admin-1")
                .header("x-change-reason", "apply reviewed GitOps resource update")
                .header("x-change-reference", "CHG-901")
                .body(Body::empty())
                .unwrap()
        };
        let app = router(service.clone());
        let response = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("allowed@2"));
        let replay = app.oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let current = service
            .inner
            .storage
            .lock()
            .await
            .model_resource("allowed", None)
            .unwrap()
            .unwrap();
        assert_eq!(current.revision, 2);
        assert_eq!(current.physical_model, "gitops-updated-model");
        let _ = std::fs::remove_file(config_path);
        unsafe {
            if let Some(previous_config_file) = previous_config_file {
                env::set_var("PROVIDER_GATEWAY_CONFIG_FILE", previous_config_file);
            } else {
                env::remove_var("PROVIDER_GATEWAY_CONFIG_FILE");
            }
        }
    }

    #[tokio::test]
    async fn admin_readiness_probe_uses_a_minimal_control_plane_request() {
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai))
            .with_state(Arc::new(AtomicUsize::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: Some("admin-token".to_string()),
            resources: vec![resource("allowed", format!("http://{address}"))],
            policies: vec![policy()],
        })
        .unwrap();
        let response = router(service)
            .oneshot(
                Request::post(
                    "/internal/provider-gateway/admin/v1/model-resources/allowed/readiness",
                )
                .header("authorization", "Bearer admin-token")
                .header("idempotency-key", "readiness-1")
                .header("x-operator-id", "platform-admin-1")
                .header("x-change-reason", "validate provider credential")
                .header("x-change-reference", "CHG-456")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "expectedRevision": 1 }).to_string()))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("\"ready\":true"));
        assert!(!body.contains("Reply with READY"));
    }

    #[tokio::test]
    async fn readiness_requires_an_enabled_policy_resource_with_a_resolvable_secret() {
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let ready_service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", "http://localhost:1".to_string())],
            policies: vec![policy()],
        })
        .unwrap();
        assert_eq!(
            router(ready_service)
                .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let mut unavailable = resource("allowed", "http://localhost:1".to_string());
        unavailable.auth.secret_ref =
            "file:/definitely-missing-provider-gateway-test-key".to_string();
        let unavailable_service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![unavailable],
            policies: vec![policy()],
        })
        .unwrap();
        assert_eq!(
            router(unavailable_service)
                .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn draining_gateway_fails_readiness_and_rejects_new_turns() {
        unsafe { env::set_var("PROVIDER_GATEWAY_TEST_KEY", "test-key") };
        let service = GatewayService::new(GatewayConfig {
            listen: default_listen(),
            database_url: Some(":memory:".to_string()),
            runtime_bearer_token: None,
            admin_bearer_token: None,
            resources: vec![resource("allowed", "http://localhost:1".to_string())],
            policies: vec![policy()],
        })
        .unwrap();
        service.begin_shutdown();
        let app = router(service);
        assert_eq!(
            app.clone()
                .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let response = app
            .oneshot(
                Request::post("/v1/agent/turn")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&request(Some("allowed"))).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("gateway_draining"));
    }

    #[test]
    fn rejects_private_and_metadata_endpoint_hosts() {
        for host in [
            "localhost",
            "gateway.local",
            "127.0.0.1",
            "10.0.0.8",
            "172.16.0.8",
            "192.168.1.8",
            "169.254.169.254",
            "::1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(unsafe_endpoint_host(host), "expected {host} to be rejected");
        }
        assert!(!unsafe_endpoint_host("api.example.com"));
        assert!(!unsafe_endpoint_host("203.0.113.10"));
    }

    #[test]
    fn weights_distribute_equal_priority_candidates_deterministically() {
        let first = ModelCandidate {
            model_resource_id: "first".to_string(),
            priority: 10,
            weight: 1,
        };
        let second = ModelCandidate {
            model_resource_id: "second".to_string(),
            priority: 10,
            weight: 1,
        };
        let candidates = [&first, &second];
        assert_eq!(
            weighted_candidate_id(&candidates, "stable-turn-key"),
            weighted_candidate_id(&candidates, "stable-turn-key")
        );
        let selected = (0..128)
            .filter_map(|index| weighted_candidate_id(&candidates, &format!("turn-{index}")))
            .collect::<HashSet<_>>();
        assert_eq!(
            selected,
            HashSet::from(["first".to_string(), "second".to_string()])
        );
    }

    #[test]
    fn rejects_deeply_nested_turn_input_before_provider_call() {
        let mut turn_request = request(Some("allowed"));
        let mut nested = json!(null);
        for _ in 0..=MAX_JSON_DEPTH {
            nested = json!([nested]);
        }
        turn_request.input.messages = vec![nested];
        assert_eq!(
            validate_turn_request(&turn_request).unwrap_err().code(),
            "invalid_turn_request"
        );
    }

    #[test]
    fn rejects_provider_tool_call_outside_runtime_registry() {
        let turn_request = request(Some("allowed"));
        let aliases = ProviderToolAliasMap::from_request(&turn_request).unwrap();
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call-1",
                        "function": { "name": "unapproved.tool", "arguments": "{}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let error = parse_openai_response(response, None, &turn_request, &aliases).unwrap_err();
        assert_eq!(error.code(), "provider_tool_policy_violation");
        assert!(!error.is_retryable_upstream());
    }

    async fn mock_openai(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "id": "mock-provider-response-id",
            "choices": [{
                "message": { "content": "ok" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
    }

    async fn delayed_success(Json(_body): Json<Value>) -> Json<Value> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Json(json!({
            "choices": [{
                "message": { "content": "late" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
    }

    async fn delayed_counted_success(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        Json(json!({
            "choices": [{
                "message": { "content": "shared" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
    }

    async fn rate_limited_then_success(Json(body): Json<Value>) -> Response {
        if body["model"] == "rate-limited-model" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate limited" })),
            )
                .into_response();
        }
        Json(json!({
            "choices": [{
                "message": { "content": "switched" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
        .into_response()
    }

    async fn unavailable_then_success(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Json(json!({
            "choices": [{
                "message": { "content": "recovered" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
        .into_response()
    }

    async fn invalid_response_then_success(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Json(json!({
                "choices": [{
                    "message": {},
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
            }))
            .into_response();
        }
        Json(json!({
            "choices": [{
                "message": { "content": "recovered" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
        .into_response()
    }

    async fn unknown_tool_then_success(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Json(json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": "unsafe-call",
                            "function": { "name": "unrequested_tool", "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
            }))
            .into_response();
        }
        Json(json!({
            "choices": [{
                "message": { "content": "must not be reached" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
        .into_response()
    }

    async fn always_rate_limited_with_retry_after() -> Response {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "2")],
            Json(json!({ "error": "rate limited" })),
        )
            .into_response()
    }

    async fn always_unavailable(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        calls.fetch_add(1, Ordering::SeqCst);
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }

    async fn slow_success(
        State((started, release)): State<(Arc<Notify>, Arc<Notify>)>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        started.notify_one();
        release.notified().await;
        Json(json!({
            "choices": [{
                "message": { "content": "done" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }))
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
}
