use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct RenderPageRequest {
    pub route: String,
    pub title: String,
    pub style_profile: String,
    pub sections: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct RenderDocumentRequest {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct BuildOverlayRequest {
    pub project_id: String,
    pub project_type: String,
    pub audience: String,
    pub content_hierarchy: Vec<String>,
    pub page_structure: Value,
    pub visual_direction: String,
    pub missing_information: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct SourceSnapshot {
    pub files: BTreeMap<String, Option<String>>,
    pub present_roots: BTreeSet<String>,
}

impl SourceSnapshot {
    pub fn file(&self, path: &str) -> Option<&str> {
        self.files.get(path).and_then(|text| text.as_deref())
    }

    pub fn has_root(&self, path: &str) -> bool {
        self.present_roots.contains(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceContractReport {
    pub violations: Vec<String>,
    pub error_kind: &'static str,
    pub summary: &'static str,
    pub guidance: &'static str,
}

impl SourceContractReport {
    pub fn valid() -> Self {
        Self {
            violations: Vec::new(),
            error_kind: "project.source_contract_invalid",
            summary: "Source contract invalid",
            guidance: "Repair the project source contract and retry the build.",
        }
    }

    pub fn is_valid(&self) -> bool {
        self.violations.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateOperationError {
    pub error_kind: &'static str,
    pub message: String,
}

impl TemplateOperationError {
    pub fn unsupported(operation: &str) -> Self {
        Self {
            error_kind: "template.operation_unsupported",
            message: format!("template operation is unsupported: {operation}"),
        }
    }
}

impl std::fmt::Display for TemplateOperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TemplateOperationError {}

pub trait TemplateOperations: Send + Sync {
    fn name(&self) -> &'static str;

    fn supports_render_page(&self) -> bool {
        false
    }

    fn supports_render_document(&self) -> bool {
        false
    }

    fn render_page(
        &self,
        _request: &RenderPageRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Err(TemplateOperationError::unsupported("render_page"))
    }

    fn render_document(
        &self,
        _request: &RenderDocumentRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Err(TemplateOperationError::unsupported("render_document"))
    }

    fn render_build_overlay(
        &self,
        _request: &BuildOverlayRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Ok(Vec::new())
    }

    fn validate_build_overlay(&self, _request: &BuildOverlayRequest) -> Vec<&'static str> {
        Vec::new()
    }

    fn source_contract_paths(&self) -> &'static [&'static str] {
        &[]
    }

    fn source_contract_roots(&self) -> &'static [&'static str] {
        &[]
    }

    fn validate_source(&self, _snapshot: &SourceSnapshot) -> SourceContractReport {
        SourceContractReport::valid()
    }
}
