use crate::{
    types::{AgentCheckpoint, AgentEvent, AgentRunStatus},
    RuntimeStore,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_REPAIR_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairActionSignature {
    pub tool: String,
    pub path: Option<String>,
    pub argv: Vec<String>,
}

impl RepairActionSignature {
    pub fn new(tool: impl Into<String>, path: Option<String>, argv: Vec<String>) -> Self {
        Self {
            tool: tool.into(),
            path,
            argv,
        }
    }

    pub fn key(&self) -> String {
        format!(
            "tool={};path={};argv={}",
            self.tool,
            self.path.as_deref().unwrap_or(""),
            self.argv.join("\u{1f}")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairAttempt {
    pub parent_run_id: String,
    pub repair_run_id: String,
    pub finding_id: String,
    pub error_key: String,
    pub action_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairLoopDecision {
    Continue {
        error_attempts: u32,
        action_attempts: u32,
    },
    Stop {
        status: AgentRunStatus,
        reason: RepairLoopStopReason,
        error_attempts: u32,
        action_attempts: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairLoopStopReason {
    MaxAttemptsForSameError,
    IdenticalActionDoomLoop,
}

pub async fn record_repair_attempt(
    store: &RuntimeStore,
    parent_run_id: &str,
    repair_run_id: &str,
    finding_id: &str,
    raw_error: &str,
    action: RepairActionSignature,
) -> Result<RepairLoopDecision> {
    let error_key = normalize_error_key(raw_error);
    if error_key.is_empty() {
        return Err(anyhow!("repair attempt requires an error key"));
    }
    store
        .record_repair_attempt(RepairAttempt {
            parent_run_id: parent_run_id.to_string(),
            repair_run_id: repair_run_id.to_string(),
            finding_id: finding_id.to_string(),
            error_key: error_key.clone(),
            action_key: action.key(),
        })
        .await?;
    let decision = evaluate_repair_loop(
        store,
        parent_run_id,
        finding_id,
        &error_key,
        &action.key(),
        DEFAULT_MAX_REPAIR_ATTEMPTS,
    )
    .await;
    if let RepairLoopDecision::Stop { status, reason, .. } = &decision {
        apply_stop_decision(store, parent_run_id, repair_run_id, *status, reason).await?;
    }
    Ok(decision)
}

pub async fn evaluate_repair_loop(
    store: &RuntimeStore,
    parent_run_id: &str,
    finding_id: &str,
    error_key: &str,
    action_key: &str,
    max_attempts: u32,
) -> RepairLoopDecision {
    let attempts = store
        .repair_attempts_for_finding(parent_run_id, finding_id)
        .await;
    let error_attempts = attempts
        .iter()
        .filter(|attempt| attempt.error_key == error_key)
        .count() as u32;
    let action_attempts = attempts
        .iter()
        .filter(|attempt| attempt.action_key == action_key)
        .count() as u32;

    if error_attempts >= max_attempts {
        return RepairLoopDecision::Stop {
            status: AgentRunStatus::Blocked,
            reason: RepairLoopStopReason::MaxAttemptsForSameError,
            error_attempts,
            action_attempts,
        };
    }
    if action_attempts >= max_attempts {
        return RepairLoopDecision::Stop {
            status: AgentRunStatus::Partial,
            reason: RepairLoopStopReason::IdenticalActionDoomLoop,
            error_attempts,
            action_attempts,
        };
    }
    RepairLoopDecision::Continue {
        error_attempts,
        action_attempts,
    }
}

pub fn normalize_error_key(raw_error: &str) -> String {
    let first_line = raw_error.lines().next().unwrap_or(raw_error).trim();
    let without_location = first_line.split(" at ").next().unwrap_or(first_line).trim();
    let without_numbers = without_location
        .chars()
        .map(|ch| if ch.is_ascii_digit() { '#' } else { ch })
        .collect::<String>();
    without_numbers
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

async fn apply_stop_decision(
    store: &RuntimeStore,
    parent_run_id: &str,
    repair_run_id: &str,
    status: AgentRunStatus,
    reason: &RepairLoopStopReason,
) -> Result<()> {
    if status == AgentRunStatus::Partial {
        save_repair_stop_checkpoint(store, parent_run_id, reason).await?;
        save_repair_stop_checkpoint(store, repair_run_id, reason).await?;
    }
    store.update_run_status(parent_run_id, status).await?;
    store.update_run_status(repair_run_id, status).await?;
    let state = match reason {
        RepairLoopStopReason::MaxAttemptsForSameError => {
            "repair_loop_stopped:max_attempts_for_same_error"
        }
        RepairLoopStopReason::IdenticalActionDoomLoop => {
            "repair_loop_stopped:identical_action_doom_loop"
        }
    };
    for run_id in [parent_run_id, repair_run_id] {
        let _ = store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.to_string(),
                state: state.to_string(),
                timestamp: Utc::now(),
            })
            .await;
    }
    Ok(())
}

async fn save_repair_stop_checkpoint(
    store: &RuntimeStore,
    run_id: &str,
    reason: &RepairLoopStopReason,
) -> Result<()> {
    let run = store
        .get_run(run_id)
        .await
        .ok_or_else(|| anyhow!("run not found for repair checkpoint: {run_id}"))?;
    if run.checkpoint_id.is_some() {
        return Ok(());
    }
    store
        .save_checkpoint(AgentCheckpoint {
            id: store.next_id("checkpoint"),
            run_id: run.id,
            project_id: run.project_id,
            phase: run.phase,
            message_window: Vec::new(),
            conversation_range: None,
            task_list: Vec::new(),
            workspace_snapshot_uri: None,
            build_result: None,
            context_content_hash: run.generation_context_content_hash.clone(),
            run_context_binding_hash: run.generation_context_binding_hash.clone(),
            runtime_attestation_hash: run.generation_context_runtime_attestation_hash.clone(),
            context_window_epoch: Some(run.context_window_epoch),
            execution_profile: run.execution_profile.clone(),
            target_session_epoch: None,
            target_workspace_revision: None,
            workflow_state: run.workflow_state.clone(),
            observation_receipts_version: (run.run_contract_version.as_deref()
                == Some(crate::generation_context::GENERATION_CONTEXT_SCHEMA))
            .then_some(1),
            brief_version: run.brief_version,
            design_version: run.design_version,
            last_known_preview_url: None,
            context_summary: format!("Repair loop stopped before completion: {reason:?}"),
            created_at: Utc::now(),
        })
        .await
}
