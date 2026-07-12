mod mutation_policy;
mod template_availability;
mod workspace_transaction;

pub use mutation_policy::{check_project_write_path, ProjectMutationViolation};
pub use workspace_transaction::{
    ProjectInitRecoveryOutcome, ProjectInitWorkspaceTransaction, WorkspaceTransactionError,
};

pub use template_availability::{
    BuiltInTemplateAvailabilityService, KubernetesSandboxExecutionProfileReadiness,
    SandboxExecutionProfileReadiness, StaticSandboxExecutionProfileReadiness,
    TemplateAvailabilityError, TemplateAvailabilityService, TemplateDescriptor,
};

use crate::{
    templates::{
        BuiltInTemplateRegistry, ManifestHash, TemplateId, TemplateRegistry, TemplateSpec,
        TemplateVersion,
    },
    types::ProjectRuntimeState,
};
use std::sync::{Arc, OnceLock};

pub fn built_in_template_availability() -> Arc<dyn TemplateAvailabilityService> {
    static SERVICE: OnceLock<Arc<BuiltInTemplateAvailabilityService>> = OnceLock::new();
    SERVICE
        .get_or_init(|| Arc::new(BuiltInTemplateAvailabilityService::built_in()))
        .clone()
}

pub async fn resolve_built_in_template_for_init(
    template: &str,
) -> Result<Arc<TemplateSpec>, TemplateAvailabilityError> {
    let id = TemplateId::parse(template)
        .map_err(|_| TemplateAvailabilityError::InvalidId(template.to_string()))?;
    built_in_template_availability().resolve_for_init(&id).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCompatibilityIssue {
    pub project_id: String,
    pub error_kind: &'static str,
    pub message: String,
}

pub fn audit_project_template_compatibility(
    states: &[ProjectRuntimeState],
    registry: &BuiltInTemplateRegistry,
) -> Vec<TemplateCompatibilityIssue> {
    states
        .iter()
        .filter_map(|state| audit_project_template_state(state, registry).err())
        .collect()
}

fn audit_project_template_state(
    state: &ProjectRuntimeState,
    registry: &BuiltInTemplateRegistry,
) -> Result<(), TemplateCompatibilityIssue> {
    let issue = |error_kind, message| TemplateCompatibilityIssue {
        project_id: state.project_id.clone(),
        error_kind,
        message,
    };
    let id = TemplateId::parse(&state.template_key)
        .map_err(|error| issue("template.legacy_state_ambiguous", error.to_string()))?;
    let version = TemplateVersion::parse(&state.template_version)
        .map_err(|error| issue("template.legacy_state_ambiguous", error.to_string()))?;
    if let Some(manifest) = state.template_manifest_sha256.as_deref() {
        let manifest = ManifestHash::parse(manifest)
            .map_err(|error| issue("template.version_incompatible", error.to_string()))?;
        return registry
            .resolve_version(&id, &version, &manifest)
            .map(|_| ())
            .map_err(|error| issue("template.version_incompatible", error.to_string()));
    }
    if registry.versions(&id).len() == 1
        && registry
            .current(&id)
            .is_ok_and(|current| current.version == version)
    {
        return Ok(());
    }
    Err(issue(
        "template.legacy_state_ambiguous",
        format!(
            "project requires ambiguous legacy template identity {} {}",
            id, version
        ),
    ))
}
