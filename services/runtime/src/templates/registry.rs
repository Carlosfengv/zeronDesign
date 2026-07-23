use super::{fumadocs_docs, next_app, ManifestHash, TemplateId, TemplateSpec, TemplateVersion};
use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

pub trait TemplateRegistry: Send + Sync {
    fn default_template(&self) -> Result<Arc<TemplateSpec>, UnknownTemplate>;

    fn current(&self, id: &TemplateId) -> Result<Arc<TemplateSpec>, UnknownTemplate>;

    fn resolve_version(
        &self,
        id: &TemplateId,
        version: &TemplateVersion,
        manifest_sha256: &ManifestHash,
    ) -> Result<Arc<TemplateSpec>, IncompatibleTemplateVersion>;

    fn versions(&self, id: &TemplateId) -> Vec<TemplateVersion>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTemplate(pub TemplateId);

impl fmt::Display for UnknownTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "template is not registered: {}", self.0)
    }
}

impl Error for UnknownTemplate {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleTemplateVersion {
    pub id: TemplateId,
    pub version: TemplateVersion,
    pub manifest_sha256: ManifestHash,
}

impl fmt::Display for IncompatibleTemplateVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "template version is not registered with the requested manifest: {} {} {}",
            self.id, self.version, self.manifest_sha256
        )
    }
}

impl Error for IncompatibleTemplateVersion {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateRegistryBuildError {
    DuplicateVersion {
        id: TemplateId,
        version: TemplateVersion,
    },
    CapabilityOperationMismatch {
        id: TemplateId,
        operation: &'static str,
    },
}

impl fmt::Display for TemplateRegistryBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateVersion { id, version } => {
                write!(formatter, "duplicate template version: {id} {version}")
            }
            Self::CapabilityOperationMismatch { id, operation } => {
                write!(
                    formatter,
                    "template capability and operation support disagree: {id} {operation}"
                )
            }
        }
    }
}

impl Error for TemplateRegistryBuildError {}

#[derive(Debug, Clone)]
pub struct BuiltInTemplateRegistry {
    versions: BTreeMap<TemplateId, BTreeMap<TemplateVersion, Arc<TemplateSpec>>>,
    current: BTreeMap<TemplateId, TemplateVersion>,
}

impl BuiltInTemplateRegistry {
    pub fn new(
        specs: impl IntoIterator<Item = TemplateSpec>,
    ) -> Result<Self, TemplateRegistryBuildError> {
        let mut versions =
            BTreeMap::<TemplateId, BTreeMap<TemplateVersion, Arc<TemplateSpec>>>::new();
        let mut current = BTreeMap::new();
        for spec in specs {
            let id = spec.id.clone();
            let version = spec.version.clone();
            if spec.capabilities.structured_page_write != spec.operations.supports_render_page() {
                return Err(TemplateRegistryBuildError::CapabilityOperationMismatch {
                    id,
                    operation: "render_page",
                });
            }
            if spec.capabilities.mdx_document_write != spec.operations.supports_render_document() {
                return Err(TemplateRegistryBuildError::CapabilityOperationMismatch {
                    id,
                    operation: "render_document",
                });
            }
            let by_version = versions.entry(id.clone()).or_default();
            if by_version.contains_key(&version) {
                return Err(TemplateRegistryBuildError::DuplicateVersion { id, version });
            }
            by_version.insert(version.clone(), Arc::new(spec));
            if current
                .get(&id)
                .is_none_or(|current_version| version > *current_version)
            {
                current.insert(id, version);
            }
        }
        Ok(Self { versions, current })
    }

    pub fn built_in() -> Self {
        Self::new(vec![
            fumadocs_docs::legacy_p3_spec(),
            fumadocs_docs::legacy_p4_spec(),
            fumadocs_docs::legacy_p5_spec(),
            fumadocs_docs::legacy_p6_spec(),
            fumadocs_docs::spec(),
            next_app::legacy_spec(),
            next_app::spec(),
        ])
        .expect("built-in template registry must be valid")
    }
}

impl Default for BuiltInTemplateRegistry {
    fn default() -> Self {
        Self::built_in()
    }
}

impl TemplateRegistry for BuiltInTemplateRegistry {
    fn default_template(&self) -> Result<Arc<TemplateSpec>, UnknownTemplate> {
        let id = TemplateId::parse("next-app").expect("next-app is a valid template id");
        self.current(&id)
    }

    fn current(&self, id: &TemplateId) -> Result<Arc<TemplateSpec>, UnknownTemplate> {
        let version = self
            .current
            .get(id)
            .ok_or_else(|| UnknownTemplate(id.clone()))?;
        self.versions
            .get(id)
            .and_then(|versions| versions.get(version))
            .cloned()
            .ok_or_else(|| UnknownTemplate(id.clone()))
    }

    fn resolve_version(
        &self,
        id: &TemplateId,
        version: &TemplateVersion,
        manifest_sha256: &ManifestHash,
    ) -> Result<Arc<TemplateSpec>, IncompatibleTemplateVersion> {
        self.versions
            .get(id)
            .and_then(|versions| versions.get(version))
            .filter(|spec| spec.manifest_sha256 == *manifest_sha256)
            .cloned()
            .ok_or_else(|| IncompatibleTemplateVersion {
                id: id.clone(),
                version: version.clone(),
                manifest_sha256: manifest_sha256.clone(),
            })
    }

    fn versions(&self, id: &TemplateId) -> Vec<TemplateVersion> {
        self.versions
            .get(id)
            .map(|versions| versions.keys().cloned().collect())
            .unwrap_or_default()
    }
}
