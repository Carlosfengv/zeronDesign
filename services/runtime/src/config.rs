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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeConfig {
    pub host: String,
    pub port: u16,
    pub model_gateway_url: String,
    pub database_url: String,
    pub object_storage_url: String,
    pub runtime_storage_dir: PathBuf,
    pub k8s_namespace: String,
    pub sandbox_backend_mode: SandboxBackendMode,
    pub enable_internal_promote_api: bool,
    pub internal_admin_token: Option<String>,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            host: env::var("RUNTIME_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: env::var("RUNTIME_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8080),
            model_gateway_url: env::var("MODEL_GATEWAY_URL")
                .unwrap_or_else(|_| "http://localhost:9000".to_string()),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://runtime.db".to_string()),
            object_storage_url: env::var("OBJECT_STORAGE_URL")
                .unwrap_or_else(|_| "file:///tmp/anydesign-runtime".to_string()),
            runtime_storage_dir: env::var("RUNTIME_STORAGE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir().join("anydesign-runtime")),
            k8s_namespace: env::var("K8S_NAMESPACE")
                .unwrap_or_else(|_| "anydesign-sandboxes".to_string()),
            sandbox_backend_mode: env::var("SANDBOX_BACKEND_MODE")
                .ok()
                .map(|value| SandboxBackendMode::from_env_value(&value))
                .unwrap_or(SandboxBackendMode::Kubernetes),
            enable_internal_promote_api: env::var("ENABLE_INTERNAL_PROMOTE_API")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            internal_admin_token: env::var("RUNTIME_INTERNAL_ADMIN_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("runtime host and port should form a socket address")
    }
}
