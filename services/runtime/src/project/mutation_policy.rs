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

pub fn check_project_write_path(
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), ProjectMutationViolation> {
    let Some(state) = ctx.run.project_state_snapshot.as_ref() else {
        return Ok(());
    };
    let Ok(template_id) = TemplateId::parse(&state.template_key) else {
        return Ok(());
    };
    let Ok(spec) = BuiltInTemplateRegistry::built_in().current(&template_id) else {
        return Ok(());
    };
    if spec.mutation_policy.forbidden_write_roots.is_empty() {
        return Ok(());
    }
    let app_root = ctx.workspace_root.join(&state.app_root);
    let forbidden_roots = spec
        .mutation_policy
        .forbidden_write_roots
        .iter()
        .map(|relative| app_root.join(relative))
        .collect::<Vec<_>>();
    if !forbidden_roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
    {
        return Ok(());
    }
    Err(ProjectMutationViolation {
        template_id: template_id.to_string(),
        error_kind: spec.mutation_policy.error_kind,
        path: path.to_path_buf(),
        app_root,
        forbidden_roots,
        guidance: spec.mutation_policy.guidance,
    })
}
