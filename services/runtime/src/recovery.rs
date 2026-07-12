use crate::{
    types::{AgentCheckpoint, AgentEvent, AgentRunStatus},
    RuntimeStore,
};
use anyhow::Result;
use chrono::Utc;

#[derive(Debug, Clone)]
pub enum RecoveryOutcome {
    Resumed {
        run_id: String,
        checkpoint: Box<AgentCheckpoint>,
    },
    Failed {
        run_id: String,
        preserved_checkpoint_id: Option<String>,
        reason: String,
    },
}

pub async fn recover_interrupted_runs(store: &RuntimeStore) -> Result<Vec<RecoveryOutcome>> {
    let runs = store.runs_requiring_recovery().await;
    let mut outcomes = Vec::new();
    for run in runs {
        outcomes.push(recover_run(store, &run.id).await?);
    }
    Ok(outcomes)
}

pub async fn recover_run(store: &RuntimeStore, run_id: &str) -> Result<RecoveryOutcome> {
    let run = store
        .get_run(run_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("run not found: {run_id}"))?;
    if run.status.is_terminal() || run.status == AgentRunStatus::NeedsUserInput {
        return Ok(RecoveryOutcome::Failed {
            run_id: run.id,
            preserved_checkpoint_id: run.checkpoint_id,
            reason: "run is not recoverable from its current status".to_string(),
        });
    }

    if let Some(checkpoint) = store.latest_checkpoint_for_run(run_id).await {
        if run.sandbox_id.is_some() {
            if let Err(error) = store.acquire_sandbox_binding_for_run(run_id, None).await {
                return fail_recovery_with_checkpoint(
                    store,
                    &run,
                    format!(
                        "Runtime restart could not recover run because its sandbox workspace is unavailable: {error}"
                    ),
                )
                .await;
            }
        }
        store
            .update_run_status(run_id, AgentRunStatus::Running)
            .await?;
        let _ = store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.to_string(),
                state: format!("recovered_from_checkpoint:{}", checkpoint.id),
                timestamp: Utc::now(),
            })
            .await;
        store
            .append_conversation_item(
                &run.project_id,
                Some(run_id),
                "progress",
                Some("system"),
                "Runtime recovered the run from its latest checkpoint.",
                Some(serde_json::json!({ "checkpointId": checkpoint.id })),
            )
            .await;
        return Ok(RecoveryOutcome::Resumed {
            run_id: run_id.to_string(),
            checkpoint: Box::new(checkpoint),
        });
    }

    let reason = "Runtime restart could not recover run because no checkpoint was available.";
    fail_recovery_with_checkpoint(store, &run, reason.to_string()).await
}

async fn fail_recovery_with_checkpoint(
    store: &RuntimeStore,
    run: &crate::types::AgentRun,
    reason: String,
) -> Result<RecoveryOutcome> {
    store
        .update_run_status(&run.id, AgentRunStatus::Failed)
        .await?;
    let _ = store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "failed".to_string(),
            summary: reason.clone(),
            timestamp: Utc::now(),
        })
        .await;
    store
        .append_conversation_item(
            &run.project_id,
            Some(&run.id),
            "error_summary",
            Some("system"),
            &reason,
            Some(serde_json::json!({ "recoverable": true, "checkpointId": run.checkpoint_id })),
        )
        .await;
    Ok(RecoveryOutcome::Failed {
        run_id: run.id.clone(),
        preserved_checkpoint_id: run.checkpoint_id.clone(),
        reason,
    })
}
