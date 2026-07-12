use super::{conflict, internal, RunLifecycleError, RunLifecycleOutcome, RunLifecycleService};
use crate::{
    profiles::edit::{self, EditIntent},
    types::{AgentEvent, AgentPhase, AgentRun, AgentRunStatus},
};
use chrono::Utc;
use serde_json::json;

impl RunLifecycleService {
    pub async fn continue_run(
        &self,
        run_id: &str,
        user_message: String,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        let run = self
            .store
            .get_run(run_id)
            .await
            .ok_or_else(|| RunLifecycleError::NotFound(format!("run not found: {run_id}")))?;
        if run.status.is_terminal() {
            return Err(RunLifecycleError::Conflict(format!(
                "run {run_id} is already terminal with status {:?}",
                run.status
            )));
        }
        self.store
            .append_conversation_item(
                &run.project_id,
                Some(run_id),
                "user_message",
                Some("user"),
                user_message.clone(),
                None,
            )
            .await;

        if run.phase == AgentPhase::Brief
            && run.status == AgentRunStatus::NeedsUserInput
            && run.brief_version.is_some()
            && is_brief_confirmation_message(&user_message)
        {
            return self.confirm_brief(run_id, &run).await;
        }
        if run.status == AgentRunStatus::Running {
            self.store.request_continue_interrupt(run_id).await;
            self.store
                .append_event(AgentEvent::StateChanged {
                    run_id: run_id.to_string(),
                    state: "running:continue_queued".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal)?;
            return Ok(outcome(run_id, "running"));
        }
        if run.phase == AgentPhase::Edit {
            if let Some(outcome) = self
                .check_edit_conflicts(run_id, &run, &user_message)
                .await?
            {
                return Ok(outcome);
            }
        }
        self.store
            .update_run_status(run_id, AgentRunStatus::Running)
            .await
            .map_err(conflict)?;
        self.store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.to_string(),
                state: "running".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)?;
        self.launch_session(run_id.to_string());
        Ok(outcome(run_id, "running"))
    }

    async fn confirm_brief(
        &self,
        run_id: &str,
        run: &AgentRun,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        let brief_id = run.brief_version.clone().expect("brief version checked");
        self.store
            .confirm_brief(run_id, &brief_id)
            .await
            .map_err(internal)?;
        self.store
            .update_run_status(run_id, AgentRunStatus::Completed)
            .await
            .map_err(conflict)?;
        self.store
            .append_event(AgentEvent::RunCompleted {
                run_id: run_id.to_string(),
                status: "completed".to_string(),
                summary: "Brief confirmed.".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)?;
        self.store
            .append_conversation_item(
                &run.project_id,
                Some(run_id),
                "run_completed",
                Some("system"),
                "Brief confirmed.",
                Some(json!({ "briefId": brief_id })),
            )
            .await;
        Ok(outcome(run_id, "completed"))
    }

    async fn check_edit_conflicts(
        &self,
        run_id: &str,
        run: &AgentRun,
        user_message: &str,
    ) -> Result<Option<RunLifecycleOutcome>, RunLifecycleError> {
        let override_accepted = run.status == AgentRunStatus::NeedsUserInput
            && run.design_profile_id.is_some()
            && is_design_profile_override_message(user_message)
            && has_design_profile_conflict_state(&self.store, run_id).await;
        if override_accepted {
            self.store
                .append_conversation_item(
                    &run.project_id,
                    Some(run_id),
                    "design_profile_override",
                    Some("user"),
                    "DesignProfile override accepted for this run.",
                    Some(json!({
                        "designProfileId": run.design_profile_id.as_deref(),
                        "designProfileVersion": run.design_profile_version,
                        "designProfileHash": run.design_profile_hash.as_deref(),
                        "decision": "override",
                        "state": "accepted",
                        "userMessage": user_message,
                    })),
                )
                .await;
        }
        if let Some(reason) =
            classify_design_profile_edit_conflict(&self.store, run, user_message).await?
        {
            self.store
                .append_conversation_item(
                    &run.project_id,
                    Some(run_id),
                    "approval_request",
                    Some("assistant"),
                    format!("DesignProfile conflict requires confirmation: {reason}"),
                    Some(json!({
                        "reason": reason,
                        "designProfileId": run.design_profile_id.as_deref(),
                        "state": "needs_user_input:design_profile_conflict",
                    })),
                )
                .await;
            self.mark_needs_input(run_id, "needs_user_input:design_profile_conflict")
                .await?;
            return Ok(Some(outcome(run_id, "needs_user_input")));
        }
        match edit::classify_edit_intent(&self.store, run, user_message)
            .await
            .map_err(internal)?
        {
            EditIntent::Compatible => Ok(None),
            EditIntent::BriefConflict { reason } => {
                self.store
                    .append_conversation_item(
                        &run.project_id,
                        Some(run_id),
                        "approval_request",
                        Some("assistant"),
                        format!("This edit may change the confirmed Brief: {reason}"),
                        Some(json!({ "reason": reason })),
                    )
                    .await;
                self.mark_needs_input(run_id, "needs_user_input:brief_conflict")
                    .await?;
                Ok(Some(outcome(run_id, "needs_user_input")))
            }
        }
    }

    async fn mark_needs_input(&self, run_id: &str, state: &str) -> Result<(), RunLifecycleError> {
        self.store
            .update_run_status(run_id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(conflict)?;
        self.store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.to_string(),
                state: state.to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal)
    }
}

fn outcome(run_id: &str, status: &str) -> RunLifecycleOutcome {
    RunLifecycleOutcome {
        run_id: run_id.to_string(),
        status: status.to_string(),
    }
}

fn is_brief_confirmation_message(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "confirm"
            | "confirmed"
            | "approve"
            | "approved"
            | "yes"
            | "ok"
            | "确认"
            | "确认 brief"
            | "确认brief"
            | "同意"
            | "可以"
            | "开始生成"
    ) {
        return true;
    }
    ["确认", "同意", "可以", "批准", "开始"]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || normalized.contains("开始生成")
        || normalized.contains("开始构建")
        || normalized.contains("开始创建")
        || (normalized.contains("confirm") && normalized.contains("brief"))
        || (normalized.contains("approve") && normalized.contains("brief"))
        || (normalized.contains("start") && normalized.contains("build"))
}

async fn classify_design_profile_edit_conflict(
    store: &crate::conversation::RuntimeStore,
    run: &AgentRun,
    user_message: &str,
) -> Result<Option<String>, RunLifecycleError> {
    if run.status == AgentRunStatus::NeedsUserInput
        && is_design_profile_override_message(user_message)
    {
        return Ok(None);
    }
    let Some(profile_id) = run.design_profile_id.as_deref() else {
        return Ok(None);
    };
    let profile = store.get_design_profile(profile_id).await.ok_or_else(|| {
        RunLifecycleError::NotFound(format!("design profile not found: {profile_id}"))
    })?;
    let normalized = user_message.to_lowercase();
    if let Some(keyword) = profile
        .visual
        .get("avoidKeywords")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .find(|keyword| normalized.contains(&keyword.to_lowercase()))
    {
        return Ok(Some(format!(
            "User edit requests visual keyword \"{keyword}\" forbidden by DesignProfile {}",
            profile.id
        )));
    }
    if let Some(claim) = profile
        .brand
        .get("messaging")
        .and_then(|messaging| messaging.get("forbiddenClaims"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .find(|claim| normalized.contains(&claim.to_lowercase()))
    {
        return Ok(Some(format!(
            "User edit requests forbidden claim \"{claim}\" from DesignProfile {}",
            profile.id
        )));
    }
    Ok(None)
}

async fn has_design_profile_conflict_state(
    store: &crate::conversation::RuntimeStore,
    run_id: &str,
) -> bool {
    store.events(run_id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    })
}

fn is_design_profile_override_message(message: &str) -> bool {
    let normalized = message.trim().to_lowercase();
    normalized.contains("override")
        || normalized.contains("temporary")
        || normalized.contains("continue anyway")
        || normalized.contains("临时覆盖")
        || normalized.contains("继续执行")
        || normalized.contains("仍然执行")
        || normalized.contains("忽略 profile")
        || normalized.contains("忽略profile")
}
