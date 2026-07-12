use anydesign_runtime::{
    conversation::RunScopedResourceKind,
    model_gateway::ToolCall,
    tools::{control_plane::control_plane_executor, streaming::StreamingToolExecutor},
};
use anydesign_runtime::{
    preview::{promote_preview, PromotionGateReport},
    repair_loop::{
        record_repair_attempt, RepairActionSignature, RepairLoopDecision, RepairLoopStopReason,
    },
    types::{
        AgentCheckpoint, AgentPhase, AgentRunStatus, PermissionMode, ReviewFindingCategory,
        ReviewFindingEvidence, ReviewFindingSeverity, ReviewFindingStatus, TranscriptMode,
    },
    RuntimeStore,
};
use chrono::Utc;
use std::{fs, path::PathBuf};

async fn parent_build_run_with_candidate(store: &RuntimeStore) -> (String, String) {
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;
    (run.id, candidate.id)
}

#[tokio::test]
async fn blocking_review_finding_prevents_preview_promotion() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Preview rendered a blank first viewport",
            Some(ReviewFindingEvidence {
                screenshot_id: Some("shot-1".to_string()),
                file_path: None,
                log_excerpt: None,
            }),
            true,
        )
        .await
        .unwrap();

    let result = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version_id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("blocking review"));
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::Open
    );
    assert!(store.current_project_version("project-1").await.is_none());
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "review.finding"));
}

#[tokio::test]
async fn persisted_blocking_review_finding_prevents_promotion_after_restart() {
    let checkpoint_dir = unique_temp_dir("review-finding-persisted");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Preview rendered a blank first viewport",
            Some(ReviewFindingEvidence {
                screenshot_id: Some("shot-1".to_string()),
                file_path: None,
                log_excerpt: None,
            }),
            true,
        )
        .await
        .unwrap();

    let finding_log_path = store.review_finding_log_path();
    assert!(finding_log_path.ends_with("review-findings.jsonl"));
    let snapshots = fs::read_to_string(&finding_log_path).unwrap();
    assert!(snapshots.contains(&finding.id));
    assert!(snapshots.contains("\"status\":\"open\""));

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    assert_eq!(
        reloaded_store
            .get_review_finding(&finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Open
    );

    let blocked = promote_preview(
        &reloaded_store,
        "project-1",
        &run_id,
        &version_id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(blocked.is_err());
    assert!(blocked.unwrap_err().to_string().contains("blocking review"));
    assert!(reloaded_store
        .current_project_version("project-1")
        .await
        .is_none());
}

#[tokio::test]
async fn active_descendant_repair_run_prevents_preview_promotion() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let review = store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{version_id}")),
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-1",
            &review.id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Preview still renders blank",
            None,
            true,
        )
        .await
        .unwrap();
    store
        .update_run_status(&review.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let repair = store
        .create_repair_run_for_findings(
            &review.id,
            std::slice::from_ref(&finding.id),
            Some("event-review-finding-1".to_string()),
            "repair".to_string(),
            "internal-balanced".to_string(),
        )
        .await
        .unwrap();

    let pending = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version_id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(pending.is_err());
    assert!(pending
        .unwrap_err()
        .to_string()
        .contains("review/repair child run"));
    assert_eq!(
        store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Queued
    );
    assert!(store.current_project_version("project-1").await.is_none());
}

#[tokio::test]
async fn unresolved_repair_finding_prevents_preview_promotion_after_repair_terminal() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Content,
            "Missing product direction",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_repair_run_for_finding(&run_id, &finding.id, None)
        .await
        .unwrap();
    store
        .update_run_status(&repair.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();

    let blocked = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version_id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(blocked.is_err());
    assert!(blocked.unwrap_err().to_string().contains("blocking review"));
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::NeedsUserInput
    );
    assert!(store.current_project_version("project-1").await.is_none());
}

#[tokio::test]
async fn review_agent_tool_reports_blocking_finding_for_promotion_gate() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let review = store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{version_id}")),
            vec![],
        )
        .await
        .unwrap();

    let result = StreamingToolExecutor::new(control_plane_executor())
        .execute_calls(
            store.clone(),
            &review.id,
            vec![ToolCall::new(
                "tool-review-finding",
                "review.report_finding",
                serde_json::json!({
                    "versionId": version_id,
                    "severity": "blocking",
                    "category": "visual",
                    "summary": "First viewport renders blank",
                    "repairable": true,
                    "evidence": {
                        "screenshotId": "shot-1"
                    }
                }),
            )],
        )
        .await;

    assert_eq!(result.len(), 1);
    assert!(!result[0].result.is_error);
    let finding_id = result[0].result.content["findingId"].as_str().unwrap();
    let finding = store.get_review_finding(finding_id).await.unwrap();
    assert_eq!(finding.run_id, review.id);
    assert_eq!(finding.version_id, version_id);
    assert_eq!(finding.severity, ReviewFindingSeverity::Blocking);
    assert_eq!(finding.category, ReviewFindingCategory::Visual);
    assert!(finding.repairable);
    assert_eq!(
        finding.evidence.unwrap().screenshot_id.as_deref(),
        Some("shot-1")
    );
    assert!(store
        .events(&review.id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "review.finding"));

    store
        .update_run_status(&review.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let promoted = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version_id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(promoted.is_err());
    assert!(promoted
        .unwrap_err()
        .to_string()
        .contains("blocking review"));
    assert!(store.current_project_version("project-1").await.is_none());
}

#[tokio::test]
async fn review_report_finding_tool_is_review_phase_only() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;

    let result = StreamingToolExecutor::new(control_plane_executor())
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-build-finding",
                "review.report_finding",
                serde_json::json!({
                    "versionId": version_id,
                    "severity": "blocking",
                    "category": "visual",
                    "summary": "Build should not be able to report review findings"
                }),
            )],
        )
        .await;

    assert_eq!(result.len(), 1);
    assert!(result[0].result.is_error);
    assert!(result[0].result.content["error"]
        .as_str()
        .unwrap()
        .contains("only available to Review runs"));
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.tool == "review.report_finding" && record.decision == "deny"));
}

#[tokio::test]
async fn repair_child_run_freezes_parent_and_finding_context() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Runtime,
            "Preview route returns 500",
            Some(ReviewFindingEvidence {
                screenshot_id: None,
                file_path: None,
                log_excerpt: Some("GET / returned 500".to_string()),
            }),
            true,
        )
        .await
        .unwrap();

    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            Some("event-review-finding-1".to_string()),
            vec![finding.id.clone()],
        )
        .await
        .unwrap();

    assert_eq!(repair.parent_run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        repair.triggered_by_event_id.as_deref(),
        Some("event-review-finding-1")
    );
    assert_eq!(repair.finding_ids, Some(vec![finding.id]));
    assert_eq!(repair.base_version_id, None);
    assert_eq!(
        repair.profile_snapshot.permission_mode,
        PermissionMode::ScopedRepair
    );
    assert_eq!(
        repair.profile_snapshot.transcript_mode,
        TranscriptMode::Sidechain
    );
    assert_eq!(repair.profile_snapshot.source_checkpoint_id, None);
    assert!(repair.profile_snapshot.mcp_server_names.is_empty());
    assert_ne!(
        repair.input_message_ids,
        store.get_run(&run_id).await.unwrap().input_message_ids
    );
}

#[tokio::test]
async fn child_run_after_restart_freezes_persisted_parent_checkpoint() {
    let checkpoint_dir = unique_temp_dir("child-source-checkpoint-after-restart");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let checkpoint = AgentCheckpoint {
        id: "checkpoint-parent-review-source-1".to_string(),
        run_id: run_id.clone(),
        project_id: "project-1".to_string(),
        phase: AgentPhase::Build,
        message_window: vec![serde_json::json!({
            "role": "assistant",
            "text": "candidate ready for review"
        })],
        conversation_range: None,
        task_list: vec![],
        workspace_snapshot_uri: Some("file:///workspace/snapshots/candidate.tar".to_string()),
        build_result: None,
        brief_version: None,
        design_version: None,
        last_known_preview_url: Some("http://preview.local/preview/project-1/current".to_string()),
        context_summary: "candidate checkpoint".to_string(),
        created_at: Utc::now(),
    };
    store.save_checkpoint(checkpoint.clone()).await.unwrap();

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let review = reloaded_store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{version_id}")),
            vec![],
        )
        .await
        .unwrap();

    assert_eq!(
        review.profile_snapshot.source_checkpoint_id.as_deref(),
        Some(checkpoint.id.as_str())
    );
    assert_eq!(
        review.checkpoint_id.as_deref(),
        Some(checkpoint.id.as_str())
    );
    assert_eq!(review.parent_run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        review.profile_snapshot.permission_mode,
        PermissionMode::ReadOnly
    );
    assert_eq!(
        review.profile_snapshot.transcript_mode,
        TranscriptMode::Sidechain
    );
}

#[tokio::test]
async fn repair_run_lifecycle_updates_finding_status() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let fixed_finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Astro build fails",
            None,
            true,
        )
        .await
        .unwrap();
    let fixed_repair = store
        .create_repair_run_for_finding(
            &run_id,
            &fixed_finding.id,
            Some("event-review-finding-fixed".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(fixed_repair.parent_run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(fixed_repair.phase, AgentPhase::Repair);
    assert_eq!(
        fixed_repair.finding_ids,
        Some(vec![fixed_finding.id.clone()])
    );
    assert_eq!(
        store
            .get_review_finding(&fixed_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Repairing
    );
    record_repair_attempt(
        &store,
        &run_id,
        &fixed_repair.id,
        &fixed_finding.id,
        "TS2304 Cannot find name Hero",
        RepairActionSignature::new(
            "fs.patch",
            Some("/workspace/project/src/pages/index.astro".to_string()),
            vec![],
        ),
    )
    .await
    .unwrap();

    store
        .update_run_status(&fixed_repair.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_review_finding(&fixed_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Fixed
    );

    let needs_user_finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Content,
            "Missing product direction",
            None,
            true,
        )
        .await
        .unwrap();
    let needs_user_repair = store
        .create_repair_run_for_finding(&run_id, &needs_user_finding.id, None)
        .await
        .unwrap();
    store
        .update_run_status(&needs_user_repair.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_review_finding(&needs_user_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::NeedsUserInput
    );
}

#[tokio::test]
async fn persisted_repair_run_terminal_status_updates_finding_after_restart() {
    let checkpoint_dir = unique_temp_dir("repair-terminal-after-restart");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Astro build fails",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_repair_run_for_finding(&run_id, &finding.id, None)
        .await
        .unwrap();
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::Repairing
    );

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    reloaded_store
        .update_run_status(&repair.id, AgentRunStatus::Completed)
        .await
        .unwrap();

    assert_eq!(
        reloaded_store
            .get_review_finding(&finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Fixed
    );
    let snapshots = fs::read_to_string(reloaded_store.review_finding_log_path()).unwrap();
    assert!(snapshots.contains("\"status\":\"repairing\""));
    assert!(snapshots.contains("\"status\":\"fixed\""));
}

#[tokio::test]
async fn review_child_run_freezes_read_only_tool_policy() {
    let store = RuntimeStore::new();
    let (run_id, _) = parent_build_run_with_candidate(&store).await;
    let review = store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            None,
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        review.profile_snapshot.permission_mode,
        PermissionMode::ReadOnly
    );
    assert_eq!(
        review.profile_snapshot.transcript_mode,
        TranscriptMode::Sidechain
    );
    assert!(review
        .profile_snapshot
        .allowed_tools
        .contains(&"preview.status".to_string()));
    assert!(review
        .profile_snapshot
        .denied_tools
        .contains(&"shell.*".to_string()));

    let executor = StreamingToolExecutor::new(control_plane_executor());
    let results = executor
        .execute_calls(
            store.clone(),
            &review.id,
            vec![
                ToolCall::new("tool-1", "preview.status", serde_json::json!({})),
                ToolCall::new(
                    "tool-2",
                    "fs.write",
                    serde_json::json!({ "path": "project/nope.md", "text": "nope" }),
                ),
                ToolCall::new(
                    "tool-3",
                    "shell.run",
                    serde_json::json!({ "argv": ["pnpm", "build"], "cwd": "project" }),
                ),
            ],
        )
        .await;

    assert!(!results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert!(results[2].result.is_error);
    let audit = store.audit_records().await;
    assert!(audit
        .iter()
        .any(|record| record.tool == "fs.write" && record.decision == "deny"));
    assert!(audit
        .iter()
        .any(|record| record.tool == "shell.run" && record.decision == "deny"));
    assert!(store.events(&review.id).await.iter().any(|event| {
        let event = serde_json::to_value(event).unwrap();
        event["type"] == "permission.denied" && event["tool"] == "shell.run"
    }));
}

#[tokio::test]
async fn repair_child_run_has_scoped_write_policy_without_parent_session_allow_rules() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Content,
            "Hero copy is missing",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id],
        )
        .await
        .unwrap();
    assert_eq!(
        repair.profile_snapshot.permission_mode,
        PermissionMode::ScopedRepair
    );
    assert!(repair
        .profile_snapshot
        .allowed_tools
        .contains(&"fs.patch".to_string()));
    assert!(repair
        .profile_snapshot
        .denied_tools
        .contains(&"mcp__*".to_string()));
    assert!(repair.profile_snapshot.mcp_server_names.is_empty());

    let executor = StreamingToolExecutor::new(control_plane_executor());
    let result = executor
        .execute_calls(
            store.clone(),
            &repair.id,
            vec![ToolCall::new(
                "tool-1",
                "mcp__figma__get_file",
                serde_json::json!({ "fileKey": "figma-file" }),
            )],
        )
        .await;

    assert!(result[0].result.is_error);
    assert!(result[0].result.content["error"]
        .as_str()
        .unwrap()
        .contains("denied by frozen run profile policy"));
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.tool == "mcp__figma__get_file" && record.decision == "deny"));
}

#[tokio::test]
async fn terminal_child_run_cleans_agent_scoped_resources() {
    let store = RuntimeStore::new();
    let (run_id, _) = parent_build_run_with_candidate(&store).await;
    let review = store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            None,
            vec![],
        )
        .await
        .unwrap();

    store
        .register_run_scoped_resource(&review.id, RunScopedResourceKind::McpServer, "mcp-review")
        .await
        .unwrap();
    store
        .register_run_scoped_resource(
            &review.id,
            RunScopedResourceKind::BackgroundShellTask,
            "shell-review",
        )
        .await
        .unwrap();
    store
        .register_run_scoped_resource(
            &review.id,
            RunScopedResourceKind::TemporaryHook,
            "hook-review",
        )
        .await
        .unwrap();
    store
        .register_run_scoped_resource(
            &review.id,
            RunScopedResourceKind::ReadFileCache,
            "cache-review",
        )
        .await
        .unwrap();
    store
        .register_run_scoped_resource(
            &review.id,
            RunScopedResourceKind::SandboxLock,
            "lock-review",
        )
        .await
        .unwrap();
    assert!(!store.run_scoped_resources(&review.id).await.is_empty());

    store
        .update_run_status(&review.id, AgentRunStatus::Completed)
        .await
        .unwrap();

    assert!(store.run_scoped_resources(&review.id).await.is_empty());
}

#[tokio::test]
async fn child_run_cleanup_runs_on_failure_abort_and_blocked_without_touching_parent_resources() {
    let store = RuntimeStore::new();
    let (run_id, _) = parent_build_run_with_candidate(&store).await;
    store
        .register_run_scoped_resource(&run_id, RunScopedResourceKind::SandboxLock, "parent-lock")
        .await
        .unwrap();

    for status in [
        AgentRunStatus::Failed,
        AgentRunStatus::Cancelled,
        AgentRunStatus::Blocked,
    ] {
        let child = store
            .create_child_run(
                &run_id,
                AgentPhase::Repair,
                "repair".to_string(),
                "internal-balanced".to_string(),
                None,
                vec![],
            )
            .await
            .unwrap();
        store
            .register_run_scoped_resource(
                &child.id,
                RunScopedResourceKind::SandboxLock,
                format!("child-lock-{status:?}"),
            )
            .await
            .unwrap();

        store.update_run_status(&child.id, status).await.unwrap();

        assert!(
            store.run_scoped_resources(&child.id).await.is_empty(),
            "child resources should be cleaned for {status:?}"
        );
    }

    let parent_resources = store.run_scoped_resources(&run_id).await;
    assert_eq!(parent_resources.sandbox_locks, vec!["parent-lock"]);
}

#[tokio::test]
async fn repair_loop_stops_after_three_attempts_for_same_error_key() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Astro build fails",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id.clone()],
        )
        .await
        .unwrap();

    let mut decision = RepairLoopDecision::Continue {
        error_attempts: 0,
        action_attempts: 0,
    };
    for line in [17, 22, 103] {
        decision = record_repair_attempt(
            &store,
            &run_id,
            &repair.id,
            &finding.id,
            &format!("TS2304 Cannot find name Hero at src/pages/index.astro:{line}:5"),
            RepairActionSignature::new(
                "fs.patch",
                Some("/workspace/project/src/pages/index.astro".to_string()),
                vec![],
            ),
        )
        .await
        .unwrap();
    }

    assert_eq!(
        decision,
        RepairLoopDecision::Stop {
            status: AgentRunStatus::Blocked,
            reason: RepairLoopStopReason::MaxAttemptsForSameError,
            error_attempts: 3,
            action_attempts: 3,
        }
    );
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
}

#[tokio::test]
async fn persisted_repair_attempts_count_toward_max_attempts_after_restart() {
    let checkpoint_dir = unique_temp_dir("repair-attempts-after-restart");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Astro build fails",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id.clone()],
        )
        .await
        .unwrap();

    for line in [17, 22] {
        let decision = record_repair_attempt(
            &store,
            &run_id,
            &repair.id,
            &finding.id,
            &format!("TS2304 Cannot find name Hero at src/pages/index.astro:{line}:5"),
            RepairActionSignature::new(
                "fs.patch",
                Some("/workspace/project/src/pages/index.astro".to_string()),
                vec![],
            ),
        )
        .await
        .unwrap();
        assert!(matches!(decision, RepairLoopDecision::Continue { .. }));
    }
    let attempts = fs::read_to_string(store.repair_attempt_log_path()).unwrap();
    assert_eq!(attempts.lines().count(), 2);

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let decision = record_repair_attempt(
        &reloaded_store,
        &run_id,
        &repair.id,
        &finding.id,
        "TS2304 Cannot find name Hero at src/pages/index.astro:103:5",
        RepairActionSignature::new(
            "fs.patch",
            Some("/workspace/project/src/pages/index.astro".to_string()),
            vec![],
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        decision,
        RepairLoopDecision::Stop {
            status: AgentRunStatus::Blocked,
            reason: RepairLoopStopReason::MaxAttemptsForSameError,
            error_attempts: 3,
            action_attempts: 3,
        }
    );
    assert_eq!(
        reloaded_store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        reloaded_store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
}

#[tokio::test]
async fn repair_report_attempt_tool_blocks_after_three_same_error_attempts() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Astro build fails",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id.clone()],
        )
        .await
        .unwrap();
    let executor = StreamingToolExecutor::new(control_plane_executor());

    let mut last = None;
    for line in [17, 22, 103] {
        let results = executor
            .execute_calls(
                store.clone(),
                &repair.id,
                vec![ToolCall::new(
                    format!("tool-repair-attempt-{line}"),
                    "repair.report_attempt",
                    serde_json::json!({
                        "findingId": finding.id,
                        "rawError": format!("TS2304 Cannot find name Hero at src/pages/index.astro:{line}:5"),
                        "action": {
                            "tool": "fs.patch",
                            "path": "/workspace/project/src/pages/index.astro",
                            "argv": []
                        }
                    }),
                )],
            )
            .await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].result.is_error);
        last = Some(results[0].result.content.clone());
    }

    let last = last.unwrap();
    assert_eq!(last["decision"], "stop");
    assert_eq!(last["status"], "blocked");
    assert_eq!(last["reason"], "max_attempts_for_same_error");
    assert_eq!(last["errorAttempts"], 3);
    assert_eq!(last["actionAttempts"], 3);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.tool == "repair.report_attempt" && record.decision == "allow"));
}

#[tokio::test]
async fn repair_loop_detects_identical_argv_path_doom_loop() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Runtime,
            "Preview command keeps failing",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id.clone()],
        )
        .await
        .unwrap();

    let mut decision = RepairLoopDecision::Continue {
        error_attempts: 0,
        action_attempts: 0,
    };
    for raw_error in [
        "ERR_MODULE_NOT_FOUND missing package alpha",
        "ERR_MODULE_NOT_FOUND missing package beta",
        "ERR_MODULE_NOT_FOUND missing package gamma",
    ] {
        decision = record_repair_attempt(
            &store,
            &run_id,
            &repair.id,
            &finding.id,
            raw_error,
            RepairActionSignature::new(
                "shell.run",
                Some("/workspace/project".to_string()),
                vec!["pnpm".to_string(), "build".to_string()],
            ),
        )
        .await
        .unwrap();
    }

    assert_eq!(
        decision,
        RepairLoopDecision::Stop {
            status: AgentRunStatus::Partial,
            reason: RepairLoopStopReason::IdenticalActionDoomLoop,
            error_attempts: 1,
            action_attempts: 3,
        }
    );
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Partial
    );
    assert_eq!(
        store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Partial
    );
    assert!(store
        .get_run(&run_id)
        .await
        .unwrap()
        .checkpoint_id
        .is_some());
    assert!(store
        .get_run(&repair.id)
        .await
        .unwrap()
        .checkpoint_id
        .is_some());
}

#[tokio::test]
async fn repair_report_attempt_tool_detects_identical_action_doom_loop() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Runtime,
            "Preview command keeps failing",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_child_run(
            &run_id,
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![finding.id.clone()],
        )
        .await
        .unwrap();
    let executor = StreamingToolExecutor::new(control_plane_executor());

    let mut last = None;
    for raw_error in [
        "ERR_MODULE_NOT_FOUND missing package alpha",
        "ERR_MODULE_NOT_FOUND missing package beta",
        "ERR_MODULE_NOT_FOUND missing package gamma",
    ] {
        let results = executor
            .execute_calls(
                store.clone(),
                &repair.id,
                vec![ToolCall::new(
                    format!("tool-repair-attempt-{raw_error}"),
                    "repair.report_attempt",
                    serde_json::json!({
                        "findingId": finding.id,
                        "rawError": raw_error,
                        "action": {
                            "tool": "shell.run",
                            "path": "/workspace/project",
                            "argv": ["pnpm", "build"]
                        }
                    }),
                )],
            )
            .await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].result.is_error);
        last = Some(results[0].result.content.clone());
    }

    let last = last.unwrap();
    assert_eq!(last["decision"], "stop");
    assert_eq!(last["status"], "partial");
    assert_eq!(last["reason"], "identical_action_doom_loop");
    assert_eq!(last["errorAttempts"], 1);
    assert_eq!(last["actionAttempts"], 3);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Partial
    );
    assert_eq!(
        store.get_run(&repair.id).await.unwrap().status,
        AgentRunStatus::Partial
    );
    assert!(store
        .get_run(&run_id)
        .await
        .unwrap()
        .checkpoint_id
        .is_some());
    assert!(store
        .get_run(&repair.id)
        .await
        .unwrap()
        .checkpoint_id
        .is_some());
}

#[tokio::test]
async fn repair_report_attempt_tool_is_repair_phase_only() {
    let store = RuntimeStore::new();
    let (run_id, version_id) = parent_build_run_with_candidate(&store).await;
    let finding = store
        .record_review_finding(
            "project-1",
            &run_id,
            &version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Build should not be able to report repair attempts",
            None,
            true,
        )
        .await
        .unwrap();

    let result = StreamingToolExecutor::new(control_plane_executor())
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-build-repair-attempt",
                "repair.report_attempt",
                serde_json::json!({
                    "findingId": finding.id,
                    "rawError": "TS2304 Cannot find name Hero",
                    "action": {
                        "tool": "fs.patch",
                        "path": "/workspace/project/src/pages/index.astro",
                        "argv": []
                    }
                }),
            )],
        )
        .await;

    assert_eq!(result.len(), 1);
    assert!(result[0].result.is_error);
    assert!(result[0].result.content["error"]
        .as_str()
        .unwrap()
        .contains("only available to Repair runs"));
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.tool == "repair.report_attempt" && record.decision == "deny"));
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
