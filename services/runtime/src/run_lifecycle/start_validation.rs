use super::{invalid_request, sandbox_binding_error, start::StartRunCommand, RunLifecycleError};
use crate::{conversation::RuntimeStore, types::AgentPhase};

pub(super) fn validate(command: &StartRunCommand) -> Result<(), RunLifecycleError> {
    required("projectId", &command.project_id)?;
    required("agentProfile", &command.agent_profile)?;
    for source in &command.input_context.content_sources {
        required("contentSources[].id", &source.id)?;
        required("contentSources[].kind", &source.kind)?;
    }
    optional("briefId", command.input_context.brief_id.as_deref())?;
    optional(
        "baseVersionId",
        command.input_context.base_version_id.as_deref(),
    )?;
    optional(
        "sandboxBindingId",
        command.input_context.sandbox_binding_id.as_deref(),
    )?;
    optional(
        "parentRunId",
        command.input_context.parent_run_id.as_deref(),
    )?;
    optional(
        "designProfileId",
        command.input_context.design_profile_id.as_deref(),
    )?;
    if command
        .input_context
        .design_fidelity_mode
        .as_deref()
        .is_some_and(|mode| !matches!(mode, "profile_only" | "source_fallback"))
    {
        return Err(invalid_request(
            "designFidelityMode must be profile_only or source_fallback".to_string(),
        ));
    }
    optional("workspaceId", command.input_context.workspace_id.as_deref())?;
    optional(
        "organizationId",
        command.input_context.organization_id.as_deref(),
    )?;
    for finding_id in &command.input_context.finding_ids {
        if finding_id.trim().is_empty() {
            return Err(invalid_request(
                "findingIds must not contain empty strings".to_string(),
            ));
        }
    }
    Ok(())
}

fn optional(field: &str, value: Option<&str>) -> Result<(), RunLifecycleError> {
    if let Some(value) = value {
        required(field, value)?;
    }
    Ok(())
}

fn required(field: &str, value: &str) -> Result<(), RunLifecycleError> {
    if value.trim().is_empty() {
        Err(invalid_request(format!("{field} must not be empty")))
    } else {
        Ok(())
    }
}

pub(super) fn sandbox_phase_requires_binding(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Repair | AgentPhase::Review | AgentPhase::Edit
    )
}

pub(super) async fn validate_openable_sandbox_binding(
    store: &RuntimeStore,
    binding_id: &str,
    allowed_parent_run_id: Option<&str>,
) -> Result<(), RunLifecycleError> {
    store
        .ensure_sandbox_binding_available(binding_id, allowed_parent_run_id)
        .await
        .map(|_| ())
        .map_err(sandbox_binding_error)
}
