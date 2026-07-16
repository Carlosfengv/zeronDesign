use super::{
    conflict, internal, not_found, RunLifecycleError, RunLifecycleOutcome, RunLifecycleService,
};
use crate::{
    design_context::{
        compile_website_design_context, frozen_run_design_context_manifest,
        DesignContextCompileOptions, ProfileCompatibilityMode, VerifierRegistry,
    },
    profile_token_sync::{
        plan_from_style_contract, resolved_target_tokens, style_contract_identity,
        ProfileTokenSyncOperation, ProfileTokenSyncOperationStatus,
    },
    project::resolve_built_in_template_for_init,
    style_contract::read_contract_token_values,
    tools::control_plane::control_plane_executor_for_config,
    types::{AgentEvent, AgentPhase, AgentRun, AgentRunStatus},
};
use chrono::Utc;
use serde_json::{json, Value};
use std::path::{Component, Path};

impl RunLifecycleService {
    /// Consumes one previously confirmed control-plane operation before the
    /// child agent session can start. The model never receives the operation
    /// id; it only sees the target DCP that is attached after this succeeds.
    pub async fn apply_confirmed_profile_token_sync(
        &self,
        operation_id: &str,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        let operation = self
            .store
            .profile_token_sync_operation(operation_id)
            .await
            .ok_or_else(|| {
                not_found(format!(
                    "profile token sync operation not found: {operation_id}"
                ))
            })?;
        if operation.status == ProfileTokenSyncOperationStatus::Applied {
            let child_run_id = operation.child_run_id.ok_or_else(|| {
                conflict(anyhow::anyhow!(
                    "applied profile token sync operation has no child run"
                ))
            })?;
            let child = self.store.get_run(&child_run_id).await.ok_or_else(|| {
                not_found(format!("profile sync child run not found: {child_run_id}"))
            })?;
            return Ok(RunLifecycleOutcome {
                run_id: child.id,
                status: profile_sync_child_status(&child.status),
            });
        }
        if !matches!(
            operation.status,
            ProfileTokenSyncOperationStatus::Confirmed
                | ProfileTokenSyncOperationStatus::RecoveryRequired
        ) {
            return Err(conflict(anyhow::anyhow!(
                "profile token sync operation is not ready to apply: {:?}",
                operation.status
            )));
        }

        let source_run = self
            .store
            .get_run(&operation.source_run_id)
            .await
            .ok_or_else(|| {
                not_found(format!("source run not found: {}", operation.source_run_id))
            })?;
        if source_run.design_context_materialization_hash.is_none()
            || source_run.design_context_style_contract_verified != Some(true)
        {
            return self
                .reject_pre_apply(
                    &operation,
                    "profile_sync_precondition_failed: source run has no verified materialized style contract",
                )
                .await;
        }
        let source_manifest = frozen_manifest_for_profile_sync(&source_run)?;
        if source_manifest.content_hash != operation.source_design_context_content_hash {
            return self
                .reject_pre_apply(&operation, "source DCP identity changed")
                .await;
        }

        let (contract, token_file_content) = match self
            .read_current_style_contract_and_tokens(&source_run)
            .await
        {
            Ok(value) => value,
            Err(error) => return self.reject_pre_apply(&operation, &format!("{error}")).await,
        };
        let (fresh_plan, fresh_identity) = match plan_from_style_contract(
            operation.plan.base.tokens.clone(),
            operation.plan.target.tokens.clone(),
            &contract,
            &token_file_content,
        ) {
            Ok(value) => value,
            Err(error) => return self.reject_pre_apply(&operation, &error).await,
        };
        let expected_before_tokens = operation.plan.current.tokens.clone();
        let mut expected_after_tokens = expected_before_tokens.clone();
        for (token, value) in resolved_target_tokens(&operation.plan).map_err(|error| {
            conflict(anyhow::anyhow!("profile_sync_precondition_failed: {error}"))
        })? {
            expected_after_tokens.insert(token, value);
        }
        let retry_from_original_current = fresh_plan.current.hash == operation.plan.current.hash
            && fresh_plan.plan_hash == operation.plan.plan_hash;
        // A process may die after `style.update_tokens` durably writes the
        // workspace but before the operation is marked Applied.  In that
        // precise RecoveryRequired window the target snapshot is evidence of
        // the same confirmed write, not a competing mutation.  Resume from
        // it without issuing a second write; every other drift remains fail
        // closed.
        let recovery_write_already_landed = operation.status
            == ProfileTokenSyncOperationStatus::RecoveryRequired
            && fresh_plan.current.tokens == expected_after_tokens;
        if fresh_identity != operation.style_contract_identity
            || (!retry_from_original_current && !recovery_write_already_landed)
        {
            return self
                .reject_pre_apply(
                    &operation,
                    "profile_sync_precondition_failed: style contract or current token snapshot changed",
                )
                .await;
        }

        let child = match self
            .ensure_profile_sync_child(&operation, &source_run, &source_manifest)
            .await
        {
            Ok(child) => child,
            Err(error) => return self.reject_pre_apply(&operation, &format!("{error}")).await,
        };
        let applying = self
            .store
            .begin_profile_token_sync_apply(operation_id, &child.id)
            .await
            .map_err(conflict)?;
        let before_tokens = expected_before_tokens;
        let targets = resolved_target_tokens(&applying.plan).map_err(|error| {
            conflict(anyhow::anyhow!("profile_sync_precondition_failed: {error}"))
        })?;
        if !recovery_write_already_landed && !targets.is_empty() {
            let execution = control_plane_executor_for_config(&self.config)
                .with_workspace_root(crate::http_api::resolved_workspace_root(
                    &self.config,
                    &child.project_id,
                ))
                .execute(
                    self.store.clone(),
                    &child.id,
                    "bootstrap:profile-sync-apply",
                    "style.update_tokens",
                    json!({ "tokens": targets }),
                )
                .await;
            if execution.result.is_error {
                return self
                    .mark_profile_sync_recovery(
                        &applying,
                        format!(
                            "style.update_tokens failed: {}",
                            execution.result.content["error"]
                                .as_str()
                                .unwrap_or("unknown error")
                        ),
                    )
                    .await;
            }
        }
        let (after_contract, after_token_file_content) =
            match self.read_current_style_contract_and_tokens(&child).await {
                Ok(value) => value,
                Err(error) => {
                    return self
                        .mark_profile_sync_recovery(&applying, error.to_string())
                        .await
                }
            };
        if style_contract_identity(&after_contract)
            .map_err(|error| conflict(anyhow::anyhow!(error)))?
            != applying.style_contract_identity
        {
            return self
                .mark_profile_sync_recovery(
                    &applying,
                    "style contract changed while profile token sync was applying".to_string(),
                )
                .await;
        }
        let after_tokens = read_contract_token_values(&after_contract, &after_token_file_content)
            .map_err(|error| conflict(anyhow::anyhow!(error)))?;
        self.store
            .complete_profile_token_sync_apply(operation_id, before_tokens, after_tokens)
            .await
            .map_err(|error| conflict(error))?;
        self.store
            .append_audit_record(
                &child.project_id,
                &child.id,
                "design_profile.sync.apply",
                format!("operationId={operation_id}"),
                "allow",
                "confirmed profile token sync applied before agent startup",
            )
            .await;
        self.store
            .append_event(AgentEvent::StateChanged {
                run_id: child.id.clone(),
                state: "queued:profile_sync_applied".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)?;
        self.register_start_session(&child).await?;
        Ok(RunLifecycleOutcome {
            run_id: child.id,
            status: "queued".to_string(),
        })
    }

    async fn ensure_profile_sync_child(
        &self,
        operation: &ProfileTokenSyncOperation,
        source_run: &AgentRun,
        source_manifest: &crate::design_context::DesignContextManifest,
    ) -> Result<AgentRun, RunLifecycleError> {
        if let Some(child_run_id) = operation.child_run_id.as_deref() {
            return self.store.get_run(child_run_id).await.ok_or_else(|| {
                not_found(format!("profile sync child run not found: {child_run_id}"))
            });
        }
        let target_profile = self
            .store
            .design_profile_versions(&operation.target_design_profile_id)
            .await
            .map_err(conflict)?
            .into_iter()
            .find(|profile| profile.version == operation.target_design_profile_version)
            .ok_or_else(|| {
                not_found(format!(
                    "target DesignProfile version not found: {}@{}",
                    operation.target_design_profile_id, operation.target_design_profile_version
                ))
            })?;
        if target_profile.status != "active" {
            return Err(conflict(anyhow::anyhow!(
                "target DesignProfile revision is not active"
            )));
        }
        if target_profile
            .project_id()
            .is_some_and(|project_id| project_id != source_run.project_id)
        {
            return Err(conflict(anyhow::anyhow!(
                "target DesignProfile is not visible to the source project"
            )));
        }
        let brief_id = source_run
            .brief_version
            .as_deref()
            .ok_or_else(|| conflict(anyhow::anyhow!("source run is missing a frozen Brief")))?;
        let brief = self
            .store
            .get_brief(brief_id)
            .await
            .ok_or_else(|| conflict(anyhow::anyhow!("source Brief not found: {brief_id}")))?;
        let template = resolve_built_in_template_for_init(&source_manifest.payload.template)
            .await
            .map_err(|error| conflict(anyhow::anyhow!(error.to_string())))?;
        let effective = target_profile
            .effective_for(
                &source_manifest.payload.surface,
                &source_manifest.payload.template,
            )
            .map_err(|error| conflict(anyhow::anyhow!(error)))?;
        if effective.effective_profile_hash != operation.target_effective_profile_hash {
            return Err(conflict(anyhow::anyhow!(
                "target DesignProfile effective hash changed after planning"
            )));
        }
        let enforcement_enabled = match self
            .store
            .get_design_context_enforcement_policy(
                &source_run.project_id,
                &target_profile.id,
                target_profile.version,
            )
            .await
        {
            Some(policy) => policy.enabled,
            None => self
                .config
                .design_context_enforcement_allowed_for(
                    &source_run.project_id,
                    &target_profile.id,
                    target_profile.version,
                )
                .map_err(|error| conflict(anyhow::anyhow!(error)))?,
        };
        let compiled = compile_website_design_context(
            &effective,
            &brief,
            &template,
            &DesignContextCompileOptions {
                expected_app_root: source_manifest.payload.expected_app_root.clone(),
                compiler_version: source_manifest.payload.compiler_version.clone(),
                enforcement_enabled,
                verification_policy: source_manifest.payload.verification_policy.clone(),
            },
        )
        .map_err(|error| conflict(anyhow::anyhow!(error)))?;
        let verification_environment = VerifierRegistry::discover_with_executables(
            self.config
                .design_context_browser_executable
                .as_deref()
                .and_then(|path| path.to_str()),
            self.config
                .design_context_browser_collector_executable
                .as_deref()
                .and_then(|path| path.to_str()),
        );
        let unavailable = verification_environment
            .missing_required_verifiers(&compiled.manifest.payload.verification_policy);
        if compiled.manifest.payload.effective_compatibility_mode
            == ProfileCompatibilityMode::Enforced
            && !unavailable.is_empty()
        {
            return Err(conflict(anyhow::anyhow!(
                "design verification unavailable for profile sync target: {}",
                unavailable.join(",")
            )));
        }

        let child = self
            .store
            .create_child_run(
                &source_run.id,
                AgentPhase::Edit,
                "edit".to_string(),
                self.config.agent_model.clone(),
                None,
                Vec::new(),
            )
            .await
            .map_err(conflict)?;
        let child = self
            .store
            .attach_run_effective_design_profile(
                &child.id,
                &target_profile,
                Some(&source_manifest.payload.surface),
                Some(&source_manifest.payload.template),
            )
            .await
            .map_err(conflict)?;
        let child = self
            .store
            .configure_run_design_fidelity(&child.id, &target_profile, None)
            .await
            .map_err(conflict)?;
        let child = self
            .store
            .attach_run_design_context(&child.id, &compiled, &verification_environment)
            .await
            .map_err(conflict)?;
        self.store
            .acquire_sandbox_binding_for_run(&child.id, Some(&source_run.id))
            .await
            .map_err(conflict)?;
        Ok(child)
    }

    async fn read_current_style_contract_and_tokens(
        &self,
        run: &AgentRun,
    ) -> Result<(Value, String), RunLifecycleError> {
        let contract_text =
            read_runtime_workspace_text(self, run, "state/style-contract.json").await?;
        let contract: Value = serde_json::from_str(&contract_text).map_err(|error| {
            conflict(anyhow::anyhow!("style contract is not valid JSON: {error}"))
        })?;
        let token_file = contract
            .get("tokenFile")
            .and_then(Value::as_str)
            .ok_or_else(|| conflict(anyhow::anyhow!("style contract is missing tokenFile")))?;
        if !is_safe_workspace_relative_path(token_file) {
            return Err(conflict(anyhow::anyhow!(
                "style contract tokenFile is not a safe workspace-relative path"
            )));
        }
        let token_file_content = read_runtime_workspace_text(self, run, token_file).await?;
        Ok((contract, token_file_content))
    }

    async fn reject_pre_apply(
        &self,
        operation: &ProfileTokenSyncOperation,
        reason: &str,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        self.store
            .reject_profile_token_sync_operation(&operation.id, reason.to_string())
            .await
            .map_err(conflict)?;
        Err(conflict(anyhow::anyhow!(reason.to_string())))
    }

    async fn mark_profile_sync_recovery(
        &self,
        operation: &ProfileTokenSyncOperation,
        reason: String,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        self.store
            .mark_profile_token_sync_recovery_required(&operation.id, reason.clone())
            .await
            .map_err(conflict)?;
        Err(conflict(anyhow::anyhow!(
            "profile_sync_recovery_required: {reason}"
        )))
    }
}

fn frozen_manifest_for_profile_sync(
    run: &AgentRun,
) -> Result<crate::design_context::DesignContextManifest, RunLifecycleError> {
    frozen_run_design_context_manifest(run)
        .map_err(|error| conflict(anyhow::anyhow!("source DCP identity is invalid: {error}")))?
        .ok_or_else(|| conflict(anyhow::anyhow!("source run has no frozen DCP manifest")))
}

async fn read_runtime_workspace_text(
    service: &RunLifecycleService,
    run: &AgentRun,
    path: &str,
) -> Result<String, RunLifecycleError> {
    let result = control_plane_executor_for_config(&service.config)
        .with_workspace_root(crate::http_api::resolved_workspace_root(
            &service.config,
            &run.project_id,
        ))
        .execute(
            service.store.clone(),
            &run.id,
            &format!("bootstrap:profile-sync-read:{path}"),
            "fs.read",
            json!({ "path": path }),
        )
        .await
        .result;
    if result.is_error {
        return Err(conflict(anyhow::anyhow!(
            "profile_sync_precondition_failed: workspace read failed for {path}: {}",
            result.content["error"].as_str().unwrap_or("unknown error")
        )));
    }
    result
        .content
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            conflict(anyhow::anyhow!(
                "profile_sync_precondition_failed: workspace read returned no text for {path}"
            ))
        })
}

fn is_safe_workspace_relative_path(path: &str) -> bool {
    !path.trim().is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn profile_sync_child_status(status: &AgentRunStatus) -> String {
    match status {
        AgentRunStatus::Queued => "queued".to_string(),
        AgentRunStatus::Running => "running".to_string(),
        AgentRunStatus::Completed => "completed".to_string(),
        AgentRunStatus::Partial => "partial".to_string(),
        AgentRunStatus::Failed => "failed".to_string(),
        AgentRunStatus::Cancelled => "cancelled".to_string(),
        AgentRunStatus::Blocked => "blocked".to_string(),
        AgentRunStatus::NeedsUserInput => "needs_user_input".to_string(),
        AgentRunStatus::Validating => "validating".to_string(),
    }
}
