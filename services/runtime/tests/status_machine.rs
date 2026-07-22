use anydesign_runtime::{
    conversation::RuntimeStore,
    types::{AgentCheckpoint, AgentPhase, AgentRunStatus},
};
use chrono::Utc;
use serde_json::json;
use std::{fs, path::PathBuf};

#[tokio::test]
async fn terminal_run_status_is_irreversible() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;

    let completed = store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let completed_at = completed.completed_at;

    let result = store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await;

    assert!(result.is_err());
    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert_eq!(run.completed_at, completed_at);

    let repeated = store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    assert_eq!(repeated.status, AgentRunStatus::Completed);
    assert_eq!(repeated.completed_at, completed_at);
}

#[tokio::test]
async fn terminal_run_status_clears_pending_continue_interrupt() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;

    store.request_continue_interrupt(&run.id).await;
    assert!(store.continue_interrupt_requested(&run.id).await);

    store
        .update_run_status(&run.id, AgentRunStatus::Cancelled)
        .await
        .unwrap();

    assert!(!store.continue_interrupt_requested(&run.id).await);
}

#[tokio::test]
async fn terminal_run_status_expires_pending_permissions() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;

    store
        .update_run_status(&run.id, AgentRunStatus::Failed)
        .await
        .unwrap();

    let permission = store.pending_permission(&permission.id).await.unwrap();
    assert_eq!(permission.status, "expired");
    assert!(permission.resolved_at.is_some());
    assert!(store
        .resolve_permission(&permission.id, "allow")
        .await
        .is_err());
}

#[tokio::test]
async fn terminal_run_status_expired_permission_survives_restart() {
    let checkpoint_dir = unique_temp_dir("status-machine-permission-expired");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;

    store
        .update_run_status(&run.id, AgentRunStatus::Failed)
        .await
        .unwrap();

    let reloaded_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let permission = reloaded_store
        .pending_permission(&permission.id)
        .await
        .unwrap();
    assert_eq!(permission.status, "expired");
    assert!(permission.resolved_at.is_some());
    assert!(reloaded_store
        .resolve_permission(&permission.id, "allow")
        .await
        .is_err());
}

#[tokio::test]
async fn partial_status_requires_checkpoint() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;

    let result = store
        .update_run_status(&run.id, AgentRunStatus::Partial)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("cannot enter partial without a checkpoint"));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    store
        .save_checkpoint(AgentCheckpoint {
            id: "checkpoint-partial-1".to_string(),
            run_id: run.id.clone(),
            project_id: run.project_id.clone(),
            phase: run.phase,
            message_window: vec![json!({ "role": "assistant", "text": "partial state" })],
            conversation_range: None,
            task_list: vec![],
            workspace_snapshot_uri: None,
            build_result: None,
            context_content_hash: None,
            run_context_binding_hash: None,
            runtime_attestation_hash: None,
            context_window_epoch: None,
            execution_profile: None,
            target_session_epoch: None,
            target_workspace_revision: None,
            workflow_state: None,
            observation_receipts_version: None,
            brief_version: None,
            design_version: None,
            last_known_preview_url: None,
            context_summary: "partial checkpoint".to_string(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let partial = store
        .update_run_status(&run.id, AgentRunStatus::Partial)
        .await
        .unwrap();
    assert_eq!(partial.status, AgentRunStatus::Partial);
    assert_eq!(
        partial.checkpoint_id.as_deref(),
        Some("checkpoint-partial-1")
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
