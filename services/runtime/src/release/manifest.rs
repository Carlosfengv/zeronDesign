use crate::types::sha256_hex;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RUNTIME_MANIFEST_SCHEMA: &str = "runtime-manifest@1";
pub const STATIC_WEB_PROFILE_ID: &str = "static-web-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeHealthSpec {
    pub path: String,
    pub initial_delay_seconds: u32,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceSpec {
    pub cpu_request: String,
    pub memory_request: String,
    pub cpu_limit: String,
    pub memory_limit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeReplicaSpec {
    pub min: u16,
    pub max: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeStateSpec {
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeNetworkSpec {
    pub egress_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema_version: String,
    pub delivery_runtime: String,
    pub container_port: u16,
    pub health: RuntimeHealthSpec,
    pub resources: RuntimeResourceSpec,
    pub replicas: RuntimeReplicaSpec,
    pub state: RuntimeStateSpec,
    pub network: RuntimeNetworkSpec,
}

impl RuntimeManifest {
    pub fn static_web_v1() -> Self {
        Self {
            schema_version: RUNTIME_MANIFEST_SCHEMA.to_string(),
            delivery_runtime: "static_web_v1".to_string(),
            container_port: 8080,
            health: RuntimeHealthSpec {
                path: "/.well-known/anydesign/healthz".to_string(),
                initial_delay_seconds: 2,
                timeout_seconds: 2,
            },
            resources: RuntimeResourceSpec {
                cpu_request: "25m".to_string(),
                memory_request: "32Mi".to_string(),
                cpu_limit: "250m".to_string(),
                memory_limit: "128Mi".to_string(),
            },
            replicas: RuntimeReplicaSpec { min: 1, max: 1 },
            state: RuntimeStateSpec {
                mode: "stateless".to_string(),
            },
            network: RuntimeNetworkSpec {
                egress_policy: "deny_by_default".to_string(),
            },
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self != &Self::static_web_v1() {
            return Err(anyhow!(
                "runtime manifest is not an approved static-web-v1 profile"
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn sha256(&self) -> Result<String> {
        Ok(sha256_hex(&self.canonical_bytes()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProfile {
    pub id: String,
    pub manifest: RuntimeManifest,
    pub base_image_digest: String,
    pub packager_version: String,
    pub scan_policy_version: String,
}

impl RuntimeProfile {
    pub fn static_web_v1(
        base_image_digest: impl Into<String>,
        packager_version: impl Into<String>,
        scan_policy_version: impl Into<String>,
    ) -> Result<Self> {
        let profile = Self {
            id: STATIC_WEB_PROFILE_ID.to_string(),
            manifest: RuntimeManifest::static_web_v1(),
            base_image_digest: base_image_digest.into(),
            packager_version: packager_version.into(),
            scan_policy_version: scan_policy_version.into(),
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id != STATIC_WEB_PROFILE_ID {
            return Err(anyhow!("runtime profile id is not approved"));
        }
        self.manifest.validate()?;
        validate_digest(&self.base_image_digest)?;
        if self.packager_version.trim().is_empty() || self.scan_policy_version.trim().is_empty() {
            return Err(anyhow!("runtime profile versions must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeProfileRegistry {
    profiles: BTreeMap<String, RuntimeProfile>,
}

impl RuntimeProfileRegistry {
    pub fn new(profiles: impl IntoIterator<Item = RuntimeProfile>) -> Result<Self> {
        let mut indexed = BTreeMap::new();
        for profile in profiles {
            profile.validate()?;
            if indexed.insert(profile.id.clone(), profile).is_some() {
                return Err(anyhow!("duplicate runtime profile id"));
            }
        }
        Ok(Self { profiles: indexed })
    }

    pub fn resolve(&self, id: &str) -> Result<&RuntimeProfile> {
        self.profiles
            .get(id)
            .ok_or_else(|| anyhow!("runtime profile is not registered: {id}"))
    }
}

fn validate_digest(value: &str) -> Result<()> {
    let Some(hash) = value.strip_prefix("sha256:") else {
        return Err(anyhow!("image digest must be sha256-pinned"));
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow!("image digest is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_web_manifest_is_canonical_and_fail_closed() {
        let manifest = RuntimeManifest::static_web_v1();
        assert_eq!(manifest.sha256().unwrap().len(), 64);
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/runtime-manifest-v1.schema.json"
        ))
        .unwrap();
        let value = serde_json::to_value(&manifest).unwrap();
        for field in [
            "schemaVersion",
            "deliveryRuntime",
            "containerPort",
            "health",
            "resources",
            "replicas",
            "state",
            "network",
        ] {
            assert_eq!(schema["properties"][field]["const"], value[field]);
        }
        let mut invalid = manifest;
        invalid.container_port = 3000;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn profile_requires_a_digest_pinned_base_image() {
        assert!(RuntimeProfile::static_web_v1("nginx:latest", "packager@1", "scan@1").is_err());
        let profile = RuntimeProfile::static_web_v1(
            format!("sha256:{}", "a".repeat(64)),
            "packager@1",
            "scan@1",
        )
        .unwrap();
        let registry = RuntimeProfileRegistry::new([profile]).unwrap();
        assert_eq!(
            registry.resolve(STATIC_WEB_PROFILE_ID).unwrap().id,
            STATIC_WEB_PROFILE_ID
        );
    }
}
