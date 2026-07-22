use super::{
    FrameworkId, ManifestHash, SandboxExecutionProfileRef, TemplateId, TemplateOperations,
    TemplateVersion,
};
use crate::artifact_manifest::ArtifactDeliverySpec;
use crate::generation_contract::GenerationContract;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    pub argv: Vec<String>,
    pub timeout_ms: u64,
    pub success_marker: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSpec {
    pub output_directories: Vec<String>,
    pub port: u16,
    pub command: &'static str,
    pub screenshot_id: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevelopmentServerSpec {
    pub argv: Vec<String>,
    pub port: u16,
    pub readiness_path: &'static str,
    pub hmr: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceContractSpec {
    pub version: &'static str,
    pub protected_paths: &'static [&'static str],
    pub forbidden_roots: &'static [&'static str],
    pub forbidden_source_patterns: &'static [&'static str],
    pub forbidden_import_prefixes: &'static [&'static str],
}

impl SourceContractSpec {
    pub const OPEN: Self = Self {
        version: "source-contract@legacy",
        protected_paths: &[],
        forbidden_roots: &[],
        forbidden_source_patterns: &[],
        forbidden_import_prefixes: &[],
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyPolicyRef {
    pub version: &'static str,
    pub catalogs: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentRegistryRef {
    pub version: &'static str,
    pub protocol: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationContractSpec {
    pub version: &'static str,
    pub static_export_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateCapabilities {
    pub structured_page_write: bool,
    pub mdx_document_write: bool,
    pub static_export: bool,
    pub supported_component_roles: &'static [&'static str],
    pub supported_craft_packs: &'static [&'static str],
}

impl TemplateCapabilities {
    pub fn supports_component_role(&self, role: &str) -> bool {
        self.supported_component_roles
            .iter()
            .any(|supported| *supported == role)
    }

    pub fn supports_craft_pack(&self, pack: &str) -> bool {
        self.supported_craft_packs
            .iter()
            .any(|supported| *supported == pack)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationPolicySpec {
    pub forbidden_write_roots: &'static [&'static str],
    pub protected_write_paths: &'static [&'static str],
    pub error_kind: &'static str,
    pub guidance: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleTokenSpec {
    pub name: &'static str,
    pub css_variable: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleContractSpec {
    pub version: &'static str,
    pub token_file: &'static str,
    pub global_css_file: &'static str,
    pub component_root: &'static str,
    pub tailwind_version: &'static str,
    pub tailwind_entry_import: &'static str,
    pub tokens: &'static [StyleTokenSpec],
}

impl StyleContractSpec {
    pub fn render(&self, template_id: &TemplateId, app_root_relative: &std::path::Path) -> Value {
        let app_root = format!(
            "/workspace/{}",
            app_root_relative.to_string_lossy().replace('\\', "/")
        );
        let tokens = self
            .tokens
            .iter()
            .map(|token| (token.name.to_string(), json!(token.css_variable)))
            .collect::<serde_json::Map<_, _>>();
        json!({
            "version": self.version,
            "template": template_id.as_str(),
            "appRoot": app_root,
            "tokenFile": format!("{app_root}/{}", self.token_file),
            "globalCssFile": format!("{app_root}/{}", self.global_css_file),
            "componentRoot": format!("{app_root}/{}", self.component_root),
            "tokens": tokens,
            "tailwind": {
                "version": self.tailwind_version,
                "entryImport": self.tailwind_entry_import,
                "themeSource": "css-variables"
            }
        })
    }
}

impl MutationPolicySpec {
    pub const ALLOW_ALL: Self = Self {
        forbidden_write_roots: &[],
        protected_write_paths: &[],
        error_kind: "project.mutation_forbidden",
        guidance: "Choose a path allowed by the active project template.",
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateWriteMode {
    CreateOnly,
    ReplaceOnInit,
    PreserveIfPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateFileRole {
    PackageManifest,
    Lockfile,
    FrameworkConfig,
    Source,
    Style,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateFile {
    pub path: &'static str,
    pub content: &'static str,
    pub trim_final_newline: bool,
    pub role: TemplateFileRole,
    pub write_mode: TemplateWriteMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableRoute {
    pub route: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableSurfaceMetadata {
    pub primary_routes: Vec<EditableRoute>,
    pub component_roots: Vec<String>,
    pub content_roots: Vec<String>,
    pub inspection_hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableSurfaceView {
    pub primary_routes: Vec<EditableRoute>,
    pub global_style_files: Vec<String>,
    pub component_roots: Vec<String>,
    pub content_roots: Vec<String>,
    pub token_file: String,
    pub protected_paths: Vec<String>,
    pub inspection_hints: Vec<String>,
}

impl TemplateFile {
    pub fn content_for_write(&self) -> &str {
        if self.trim_final_newline {
            self.content.strip_suffix('\n').unwrap_or(self.content)
        } else {
            self.content
        }
    }
}

#[derive(Clone)]
pub struct TemplateSpec {
    pub id: TemplateId,
    pub version: TemplateVersion,
    pub manifest_sha256: ManifestHash,
    pub framework: FrameworkId,
    pub surface: &'static str,
    pub default_title: &'static str,
    pub sandbox_execution_profile: SandboxExecutionProfileRef,
    pub files: &'static [TemplateFile],
    pub inspection_files: &'static [&'static str],
    pub editable_surface: EditableSurfaceMetadata,
    pub build: BuildSpec,
    pub development_server: Option<DevelopmentServerSpec>,
    pub preview: PreviewSpec,
    pub artifact_delivery: ArtifactDeliverySpec,
    pub source_contract: SourceContractSpec,
    pub capabilities: TemplateCapabilities,
    pub mutation_policy: MutationPolicySpec,
    pub dependency_policy: DependencyPolicyRef,
    pub component_registry: Option<ComponentRegistryRef>,
    pub style: StyleContractSpec,
    pub validation_contract: ValidationContractSpec,
    pub operations: &'static dyn TemplateOperations,
}

impl std::fmt::Debug for TemplateSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TemplateSpec")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("manifest_sha256", &self.manifest_sha256)
            .field("framework", &self.framework)
            .field("surface", &self.surface)
            .field("default_title", &self.default_title)
            .field("sandbox_execution_profile", &self.sandbox_execution_profile)
            .field("files", &self.files)
            .field("inspection_files", &self.inspection_files)
            .field("editable_surface", &self.editable_surface)
            .field("build", &self.build)
            .field("development_server", &self.development_server)
            .field("preview", &self.preview)
            .field("artifact_delivery", &self.artifact_delivery)
            .field("source_contract", &self.source_contract)
            .field("capabilities", &self.capabilities)
            .field("mutation_policy", &self.mutation_policy)
            .field("dependency_policy", &self.dependency_policy)
            .field("component_registry", &self.component_registry)
            .field("style", &self.style)
            .field("validation_contract", &self.validation_contract)
            .field("operations", &self.operations.name())
            .finish()
    }
}

impl PartialEq for TemplateSpec {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.version == other.version
            && self.manifest_sha256 == other.manifest_sha256
            && self.framework == other.framework
            && self.surface == other.surface
            && self.default_title == other.default_title
            && self.sandbox_execution_profile == other.sandbox_execution_profile
            && self.files == other.files
            && self.inspection_files == other.inspection_files
            && self.editable_surface == other.editable_surface
            && self.build == other.build
            && self.development_server == other.development_server
            && self.preview == other.preview
            && self.artifact_delivery == other.artifact_delivery
            && self.source_contract == other.source_contract
            && self.capabilities == other.capabilities
            && self.mutation_policy == other.mutation_policy
            && self.dependency_policy == other.dependency_policy
            && self.component_registry == other.component_registry
            && self.style == other.style
            && self.validation_contract == other.validation_contract
            && self.operations.name() == other.operations.name()
    }
}

impl Eq for TemplateSpec {}

impl TemplateSpec {
    pub fn editable_surface_view(&self, app_root: &str) -> Result<EditableSurfaceView, String> {
        validate_relative_path(app_root, "appRoot")?;
        let prepend = |path: &str| -> Result<String, String> {
            validate_relative_path(path, "EditableSurface path")?;
            Ok(format!("{app_root}/{path}"))
        };
        let primary_routes = self
            .editable_surface
            .primary_routes
            .iter()
            .map(|route| {
                if !route.route.starts_with('/') || route.route.starts_with("//") {
                    return Err(format!("invalid editable route: {}", route.route));
                }
                Ok(EditableRoute {
                    route: route.route.clone(),
                    source: prepend(&route.source)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let protected = self
            .source_contract
            .protected_paths
            .iter()
            .chain(self.mutation_policy.protected_write_paths.iter())
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let protected_paths = protected
            .iter()
            .map(|path| prepend(path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EditableSurfaceView {
            primary_routes,
            global_style_files: vec![prepend(self.style.global_css_file)?],
            component_roots: self
                .editable_surface
                .component_roots
                .iter()
                .map(|path| prepend(path))
                .collect::<Result<Vec<_>, _>>()?,
            content_roots: self
                .editable_surface
                .content_roots
                .iter()
                .map(|path| prepend(path))
                .collect::<Result<Vec<_>, _>>()?,
            token_file: prepend(self.style.token_file)?,
            protected_paths,
            inspection_hints: self
                .editable_surface
                .inspection_hints
                .iter()
                .map(|path| prepend(path))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn generation_contract(&self) -> Result<GenerationContract, String> {
        let output_directory = self
            .preview
            .output_directories
            .first()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "template {} has no preview output directory for its generation contract",
                    self.id
                )
            })?;
        let contract = match self.surface {
            "website" => GenerationContract::website(self.id.as_str(), output_directory),
            "docs" => GenerationContract::docs(self.id.as_str(), output_directory),
            surface => {
                return Err(format!(
                    "template {} has unsupported generation surface: {surface}",
                    self.id
                ));
            }
        };
        contract.validate()?;
        Ok(contract)
    }
}

fn validate_relative_path(path: &str, label: &str) -> Result<(), String> {
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(format!("{label} must be a normalized relative path"));
    }
    Ok(())
}
