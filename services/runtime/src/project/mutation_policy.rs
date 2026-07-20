use crate::{
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    tools::runtime::ToolContext,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMutationViolation {
    pub template_id: String,
    pub error_kind: &'static str,
    pub path: PathBuf,
    pub app_root: PathBuf,
    pub forbidden_roots: Vec<PathBuf>,
    pub guidance: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSourceViolation {
    pub template_id: String,
    pub error_kind: &'static str,
    pub path: PathBuf,
    pub patterns: Vec<String>,
    pub guidance: &'static str,
}

pub fn check_project_write_path(
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), Box<ProjectMutationViolation>> {
    let Some(state) = ctx.run.project_state_snapshot.as_ref() else {
        return Ok(());
    };
    let Ok(template_id) = TemplateId::parse(&state.template_key) else {
        return Ok(());
    };
    let Ok(spec) = BuiltInTemplateRegistry::built_in().current(&template_id) else {
        return Ok(());
    };
    if spec.mutation_policy.forbidden_write_roots.is_empty()
        && spec.mutation_policy.protected_write_paths.is_empty()
    {
        return Ok(());
    }
    let app_root = ctx.workspace_root.join(&state.app_root);
    let forbidden_roots = spec
        .mutation_policy
        .forbidden_write_roots
        .iter()
        .map(|relative| app_root.join(relative))
        .collect::<Vec<_>>();
    let protected_paths = spec
        .mutation_policy
        .protected_write_paths
        .iter()
        .map(|relative| app_root.join(relative))
        .collect::<Vec<_>>();
    if !forbidden_roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
        && !protected_paths.iter().any(|protected| path == protected)
    {
        return Ok(());
    }
    let mut forbidden_roots = forbidden_roots;
    forbidden_roots.extend(protected_paths);
    Err(Box::new(ProjectMutationViolation {
        template_id: template_id.to_string(),
        error_kind: spec.mutation_policy.error_kind,
        path: path.to_path_buf(),
        app_root,
        forbidden_roots,
        guidance: spec.mutation_policy.guidance,
    }))
}

pub fn check_project_write_content(
    ctx: &ToolContext,
    path: &Path,
    content: &str,
) -> Result<(), Box<ProjectSourceViolation>> {
    let Some(state) = ctx.run.project_state_snapshot.as_ref() else {
        return Ok(());
    };
    let Ok(template_id) = TemplateId::parse(&state.template_key) else {
        return Ok(());
    };
    let Ok(spec) = BuiltInTemplateRegistry::built_in().current(&template_id) else {
        return Ok(());
    };
    let app_root = ctx.workspace_root.join(&state.app_root);
    if !path.starts_with(&app_root) || !is_source_file(path) {
        return Ok(());
    }
    let mut patterns = spec
        .source_contract
        .forbidden_source_patterns
        .iter()
        .filter(|pattern| content.contains(**pattern))
        .map(|pattern| (*pattern).to_string())
        .collect::<Vec<_>>();
    patterns.extend(
        spec.source_contract
            .forbidden_import_prefixes
            .iter()
            .filter(|prefix| content.contains(**prefix))
            .map(|prefix| format!("import:{prefix}")),
    );
    if patterns.is_empty() {
        return Ok(());
    }
    Err(Box::new(ProjectSourceViolation {
        template_id: template_id.to_string(),
        error_kind: "template.static_export_forbidden",
        path: path.to_path_buf(),
        patterns,
        guidance: "Remove server-only code and keep next-app compatible with static export.",
    }))
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "jsx" | "mjs" | "ts" | "tsx")
    )
}
