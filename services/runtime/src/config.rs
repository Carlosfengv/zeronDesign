use serde::Serialize;
use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackendMode {
    Kubernetes,
    PhaseAContract,
}

impl SandboxBackendMode {
    fn from_env_value(value: &str) -> Self {
        match value {
            "phase_a_contract" | "phase-a-contract" | "contract" | "local" => Self::PhaseAContract,
            _ => Self::Kubernetes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    InternalGateway,
    DeepSeek,
    KimiGlobal,
    KimiChina,
}

impl ModelProvider {
    fn from_env_value(value: &str) -> Self {
        match value {
            "deepseek" => Self::DeepSeek,
            "kimi" | "kimi_global" | "kimi-global" | "kimi_overseas" | "kimi-overseas"
            | "moonshot" | "moonshot_global" | "moonshot-global" => Self::KimiGlobal,
            "kimi_cn" | "kimi-cn" | "kimi_china" | "kimi-china" | "moonshot_cn" | "moonshot-cn"
            | "moonshot_china" | "moonshot-china" => Self::KimiChina,
            _ => Self::InternalGateway,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePolicyProfile {
    Production,
    LocalE2e,
}

impl RuntimePolicyProfile {
    pub fn from_env_value(value: &str) -> Self {
        match value {
            "local-e2e" | "local_e2e" | "local" | "e2e" => Self::LocalE2e,
            _ => Self::Production,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeConfig {
    pub host: String,
    pub port: u16,
    pub model_gateway_url: String,
    pub model_provider: ModelProvider,
    pub agent_model: String,
    pub deepseek_api_key: Option<String>,
    pub deepseek_base_url: String,
    pub kimi_api_key: Option<String>,
    pub kimi_base_url: String,
    pub kimi_cn_api_key: Option<String>,
    pub kimi_cn_base_url: String,
    pub database_url: String,
    pub object_storage_url: String,
    pub runtime_storage_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub k8s_namespace: String,
    pub sandbox_backend_mode: SandboxBackendMode,
    pub policy_profile: RuntimePolicyProfile,
    pub npm_registry: String,
    pub enable_internal_promote_api: bool,
    pub internal_admin_token: Option<String>,
    pub model_streaming: bool,
    pub model_strict_tools: bool,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        let model_provider = env::var("MODEL_PROVIDER")
            .ok()
            .map(|value| ModelProvider::from_env_value(&value))
            .unwrap_or(ModelProvider::InternalGateway);
        Self {
            host: env::var("RUNTIME_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: env::var("RUNTIME_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8080),
            model_gateway_url: env::var("MODEL_GATEWAY_URL")
                .unwrap_or_else(|_| "http://localhost:9000".to_string()),
            model_provider,
            agent_model: agent_model_from_env(model_provider),
            deepseek_api_key: secret_env("DEEPSEEK_API_KEY"),
            deepseek_base_url: env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com".to_string()),
            kimi_api_key: secret_env("KIMI_API_KEY").or_else(|| secret_env("MOONSHOT_API_KEY")),
            kimi_base_url: env::var("KIMI_BASE_URL")
                .or_else(|_| env::var("MOONSHOT_API_BASE"))
                .unwrap_or_else(|_| "https://api.moonshot.ai/v1".to_string()),
            kimi_cn_api_key: secret_env("KIMI_CN_API_KEY"),
            kimi_cn_base_url: env::var("KIMI_CN_BASE_URL")
                .unwrap_or_else(|_| "https://api.moonshot.cn/v1".to_string()),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://runtime.db".to_string()),
            object_storage_url: env::var("OBJECT_STORAGE_URL")
                .unwrap_or_else(|_| "file:///tmp/anydesign-runtime".to_string()),
            runtime_storage_dir: env::var("RUNTIME_STORAGE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir().join("anydesign-runtime")),
            workspace_root: env::var("RUNTIME_WORKSPACE_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/workspace")),
            k8s_namespace: env::var("K8S_NAMESPACE")
                .unwrap_or_else(|_| "anydesign-sandboxes".to_string()),
            sandbox_backend_mode: env::var("SANDBOX_BACKEND_MODE")
                .ok()
                .map(|value| SandboxBackendMode::from_env_value(&value))
                .unwrap_or(SandboxBackendMode::Kubernetes),
            policy_profile: env::var("RUNTIME_POLICY_PROFILE")
                .ok()
                .map(|value| RuntimePolicyProfile::from_env_value(&value))
                .unwrap_or(RuntimePolicyProfile::Production),
            npm_registry: env::var("RUNTIME_NPM_REGISTRY")
                .unwrap_or_else(|_| "https://registry.internal.example/npm/".to_string()),
            enable_internal_promote_api: env::var("ENABLE_INTERNAL_PROMOTE_API")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            internal_admin_token: env::var("RUNTIME_INTERNAL_ADMIN_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            model_streaming: env::var("MODEL_STREAMING")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            model_strict_tools: env::var("MODEL_STRICT_TOOLS")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("runtime host and port should form a socket address")
    }
}

fn secret_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn agent_model_from_env(provider: ModelProvider) -> String {
    if let Ok(value) = env::var("AGENT_MODEL").or_else(|_| env::var("MODEL_NAME")) {
        if !value.trim().is_empty() {
            return value;
        }
    }
    match provider {
        ModelProvider::InternalGateway => "internal-balanced".to_string(),
        ModelProvider::DeepSeek => env::var("DEEPSEEK_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "deepseek-chat".to_string()),
        ModelProvider::KimiGlobal => env::var("KIMI_MODEL")
            .or_else(|_| env::var("MOONSHOT_MODEL"))
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "moonshot-v1-8k".to_string()),
        ModelProvider::KimiChina => env::var("KIMI_CN_MODEL")
            .or_else(|_| env::var("KIMI_MODEL"))
            .or_else(|_| env::var("MOONSHOT_MODEL"))
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "moonshot-v1-8k".to_string()),
    }
}
