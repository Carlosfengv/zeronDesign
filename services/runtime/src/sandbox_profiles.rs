use crate::templates::{
    SandboxExecutionProfileId, SandboxExecutionProfileRef, SandboxExecutionProfileVersion,
};
use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxCapabilities {
    pub node_major: u16,
    pub browser: bool,
    pub workspace_channel: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExecutionProfile {
    pub id: SandboxExecutionProfileId,
    pub version: SandboxExecutionProfileVersion,
    pub sandbox_template_name: String,
    pub warm_pool_name: String,
    pub image_ref: String,
    pub capabilities: SandboxCapabilities,
}

pub trait SandboxExecutionProfileRegistry: Send + Sync {
    fn resolve(
        &self,
        profile: &SandboxExecutionProfileRef,
    ) -> Result<Arc<SandboxExecutionProfile>, UnknownSandboxExecutionProfile>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSandboxExecutionProfile(pub SandboxExecutionProfileRef);

impl fmt::Display for UnknownSandboxExecutionProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sandbox execution profile is not registered: {} {}",
            self.0.id, self.0.version
        )
    }
}

impl Error for UnknownSandboxExecutionProfile {}

#[derive(Debug, Clone)]
pub struct BuiltInSandboxExecutionProfileRegistry {
    profiles: BTreeMap<
        (SandboxExecutionProfileId, SandboxExecutionProfileVersion),
        Arc<SandboxExecutionProfile>,
    >,
}

impl BuiltInSandboxExecutionProfileRegistry {
    pub fn built_in() -> Self {
        let profiles = [
            profile(
                "fumadocs-docs",
                "anydesign-fumadocs-docs",
                "anydesign-fumadocs-docs-pool",
            ),
            profile("next-app", "anydesign-next-app", "anydesign-next-app-pool"),
        ]
        .into_iter()
        .map(|profile| {
            (
                (profile.id.clone(), profile.version.clone()),
                Arc::new(profile),
            )
        })
        .collect();
        Self { profiles }
    }
}

impl Default for BuiltInSandboxExecutionProfileRegistry {
    fn default() -> Self {
        Self::built_in()
    }
}

impl SandboxExecutionProfileRegistry for BuiltInSandboxExecutionProfileRegistry {
    fn resolve(
        &self,
        profile: &SandboxExecutionProfileRef,
    ) -> Result<Arc<SandboxExecutionProfile>, UnknownSandboxExecutionProfile> {
        self.profiles
            .get(&(profile.id.clone(), profile.version.clone()))
            .cloned()
            .ok_or_else(|| UnknownSandboxExecutionProfile(profile.clone()))
    }
}

fn profile(id: &str, sandbox_template_name: &str, warm_pool_name: &str) -> SandboxExecutionProfile {
    SandboxExecutionProfile {
        id: SandboxExecutionProfileId::parse(id).unwrap(),
        version: SandboxExecutionProfileVersion::parse("0.1.0").unwrap(),
        sandbox_template_name: sandbox_template_name.to_string(),
        warm_pool_name: warm_pool_name.to_string(),
        image_ref: "ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0".to_string(),
        capabilities: SandboxCapabilities {
            node_major: 22,
            browser: true,
            workspace_channel: true,
        },
    }
}
