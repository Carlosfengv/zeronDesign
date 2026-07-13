use serde::Serialize;
use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackendMode {
    Kubernetes,
    PhaseAContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkRuntimeBackendMode {
    ControlPlaneOnly,
    Kubernetes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkRuntimeExposureMode {
    ClusterOnly,
    Ingress,
}

impl WorkRuntimeExposureMode {
    fn from_env_value(value: &str) -> Self {
        match value {
            "ingress" | "public" => Self::Ingress,
            _ => Self::ClusterOnly,
        }
    }
}

impl WorkRuntimeBackendMode {
    fn from_env_value(value: &str) -> Self {
        match value {
            "kubernetes" | "k8s" => Self::Kubernetes,
            _ => Self::ControlPlaneOnly,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicPrincipalAuthMode {
    Required,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceChannelTlsMode {
    Required,
    DebugLoopback,
}

impl WorkspaceChannelTlsMode {
    fn from_env_value(value: &str) -> Self {
        match value {
            "debug-loopback" | "debug_loopback" | "debug" => Self::DebugLoopback,
            _ => Self::Required,
        }
    }
}

impl PublicPrincipalAuthMode {
    fn from_env_value(value: &str) -> Self {
        match value {
            "disabled" | "off" | "false" | "0" => Self::Disabled,
            _ => Self::Required,
        }
    }
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
    pub runtime_public_base_url: String,
    pub runtime_browser_proxy_bind: SocketAddr,
    pub workspace_channel_signing_key_file: Option<PathBuf>,
    pub workspace_channel_token_ttl_seconds: u64,
    pub workspace_channel_tls_mode: WorkspaceChannelTlsMode,
    pub workspace_channel_ca_file: Option<PathBuf>,
    pub workspace_channel_client_cert_file: Option<PathBuf>,
    pub workspace_channel_client_key_file: Option<PathBuf>,
    pub workspace_channel_server_san: String,
    pub public_principal_auth_mode: PublicPrincipalAuthMode,
    pub public_principal_issuer: String,
    pub public_principal_audience: String,
    pub public_principal_public_key_files: Vec<PathBuf>,
    pub public_principal_max_ttl_seconds: u64,
    pub k8s_namespace: String,
    pub work_runtime_backend_mode: WorkRuntimeBackendMode,
    pub work_runtime_exposure_mode: WorkRuntimeExposureMode,
    pub work_runtime_prober_image: Option<String>,
    pub works_base_domain: Option<String>,
    pub works_ingress_class: Option<String>,
    pub works_tls_secret_name: Option<String>,
    pub works_probe_scheme: String,
    pub works_probe_resolve: Option<SocketAddr>,
    pub works_probe_ca_file: Option<PathBuf>,
    pub sandbox_backend_mode: SandboxBackendMode,
    pub policy_profile: RuntimePolicyProfile,
    pub npm_registry: String,
    pub enable_internal_template_build_api: bool,
    pub enable_internal_promote_api: bool,
    pub internal_admin_token: Option<String>,
    pub model_streaming: bool,
    pub model_strict_tools: bool,
    pub model_request_timeout_seconds: u64,
    pub repository_commit: String,
    pub repository_dirty: bool,
    pub runtime_image_ref: Option<String>,
    pub release_base_image_digest: Option<String>,
    pub release_packager_version: Option<String>,
    pub release_registry_repository: Option<String>,
    pub release_scan_policy_version: Option<String>,
    pub release_packaging_helper_path: Option<PathBuf>,
    pub release_packaging_helper_sha256: Option<String>,
    pub release_packaging_deadline_seconds: u64,
    pub release_packager_root: Option<PathBuf>,
    pub release_packager_tools: Option<String>,
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
            runtime_public_base_url: env::var("RUNTIME_PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
                .trim_end_matches('/')
                .to_string(),
            runtime_browser_proxy_bind: env::var("RUNTIME_BROWSER_PROXY_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8081".to_string())
                .parse()
                .expect("RUNTIME_BROWSER_PROXY_BIND must be a socket address"),
            workspace_channel_signing_key_file: env::var("WORKSPACE_CHANNEL_SIGNING_KEY_FILE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            workspace_channel_token_ttl_seconds: env::var("WORKSPACE_CHANNEL_TOKEN_TTL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(60),
            workspace_channel_tls_mode: env::var("WORKSPACE_CHANNEL_TLS_MODE")
                .ok()
                .map(|value| WorkspaceChannelTlsMode::from_env_value(&value))
                .unwrap_or(WorkspaceChannelTlsMode::Required),
            workspace_channel_ca_file: optional_path_env("WORKSPACE_CHANNEL_CA_FILE"),
            workspace_channel_client_cert_file: optional_path_env(
                "WORKSPACE_CHANNEL_CLIENT_CERT_FILE",
            ),
            workspace_channel_client_key_file: optional_path_env(
                "WORKSPACE_CHANNEL_CLIENT_KEY_FILE",
            ),
            workspace_channel_server_san: env::var("WORKSPACE_CHANNEL_SERVER_SAN").unwrap_or_else(
                |_| {
                    "spiffe://anydesign.local/ns/anydesign-sandboxes/sa/anydesign-sandbox"
                        .to_string()
                },
            ),
            public_principal_auth_mode: env::var("PUBLIC_PRINCIPAL_AUTH_MODE")
                .ok()
                .map(|value| PublicPrincipalAuthMode::from_env_value(&value))
                .unwrap_or(PublicPrincipalAuthMode::Required),
            public_principal_issuer: env::var("PUBLIC_PRINCIPAL_ISSUER")
                .unwrap_or_else(|_| "anydesign-bff".to_string()),
            public_principal_audience: env::var("PUBLIC_PRINCIPAL_AUDIENCE")
                .unwrap_or_else(|_| "anydesign-runtime-public".to_string()),
            public_principal_public_key_files: env::var("PUBLIC_PRINCIPAL_PUBLIC_KEY_FILES")
                .ok()
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            public_principal_max_ttl_seconds: env::var("PUBLIC_PRINCIPAL_MAX_TTL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(120),
            k8s_namespace: env::var("K8S_NAMESPACE")
                .unwrap_or_else(|_| "anydesign-sandboxes".to_string()),
            work_runtime_backend_mode: env::var("WORK_RUNTIME_BACKEND")
                .ok()
                .map(|value| WorkRuntimeBackendMode::from_env_value(&value))
                .unwrap_or(WorkRuntimeBackendMode::ControlPlaneOnly),
            work_runtime_exposure_mode: env::var("WORK_RUNTIME_EXPOSURE")
                .ok()
                .map(|value| WorkRuntimeExposureMode::from_env_value(&value))
                .unwrap_or(WorkRuntimeExposureMode::ClusterOnly),
            work_runtime_prober_image: optional_string_env("WORK_RUNTIME_PROBER_IMAGE"),
            works_base_domain: optional_string_env("WORKS_BASE_DOMAIN"),
            works_ingress_class: optional_string_env("WORKS_INGRESS_CLASS"),
            works_tls_secret_name: optional_string_env("WORKS_TLS_SECRET_NAME"),
            works_probe_scheme: env::var("WORKS_PROBE_SCHEME")
                .unwrap_or_else(|_| "https".to_string()),
            works_probe_resolve: env::var("WORKS_PROBE_RESOLVE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    value
                        .parse()
                        .expect("WORKS_PROBE_RESOLVE must be a socket address")
                }),
            works_probe_ca_file: optional_path_env("WORKS_PROBE_CA_FILE"),
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
            enable_internal_template_build_api: truthy_env("ENABLE_INTERNAL_TEMPLATE_BUILD_API"),
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
            model_request_timeout_seconds: env::var("MODEL_REQUEST_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(180),
            repository_commit: env::var("RUNTIME_REPOSITORY_COMMIT")
                .unwrap_or_else(|_| "unknown".to_string()),
            repository_dirty: truthy_env("RUNTIME_REPOSITORY_DIRTY"),
            runtime_image_ref: secret_env("RUNTIME_IMAGE_REF"),
            release_base_image_digest: optional_string_env("RELEASE_BASE_IMAGE_DIGEST"),
            release_packager_version: optional_string_env("RELEASE_PACKAGER_VERSION"),
            release_registry_repository: optional_string_env("RELEASE_REGISTRY_REPOSITORY"),
            release_scan_policy_version: optional_string_env("RELEASE_SCAN_POLICY_VERSION"),
            release_packaging_helper_path: optional_path_env("ANYDESIGN_RELEASE_PACKAGER_HELPER"),
            release_packaging_helper_sha256: optional_string_env("RELEASE_PACKAGING_HELPER_SHA256"),
            release_packaging_deadline_seconds: env::var("RELEASE_PACKAGING_DEADLINE_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(20 * 60),
            release_packager_root: optional_path_env("ANYDESIGN_PACKAGER_ROOT"),
            release_packager_tools: optional_string_env("ANYDESIGN_PACKAGER_TOOLS"),
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("runtime host and port should form a socket address")
    }

    pub fn runtime_browser_proxy_base_url(&self) -> String {
        format!("http://{}", self.runtime_browser_proxy_bind)
    }

    pub fn validate_startup(&self) -> Result<(), String> {
        if self.work_runtime_backend_mode == WorkRuntimeBackendMode::Kubernetes {
            let prober = self.work_runtime_prober_image.as_deref().ok_or_else(|| {
                "WORK_RUNTIME_PROBER_IMAGE is required for Kubernetes work runtime".to_string()
            })?;
            if !is_digest_pinned_image(prober) {
                return Err("WORK_RUNTIME_PROBER_IMAGE must be sha256-pinned".to_string());
            }
            if self.work_runtime_exposure_mode == WorkRuntimeExposureMode::Ingress {
                for (name, value) in [
                    ("WORKS_BASE_DOMAIN", &self.works_base_domain),
                    ("WORKS_INGRESS_CLASS", &self.works_ingress_class),
                    ("WORKS_TLS_SECRET_NAME", &self.works_tls_secret_name),
                ] {
                    if value.as_deref().is_none_or(str::is_empty) {
                        return Err(format!("{name} is required for Ingress exposure"));
                    }
                }
                if self.works_probe_scheme != "https" {
                    return Err("WORKS_PROBE_SCHEME must be https".to_string());
                }
                if self
                    .works_probe_ca_file
                    .as_ref()
                    .is_some_and(|path| !path.is_file())
                {
                    return Err("WORKS_PROBE_CA_FILE does not exist".to_string());
                }
            }
        }
        if self.policy_profile == RuntimePolicyProfile::Production
            && self.npm_registry.contains("registry.npmjs.org")
        {
            return Err(
                "RUNTIME_NPM_REGISTRY must not use the public npm registry in production"
                    .to_string(),
            );
        }
        if !self.runtime_browser_proxy_bind.ip().is_loopback() {
            return Err("RUNTIME_BROWSER_PROXY_BIND must use a loopback address".to_string());
        }
        if self.sandbox_backend_mode == SandboxBackendMode::Kubernetes {
            let key_file = self
                .workspace_channel_signing_key_file
                .as_ref()
                .ok_or_else(|| {
                    "WORKSPACE_CHANNEL_SIGNING_KEY_FILE is required for Kubernetes sandbox mode"
                        .to_string()
                })?;
            if !key_file.is_file() {
                return Err(format!(
                    "workspace channel signing key does not exist: {}",
                    key_file.display()
                ));
            }
            if self.policy_profile == RuntimePolicyProfile::Production
                && self.workspace_channel_tls_mode != WorkspaceChannelTlsMode::Required
            {
                return Err("WORKSPACE_CHANNEL_TLS_MODE must be required in production".to_string());
            }
            if self.workspace_channel_tls_mode == WorkspaceChannelTlsMode::Required {
                for (name, path) in [
                    ("WORKSPACE_CHANNEL_CA_FILE", &self.workspace_channel_ca_file),
                    (
                        "WORKSPACE_CHANNEL_CLIENT_CERT_FILE",
                        &self.workspace_channel_client_cert_file,
                    ),
                    (
                        "WORKSPACE_CHANNEL_CLIENT_KEY_FILE",
                        &self.workspace_channel_client_key_file,
                    ),
                ] {
                    let path = path
                        .as_ref()
                        .ok_or_else(|| format!("{name} is required for mTLS workspace channels"))?;
                    if !path.is_file() {
                        return Err(format!("{name} does not exist: {}", path.display()));
                    }
                }
                if !self.workspace_channel_server_san.starts_with("spiffe://") {
                    return Err("WORKSPACE_CHANNEL_SERVER_SAN must be a SPIFFE URI".to_string());
                }
            }
        }
        if self.policy_profile == RuntimePolicyProfile::Production
            && self.public_principal_auth_mode != PublicPrincipalAuthMode::Required
        {
            return Err("PUBLIC_PRINCIPAL_AUTH_MODE must be required in production".to_string());
        }
        if self.public_principal_auth_mode == PublicPrincipalAuthMode::Required {
            if self.public_principal_public_key_files.is_empty() {
                return Err(
                    "PUBLIC_PRINCIPAL_PUBLIC_KEY_FILES is required when public principal auth is enabled"
                        .to_string(),
                );
            }
            for key_file in &self.public_principal_public_key_files {
                if !key_file.is_file() {
                    return Err(format!(
                        "public principal verification key does not exist: {}",
                        key_file.display()
                    ));
                }
            }
            if !(1..=300).contains(&self.public_principal_max_ttl_seconds) {
                return Err(
                    "PUBLIC_PRINCIPAL_MAX_TTL_SECONDS must be between 1 and 300".to_string()
                );
            }
        }
        if !(10..=600).contains(&self.model_request_timeout_seconds) {
            return Err("MODEL_REQUEST_TIMEOUT_SECONDS must be between 10 and 600".to_string());
        }
        let release_configuration = [
            self.release_base_image_digest.is_some(),
            self.release_packager_version.is_some(),
            self.release_registry_repository.is_some(),
            self.release_scan_policy_version.is_some(),
            self.release_packaging_helper_path.is_some(),
            self.release_packaging_helper_sha256.is_some(),
        ];
        if release_configuration.iter().any(|configured| *configured)
            && !release_configuration.iter().all(|configured| *configured)
        {
            return Err(
                "release packaging requires the complete profile and helper configuration"
                    .to_string(),
            );
        }
        if !(1..=3600).contains(&self.release_packaging_deadline_seconds) {
            return Err(
                "RELEASE_PACKAGING_DEADLINE_SECONDS must be between 1 and 3600".to_string(),
            );
        }
        Ok(())
    }
}

fn secret_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn optional_string_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn optional_path_env(name: &str) -> Option<PathBuf> {
    secret_env(name).map(PathBuf::from)
}

fn truthy_env(name: &str) -> bool {
    env::var(name)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn is_digest_pinned_image(image: &str) -> bool {
    image
        .rsplit_once("@sha256:")
        .is_some_and(|(repository, digest)| {
            !repository.is_empty()
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn phase_a_config() -> RuntimeConfig {
        let mut config = RuntimeConfig::from_env();
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        config.public_principal_public_key_files.clear();
        config
    }

    #[test]
    fn production_rejects_disabled_public_principal_auth() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::Production;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        assert_eq!(
            config.validate_startup().unwrap_err(),
            "PUBLIC_PRINCIPAL_AUTH_MODE must be required in production"
        );
    }

    #[test]
    fn required_public_principal_auth_rejects_missing_keys() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::Production;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
        assert!(config
            .validate_startup()
            .unwrap_err()
            .contains("PUBLIC_PRINCIPAL_PUBLIC_KEY_FILES is required"));
    }

    #[test]
    fn local_e2e_allows_explicitly_disabled_public_principal_auth() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::LocalE2e;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        config.validate_startup().unwrap();
    }

    #[test]
    fn release_packaging_requires_complete_profile_and_helper_configuration() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::LocalE2e;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        config.release_base_image_digest = Some(format!("sha256:{}", "a".repeat(64)));
        config.release_packager_version = None;
        config.release_registry_repository = None;
        config.release_scan_policy_version = None;
        config.release_packaging_helper_path = None;
        config.release_packaging_helper_sha256 = None;
        assert!(config
            .validate_startup()
            .unwrap_err()
            .contains("complete profile and helper"));
    }

    #[test]
    fn release_packaging_deadline_is_bounded() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::LocalE2e;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        config.release_base_image_digest = None;
        config.release_packager_version = None;
        config.release_registry_repository = None;
        config.release_scan_policy_version = None;
        config.release_packaging_helper_path = None;
        config.release_packaging_helper_sha256 = None;
        config.release_packaging_deadline_seconds = 0;
        assert!(config
            .validate_startup()
            .unwrap_err()
            .contains("RELEASE_PACKAGING_DEADLINE_SECONDS"));
    }

    #[test]
    fn kubernetes_work_runtime_requires_pinned_prober_and_complete_ingress_contract() {
        let mut config = phase_a_config();
        config.policy_profile = RuntimePolicyProfile::LocalE2e;
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        config.work_runtime_backend_mode = WorkRuntimeBackendMode::Kubernetes;
        config.work_runtime_prober_image = Some("release-prober:latest".into());
        assert!(config
            .validate_startup()
            .unwrap_err()
            .contains("sha256-pinned"));
        config.work_runtime_prober_image = Some(format!(
            "registry.example/release-prober@sha256:{}",
            "a".repeat(64)
        ));
        config.work_runtime_exposure_mode = WorkRuntimeExposureMode::Ingress;
        assert!(config
            .validate_startup()
            .unwrap_err()
            .contains("WORKS_BASE_DOMAIN"));
        config.works_base_domain = Some("works.example.test".into());
        config.works_ingress_class = Some("nginx".into());
        config.works_tls_secret_name = Some("works-wildcard-tls".into());
        config.validate_startup().unwrap();
    }
}
