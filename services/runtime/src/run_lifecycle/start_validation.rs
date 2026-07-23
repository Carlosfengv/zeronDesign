use super::{invalid_request, sandbox_binding_error, start::StartRunCommand, RunLifecycleError};
use crate::{conversation::RuntimeStore, types::AgentPhase};
use std::collections::HashSet;

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
        "editImpactPlanHash",
        command.input_context.edit_impact_plan_hash.as_deref(),
    )?;
    if let Some(edit_base) = command.input_context.edit_base.as_ref() {
        edit_base.validate().map_err(invalid_request)?;
    }
    optional(
        "sandboxBindingId",
        command.input_context.sandbox_binding_id.as_deref(),
    )?;
    optional(
        "parentRunId",
        command.input_context.parent_run_id.as_deref(),
    )?;
    optional(
        "predecessorRunId",
        command.input_context.predecessor_run_id.as_deref(),
    )?;
    optional(
        "continuationSnapshotId",
        command.input_context.continuation_snapshot_id.as_deref(),
    )?;
    if command.input_context.continuation_snapshot_id.is_some()
        && command.input_context.predecessor_run_id.is_none()
    {
        return Err(invalid_request(
            "continuationSnapshotId requires predecessorRunId".to_string(),
        ));
    }
    if command.input_context.parent_run_id.is_some()
        && command.input_context.predecessor_run_id.is_some()
    {
        return Err(invalid_request(
            "parentRunId and predecessorRunId are mutually exclusive".to_string(),
        ));
    }
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
    if let Some(model_resource_id) = command.input_context.model_resource_id.as_deref() {
        if model_resource_id.len() > 128
            || !model_resource_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(invalid_request(
                "modelResourceId must use only letters, digits, '-', '_', '.', or ':'".to_string(),
            ));
        }
        required("modelResourceId", model_resource_id)?;
    }
    if command.input_context.visual_bindings.len() > 16 {
        return Err(invalid_request(
            "visualBindings must contain at most 16 entries".to_string(),
        ));
    }
    let mut visual_slots = HashSet::new();
    for binding in &command.input_context.visual_bindings {
        binding.validate().map_err(invalid_request)?;
        if binding.role != crate::visual_contracts::RunVisualBindingRole::Reference {
            return Err(invalid_request(
                "StartRun visualBindings may only contain reference bindings".to_string(),
            ));
        }
        if !visual_slots.insert((binding.role, binding.order)) {
            return Err(invalid_request(
                "visualBindings role/order pairs must be unique".to_string(),
            ));
        }
    }
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
