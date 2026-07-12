use super::start_validation::{sandbox_phase_requires_binding, validate_openable_sandbox_binding};
use super::{
    conflict as conflict_error, design_profile_error, internal as internal_error, not_found,
    profile_service_error, repair_run_error, sandbox_binding_error, RunLifecycleError,
    RunLifecycleOutcome, RunLifecycleService,
};
use crate::{
    config::PublicPrincipalAuthMode,
    conversation::RuntimeStore,
    project::resolve_built_in_template_for_init,
    types::{AgentEvent, AgentPhase, AgentRun, AgentRunStatus, ContentSource},
};
use chrono::Utc;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct StartRunCommand {
    pub project_id: String,
    pub phase: AgentPhase,
    pub agent_profile: String,
    pub input_context: StartRunContext,
}

#[derive(Debug, Clone, Default)]
pub struct StartRunContext {
    pub content_sources: Vec<ContentSource>,
    pub brief_id: Option<String>,
    pub base_version_id: Option<String>,
    pub sandbox_binding_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub design_profile_id: Option<String>,
    pub design_fidelity_mode: Option<String>,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    pub finding_ids: Vec<String>,
}

impl RunLifecycleService {
    pub async fn start(
        &self,
        request: StartRunCommand,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        super::start_validation::validate(&request)?;
        validate_project_access_before_initial_run(self, &request).await?;
        validate_sandbox_context(&self.store, &request).await?;
        validate_project_lifecycle_context(&self.store, &request).await?;
        let prepared = self
            .design_profiles
            .prepare_run_context(crate::design_profile_service::RunProfileContextQuery {
                project_id: &request.project_id,
                workspace_id: request.input_context.workspace_id.as_deref(),
                organization_id: request.input_context.organization_id.as_deref(),
                explicit_profile_id: request.input_context.design_profile_id.as_deref(),
                phase: request.phase,
                brief_id: request.input_context.brief_id.as_deref(),
            })
            .await
            .map_err(profile_service_error)?;
        let design_profile = prepared.profile;
        let design_profile_target = prepared.execution_target;
        let design_profile_conflict = prepared.conflict;
        let content_sources = merge_content_sources(
            inherited_build_content_sources(&self.store, &request).await,
            request.input_context.content_sources.clone(),
        );
        let run = if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
            if request.phase == AgentPhase::Repair {
                self.store
                    .create_repair_run_for_findings(
                        parent_run_id,
                        &request.input_context.finding_ids,
                        None,
                        request.agent_profile,
                        self.config.agent_model.clone(),
                    )
                    .await
                    .map_err(repair_run_error)?
            } else {
                self.store
                    .create_child_run(
                        parent_run_id,
                        request.phase,
                        request.agent_profile,
                        self.config.agent_model.clone(),
                        None,
                        request.input_context.finding_ids,
                    )
                    .await
                    .map_err(|_| not_found(format!("parent run not found: {parent_run_id}")))?
            }
        } else {
            self.store
                .create_run_with_context(
                    request.project_id,
                    request.phase,
                    request.agent_profile,
                    self.config.agent_model.clone(),
                    content_sources,
                    request.input_context.brief_id,
                    request.input_context.base_version_id,
                )
                .await
        };
        let run = if let Some(profile) = design_profile.as_ref() {
            let effective_target = if design_profile_conflict.is_none() {
                design_profile_target.as_ref()
            } else {
                None
            };
            if let Some((surface, template)) = effective_target {
                self.created_run_step(
                    &run,
                    "design_profile_attach",
                    self.store
                        .attach_run_effective_design_profile(
                            &run.id,
                            profile,
                            Some(surface),
                            Some(template),
                        )
                        .await,
                    design_profile_error,
                )
                .await?
            } else {
                self.created_run_step(
                    &run,
                    "design_profile_attach",
                    self.store.attach_run_design_profile(&run.id, profile).await,
                    design_profile_error,
                )
                .await?
            }
        } else {
            run
        };
        let run = if let Some(profile) = design_profile.as_ref() {
            let configured = self
                .created_run_step(
                    &run,
                    "design_fidelity_configure",
                    self.store
                        .configure_run_design_fidelity(
                            &run.id,
                            profile,
                            request.input_context.design_fidelity_mode.as_deref(),
                        )
                        .await,
                    design_profile_error,
                )
                .await?;
            if let Some(mode) = request.input_context.design_fidelity_mode.as_deref() {
                self.store
                    .append_audit_record(
                        &run.project_id,
                        &run.id,
                        "design_profile.fidelity_mode",
                        format!("mode={mode}"),
                        "allow",
                        "explicit StartRun input",
                    )
                    .await;
            }
            configured
        } else {
            run
        };
        if let Some(profile) = design_profile.as_ref() {
            if let Some((blocked_state, message)) =
                self.design_profiles.prebuild_failure(&run, profile).await
            {
                self.store
                    .append_conversation_item(
                        &run.project_id,
                        Some(&run.id),
                        "approval_request",
                        Some("assistant"),
                        &message,
                        Some(json!({
                            "state": blocked_state,
                            "designProfileId": profile.id,
                        })),
                    )
                    .await;
                self.store
                    .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
                    .await
                    .map_err(conflict_error)?;
                self.store
                    .append_event(AgentEvent::StateChanged {
                        run_id: run.id.clone(),
                        state: blocked_state,
                        timestamp: Utc::now(),
                    })
                    .await
                    .map_err(internal_error)?;
                return Ok(RunLifecycleOutcome {
                    run_id: run.id,
                    status: "needs_user_input".to_string(),
                });
            }
        }
        if !run.design_profile_unsupported_extended_tokens.is_empty() {
            self.store
                .append_audit_record(
                    &run.project_id,
                    &run.id,
                    "design_profile.capability_gap",
                    format!(
                        "unsupportedExtendedTokens={}",
                        run.design_profile_unsupported_extended_tokens.join(",")
                    ),
                    if run.design_profile_blocking_capability_rule_ids.is_empty() {
                        "allow"
                    } else {
                        "ask"
                    },
                    "effective profile versus template style contract",
                )
                .await;
        }
        if !run.design_profile_blocking_capability_rule_ids.is_empty() {
            self
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "approval_request",
                Some("assistant"),
                "Required DesignProfile rules depend on template capabilities that are not supported.",
                Some(json!({
                    "state": "needs_user_input:design_profile_capability_gap",
                    "ruleIds": run.design_profile_blocking_capability_rule_ids,
                    "unsupportedExtendedTokens": run.design_profile_unsupported_extended_tokens,
                })),
            )
            .await;
            self.store
                .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            self.store
                .append_event(AgentEvent::StateChanged {
                    run_id: run.id.clone(),
                    state: "needs_user_input:design_profile_capability_gap".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(RunLifecycleOutcome {
                run_id: run.id,
                status: "needs_user_input".to_string(),
            });
        }
        if let Some(conflict_reason) = design_profile_conflict {
            self.store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run.id),
                    "approval_request",
                    Some("assistant"),
                    format!("DesignProfile conflict requires confirmation: {conflict_reason}"),
                    Some(json!({
                        "reason": conflict_reason,
                        "designProfileId": run.design_profile_id.as_deref(),
                        "state": "needs_user_input:design_profile_conflict",
                    })),
                )
                .await;
            self.store
                .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            self.store
                .append_event(AgentEvent::StateChanged {
                    run_id: run.id.clone(),
                    state: "needs_user_input:design_profile_conflict".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(RunLifecycleOutcome {
                run_id: run.id,
                status: "needs_user_input".to_string(),
            });
        }
        let run =
            if let Some(sandbox_binding_id) = request.input_context.sandbox_binding_id.as_deref() {
                self.created_run_step(
                    &run,
                    "sandbox_bind",
                    self.store
                        .bind_run_to_sandbox(&run.id, sandbox_binding_id)
                        .await,
                    sandbox_binding_error,
                )
                .await?
            } else {
                run
            };
        let run = maybe_provision_build_sandbox(self, run).await?;
        if sandbox_phase_requires_binding(run.phase) && run.sandbox_id.is_some() {
            let allowed_parent_run_id = request.input_context.parent_run_id.as_deref();
            if let Err(error) = self
                .store
                .acquire_sandbox_binding_for_run(&run.id, allowed_parent_run_id)
                .await
            {
                return Err(self
                    .compensate_created_run_error(
                        &run,
                        "sandbox_exclusive_acquire",
                        sandbox_binding_error(error),
                    )
                    .await);
            }
        }
        if run.phase == AgentPhase::Edit {
            if let Err(error) = restore_edit_workspace_from_base_version(self, &run).await {
                return Err(self
                    .compensate_created_run_error(
                        &run,
                        "edit_workspace_restore",
                        conflict_error(error),
                    )
                    .await);
            }
        }
        if run.phase != AgentPhase::Edit {
            self.register_start_session(&run).await?;
        }

        Ok(RunLifecycleOutcome {
            run_id: run.id,
            status: "queued".to_string(),
        })
    }
}

async fn validate_project_access_before_initial_run(
    service: &RunLifecycleService,
    request: &StartRunCommand,
) -> Result<(), RunLifecycleError> {
    let production_auth_active = service.config.policy_profile
        == crate::config::RuntimePolicyProfile::Production
        && service.config.public_principal_auth_mode == PublicPrincipalAuthMode::Required
        && service.config.validate_startup().is_ok();
    if !production_auth_active
        || request.input_context.parent_run_id.is_some()
        || !matches!(request.phase, AgentPhase::Brief | AgentPhase::Build)
    {
        return Ok(());
    }
    let access = service
        .store
        .get_project_access(&request.project_id)
        .await
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "project access ownership must be registered before the initial run"
            ))
        })?;
    if request
        .input_context
        .workspace_id
        .as_deref()
        .is_some_and(|workspace_id| access.workspace_id.as_deref() != Some(workspace_id))
        || request
            .input_context
            .organization_id
            .as_deref()
            .is_some_and(|organization_id| {
                access.organization_id.as_deref() != Some(organization_id)
            })
    {
        return Err(conflict_error(anyhow::anyhow!(
            "project access scope identity drift detected"
        )));
    }
    Ok(())
}

async fn restore_edit_workspace_from_base_version(
    service: &RunLifecycleService,
    run: &AgentRun,
) -> anyhow::Result<()> {
    let base_version_id = run
        .base_version_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("edit run missing baseVersionId"))?;
    let version = service
        .store
        .get_project_version(base_version_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("base version not found: {base_version_id}"))?;
    let source_snapshot_uri = version.source_snapshot_uri.as_deref().ok_or_else(|| {
        anyhow::anyhow!("base version {base_version_id} is missing sourceSnapshotUri")
    })?;
    service
        .edit_workspace_restorer
        .restore(&service.store, &service.config, run, source_snapshot_uri)
        .await
}

async fn inherited_build_content_sources(
    store: &RuntimeStore,
    request: &StartRunCommand,
) -> Vec<ContentSource> {
    if request.phase != AgentPhase::Build {
        return Vec::new();
    }
    let Some(brief_id) = request.input_context.brief_id.as_deref() else {
        return Vec::new();
    };
    store
        .content_sources_for_brief(brief_id)
        .await
        .into_iter()
        .filter(|source| source.readable)
        .collect()
}

fn merge_content_sources(
    inherited: Vec<ContentSource>,
    explicit: Vec<ContentSource>,
) -> Vec<ContentSource> {
    let mut merged: Vec<ContentSource> = Vec::new();
    for source in inherited.into_iter().chain(explicit) {
        if let Some(index) = merged
            .iter()
            .position(|existing| existing.id == source.id || existing.kind == source.kind)
        {
            merged[index] = source;
        } else {
            merged.push(source);
        }
    }
    merged
}

async fn validate_sandbox_context(
    store: &RuntimeStore,
    request: &StartRunCommand,
) -> Result<(), RunLifecycleError> {
    let requested_binding = request.input_context.sandbox_binding_id.as_deref();

    if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        let parent = store
            .get_run(parent_run_id)
            .await
            .ok_or_else(|| not_found(format!("parent run not found: {parent_run_id}")))?;
        if let (Some(parent_binding), Some(requested_binding)) =
            (parent.sandbox_id.as_deref(), requested_binding)
        {
            if parent_binding != requested_binding {
                return Err(conflict_error(anyhow::anyhow!(
                    "child run must use parent sandbox binding {parent_binding}, got {requested_binding}"
                )));
            }
        }
        if sandbox_phase_requires_binding(request.phase)
            && parent.sandbox_id.is_none()
            && requested_binding.is_none()
        {
            return Err(conflict_error(anyhow::anyhow!(
                "{:?} run requires sandboxBindingId or a parent run with an existing sandbox binding",
                request.phase
            )));
        }
        let binding_to_validate = requested_binding.or(parent.sandbox_id.as_deref());
        if let Some(binding_id) = binding_to_validate {
            validate_openable_sandbox_binding(store, binding_id, Some(parent_run_id)).await?;
        }
        return Ok(());
    }

    if let Some(binding_id) = requested_binding {
        validate_openable_sandbox_binding(store, binding_id, None).await?;
    }

    if request.phase == AgentPhase::Build {
        validate_build_confirmed_brief(store, request).await?;
    }

    if sandbox_phase_requires_binding(request.phase) && requested_binding.is_none() {
        if request.phase == AgentPhase::Build {
            return Ok(());
        }
        return Err(conflict_error(anyhow::anyhow!(
            "{:?} run requires sandboxBindingId",
            request.phase
        )));
    }

    Ok(())
}

async fn validate_build_confirmed_brief(
    store: &RuntimeStore,
    request: &StartRunCommand,
) -> Result<(), RunLifecycleError> {
    let brief_id = request.input_context.brief_id.as_deref().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed briefId before generation"
        ))
    })?;
    let brief = store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    if !store.is_brief_confirmed(brief_id).await {
        return Err(conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed brief: {brief_id}"
        )));
    }
    resolve_built_in_template_for_init(&brief.recommended_template)
        .await
        .map_err(|error| conflict_error(anyhow::anyhow!(error.to_string())))?;
    Ok(())
}

async fn validate_project_lifecycle_context(
    store: &RuntimeStore,
    request: &StartRunCommand,
) -> Result<(), RunLifecycleError> {
    if is_mutable_phase(request.phase) && request.input_context.parent_run_id.is_none() {
        if let Some(active) = store
            .active_mutable_run_for_project(&request.project_id)
            .await
        {
            return Err(conflict_error(anyhow::anyhow!(
                "project {} already has active mutable run {}",
                request.project_id,
                active.id
            )));
        }
    }

    if request.phase == AgentPhase::Edit {
        let base_version_id = request
            .input_context
            .base_version_id
            .as_deref()
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires baseVersionId for lifecycle snapshot verification"
                ))
            })?;
        let current = store
            .current_project_version(&request.project_id)
            .await
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires a promoted current version for project {}",
                    request.project_id
                ))
            })?;
        if current.id != base_version_id {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run baseVersionId {base_version_id} is stale; currentVersionId is {}",
                current.id
            )));
        }
        if current.source_snapshot_uri.is_none() {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run requires sourceSnapshotUri for baseVersionId {base_version_id}"
            )));
        }
    }

    Ok(())
}

fn is_mutable_phase(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair | AgentPhase::Export
    )
}

async fn maybe_provision_build_sandbox(
    service: &RunLifecycleService,
    run: AgentRun,
) -> Result<AgentRun, RunLifecycleError> {
    if run.phase != AgentPhase::Build || run.sandbox_id.is_some() {
        return Ok(run);
    }
    let Some(brief_id) = run.brief_version.as_deref() else {
        return Ok(run);
    };
    let brief = service
        .store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let binding = match service
        .sandbox_provisioner
        .provision_ready(&service.store, &run.project_id, &brief.recommended_template)
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            return Err(service
                .compensate_created_run_error(
                    &run,
                    "sandbox_provision",
                    sandbox_binding_error(error),
                )
                .await);
        }
    };
    service
        .store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .map_err(sandbox_binding_error)
}
