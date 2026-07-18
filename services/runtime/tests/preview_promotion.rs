use anydesign_runtime::{
    agent_loop::AgentLoop,
    config::RuntimePolicyProfile,
    conversation::RuntimeStore,
    model_gateway::{MockModelClient, ModelResponse, ToolCall},
    preview::{promote_preview, PromotionGateReport},
    tools::{
        control_plane::control_plane_executor,
        streaming::{tool_result_error_text, StreamingToolExecutor},
    },
    types::{
        sha256_hex, AgentEvent, AgentPhase, AgentRunStatus, ArtifactPublishStatus, PermissionMode,
        ProjectVersionStatus, ReviewFindingCategory, ReviewFindingSeverity, ReviewFindingStatus,
        TranscriptMode,
    },
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

async fn create_run(store: &RuntimeStore, phase: AgentPhase) -> String {
    store
        .create_run(
            "project-1".to_string(),
            phase,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id
}

fn executor() -> StreamingToolExecutor {
    StreamingToolExecutor::new(control_plane_executor().with_policy_profile_and_registry(
        RuntimePolicyProfile::LocalE2e,
        "https://registry.internal.example/npm/",
    ))
}

fn executor_with_workspace(workspace: &PathBuf) -> StreamingToolExecutor {
    StreamingToolExecutor::new(
        control_plane_executor()
            .with_policy_profile_and_registry(
                RuntimePolicyProfile::LocalE2e,
                "https://registry.internal.example/npm/",
            )
            .with_workspace_root(workspace),
    )
}

fn assert_error_kind(result: &anydesign_runtime::tools::runtime::ToolResult, expected: &str) {
    let metadata = result.metadata.as_ref().expect("error metadata");
    assert_eq!(
        metadata.get("errorKind").and_then(Value::as_str),
        Some(expected)
    );
    assert_eq!(
        metadata.get("recoverable").and_then(Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
async fn production_retires_manual_candidate_reporting_before_it_can_promote() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let production_executor = control_plane_executor();
    let (production_tools, production_deferred_tools) = production_executor
        .model_tool_snapshot(store.clone(), &run_id)
        .await;
    assert!(!production_tools
        .iter()
        .chain(production_deferred_tools.iter())
        .any(|tool| tool.name == "preview.report_candidate"));
    let local_executor = control_plane_executor().with_policy_profile_and_registry(
        RuntimePolicyProfile::LocalE2e,
        "https://registry.internal.example/npm/",
    );
    let (local_tools, local_deferred_tools) = local_executor
        .model_tool_snapshot(store.clone(), &run_id)
        .await;
    assert!(local_tools
        .iter()
        .chain(local_deferred_tools.iter())
        .any(|tool| tool.name == "preview.report_candidate"));

    let results = StreamingToolExecutor::new(production_executor)
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "manual-candidate",
                "preview.report_candidate",
                json!({
                    "url": "http://127.0.0.1:4321",
                    "screenshotId": "shot-1"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.manual_candidate_retired");
    assert!(store.current_project_version("project-1").await.is_none());
    assert!(store
        .get_run(&run_id)
        .await
        .unwrap()
        .output_version_id
        .is_none());
}

#[tokio::test]
async fn preview_tools_emit_rebuilding_and_candidate_events() {
    let workspace = setup_passing_promotion_workspace("preview-promotion");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "preview.rebuilding",
                    json!({ "previousVersionId": "version-old" }),
                ),
                ToolCall::new(
                    "tool-2",
                    "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1",
                    "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-test"
                }),
            ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| !result.result.is_error));
    let version_id = results[1].result.content["versionId"].as_str().unwrap();
    let review_run_id = results[1].result.content["reviewRunId"].as_str().unwrap();
    let version = store.get_project_version(version_id).await.unwrap();
    assert_eq!(version.status, ProjectVersionStatus::Promoted);
    assert_eq!(version.preview_url, preview_url);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().output_version_id,
        Some(version_id.to_string())
    );
    let review_run = store.get_run(review_run_id).await.unwrap();
    assert_eq!(review_run.parent_run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(review_run.phase, AgentPhase::Review);
    assert_eq!(review_run.agent_profile, "visual-review");
    assert_eq!(review_run.status, AgentRunStatus::Completed);
    let expected_trigger = format!("preview.candidate:{version_id}");
    assert_eq!(
        review_run.triggered_by_event_id.as_deref(),
        Some(expected_trigger.as_str())
    );
    assert_eq!(
        review_run.profile_snapshot.permission_mode,
        PermissionMode::ReadOnly
    );
    assert_eq!(
        review_run.profile_snapshot.transcript_mode,
        TranscriptMode::Sidechain
    );
    let child_runs = store.child_runs(&run_id).await;
    assert_eq!(child_runs.len(), 1);
    assert_eq!(child_runs[0].id, review_run_id);

    let event_types = store
        .events(&run_id)
        .await
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"preview.rebuilding".to_string()));
    assert!(event_types.contains(&"preview.candidate".to_string()));
    assert!(event_types.contains(&"preview.updated".to_string()));
}

#[tokio::test]
async fn preview_report_candidate_rejects_second_promotion_in_same_run() {
    let workspace = setup_passing_promotion_workspace("preview-promotion-duplicate");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Edit).await;
    let (preview_url, _preview_server) = start_preview_server().await;
    let executor = executor_with_workspace(&workspace);

    let first = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-first",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1",
                    "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-test"
                }),
            )],
        )
        .await;

    assert_eq!(first.len(), 1);
    assert!(!first[0].result.is_error);
    let first_version_id = first[0].result.content["versionId"]
        .as_str()
        .unwrap()
        .to_string();

    let second = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-second",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1",
                    "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-test"
                }),
            )],
        )
        .await;

    assert_eq!(second.len(), 1);
    assert!(second[0].result.is_error);
    assert_error_kind(&second[0].result, "preview.already_promoted");
    assert_eq!(
        store.get_run(&run_id).await.unwrap().output_version_id,
        Some(first_version_id)
    );
    let candidate_events = store
        .events(&run_id)
        .await
        .into_iter()
        .filter(|event| serde_json::to_value(event).unwrap()["type"] == "preview.candidate")
        .count();
    assert_eq!(candidate_events, 1);
}

#[tokio::test]
async fn preview_report_candidate_rejects_public_or_unreachable_urls() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;

    let public = executor()
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-public",
                "preview.report_candidate",
                json!({ "url": "https://example.com/candidate" }),
            )],
        )
        .await;
    assert!(public[0].result.is_error);
    assert!(tool_result_error_text(&public[0].result).contains("public preview URL"));

    let unreachable = executor()
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-unreachable",
                "preview.report_candidate",
                json!({ "url": "http://127.0.0.1:9/candidate" }),
            )],
        )
        .await;
    assert!(unreachable[0].result.is_error);
    assert!(tool_result_error_text(&unreachable[0].result).contains("could not reach"));
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .all(|event| { serde_json::to_value(event).unwrap()["type"] != "preview.candidate" }));
}

#[tokio::test]
async fn preview_report_candidate_requires_screenshot_before_creating_candidate() {
    let workspace = setup_passing_promotion_workspace("preview-missing-screenshot");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-missing-shot",
                "preview.report_candidate",
                json!({ "url": preview_url }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("requires screenshotId"));
    assert_error_kind(&results[0].result, "preview.screenshot_missing");
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .all(|event| { serde_json::to_value(event).unwrap()["type"] != "preview.candidate" }));
}

#[tokio::test]
async fn preview_report_candidate_rejects_invalid_screenshot_id() {
    let workspace = setup_passing_promotion_workspace("preview-invalid-screenshot");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-invalid-shot",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "../shot-1"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.screenshot_invalid");
}

#[tokio::test]
async fn preview_report_candidate_rejects_blank_screenshot() {
    let workspace = setup_passing_promotion_workspace("preview-blank-screenshot");
    fs::write(
        workspace.join("outputs/screenshots/blank-shot.json"),
        json!({ "blank": true }).to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-blank-shot",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "blank-shot"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.screenshot_blank");
}

#[tokio::test]
async fn preview_report_candidate_requires_build_evidence() {
    let workspace = setup_passing_promotion_workspace("preview-build-missing");
    fs::remove_file(workspace.join("outputs/build/latest.json")).unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-build-missing",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.build_missing");
}

#[tokio::test]
async fn preview_report_candidate_rejects_failed_latest_build() {
    let workspace = setup_passing_promotion_workspace("preview-build-failed");
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "buildId": "build-test",
            "status": "failed",
            "success": false,
            "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-test"
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-build-failed",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.build_failed");
}

#[tokio::test]
async fn preview_report_candidate_requires_source_snapshot_evidence() {
    let workspace = setup_passing_promotion_workspace("preview-source-missing");
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "buildId": "build-test",
            "status": "success",
            "success": true
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-source-missing",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.source_snapshot_missing");
}

#[tokio::test]
async fn preview_report_candidate_requires_latest_build_source_snapshot() {
    let workspace = setup_passing_promotion_workspace("preview-source-snapshot");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let (preview_url, _preview_server) = start_preview_server().await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-mismatch",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-1",
                    "sourceSnapshotUri": "file:///workspace/outputs/build/old-source-snapshot.txt"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert!(
        tool_result_error_text(&results[0].result).contains("does not match latest project.build")
    );
    assert_error_kind(&results[0].result, "preview.source_snapshot_mismatch");
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .all(|event| { serde_json::to_value(event).unwrap()["type"] != "preview.candidate" }));
}

#[tokio::test]
async fn preview_start_requires_built_dist_directory() {
    let workspace = setup_passing_promotion_workspace("preview-dist-missing");
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;

    let results = executor_with_workspace(&workspace)
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-preview-start",
                "preview.start",
                json!({ "url": "http://127.0.0.1:9" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "preview.dist_missing");
}

#[tokio::test]
async fn promote_preview_waits_for_review_child_terminal_state() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/candidate".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;
    let review = store
        .create_child_run(
            &run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{}", version.id)),
            vec![],
        )
        .await
        .unwrap();

    let pending = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version.id,
        PromotionGateReport::passing(),
    )
    .await;

    assert!(pending.is_err());
    assert!(pending
        .unwrap_err()
        .to_string()
        .contains("review/repair child run"));
    assert!(store.current_project_version("project-1").await.is_none());

    store
        .update_run_status(&review.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let promoted = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();

    assert_eq!(promoted.status, ProjectVersionStatus::Promoted);
}

#[tokio::test]
async fn promote_preview_rejects_failed_gate_and_leaves_candidate_unpromoted() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/candidate".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;

    let result = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version.id,
        PromotionGateReport {
            screenshot_blank: true,
            ..PromotionGateReport::passing()
        },
    )
    .await;

    assert!(result.is_err());
    let version = store.get_project_version(&version.id).await.unwrap();
    assert_eq!(version.status, ProjectVersionStatus::Candidate);
    assert!(store
        .get_run(&run_id)
        .await
        .unwrap()
        .output_version_id
        .is_none());
}

#[tokio::test]
async fn promote_preview_updates_project_run_and_events() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/candidate".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;

    let promoted = promote_preview(
        &store,
        "project-1",
        &run_id,
        &version.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();

    assert_eq!(promoted.status, ProjectVersionStatus::Promoted);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().output_version_id,
        Some(version.id.clone())
    );
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        version.id
    );
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "preview.updated"));
    let run = store.get_run(&run_id).await.unwrap();
    let checkpoint_id = run
        .checkpoint_id
        .expect("promotion should save a checkpoint");
    let checkpoint = store.get_checkpoint(&checkpoint_id).await.unwrap();
    assert_eq!(
        checkpoint.workspace_snapshot_uri.as_deref(),
        Some("file:///workspace/snapshots/candidate.tar")
    );
    assert_eq!(
        checkpoint.last_known_preview_url.as_deref(),
        Some("http://preview.local/candidate")
    );
    let build_result = checkpoint
        .build_result
        .as_ref()
        .expect("promotion checkpoint should capture build result");
    assert_eq!(build_result.version_id, version.id);
    assert_eq!(build_result.status, ProjectVersionStatus::Promoted);
    assert_eq!(build_result.preview_url, "http://preview.local/candidate");
    assert_eq!(
        build_result.source_snapshot_uri.as_deref(),
        Some("file:///workspace/snapshots/candidate.tar")
    );
    assert_eq!(build_result.screenshot_id.as_deref(), Some("shot-1"));
    assert!(checkpoint.context_summary.contains("preview promoted"));
}

#[tokio::test]
async fn build_run_complete_requires_promoted_preview() {
    let store = RuntimeStore::new();
    let run_id = create_run(&store, AgentPhase::Build).await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/candidate".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;

    let without_output = executor()
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Done" }),
            )],
        )
        .await;
    assert!(without_output[0].result.is_error);
    assert!(tool_result_error_text(&without_output[0].result).contains("output_version_id"));

    store
        .set_run_output_version(&run_id, candidate.id.clone())
        .await
        .unwrap();
    let unpromoted = executor()
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-2",
                "run.complete",
                json!({ "status": "completed", "summary": "Done" }),
            )],
        )
        .await;
    assert!(unpromoted[0].result.is_error);
    let unpromoted_error = tool_result_error_text(&unpromoted[0].result);
    assert!(
        unpromoted_error.contains("project state")
            || unpromoted_error.contains("candidate validation report"),
        "candidate completion must fail closed before artifact promotion when its frozen contract or validation evidence is missing: {unpromoted_error}"
    );

    promote_preview(
        &store,
        "project-1",
        &run_id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-3",
        "run.complete",
        json!({ "status": "completed", "summary": "Done" }),
    )])]);
    let loop_runner = AgentLoop::with_tool_executor(
        store.clone(),
        Arc::new(model.clone()),
        control_plane_executor(),
    );

    let completed = loop_runner.run(&run_id).await.unwrap();

    model.assert_all_consumed().await;
    assert!(!completed[0].is_error);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn repair_run_complete_requires_new_version_and_fresh_runtime_source_snapshot() {
    let store = RuntimeStore::new();
    let build_run_id = create_run(&store, AgentPhase::Build).await;
    let base = store
        .create_project_version_candidate(
            "project-1",
            &build_run_id,
            "http://preview.local/base".to_string(),
            Some("shot-base".to_string()),
            Some("runtime://source-snapshots/project-1/build-base".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &build_run_id,
        &base.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let repair = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            vec![],
            None,
            Some(base.id.clone()),
        )
        .await;

    let without_output = executor()
        .execute_calls(
            store.clone(),
            &repair.id,
            vec![ToolCall::new(
                "repair-complete-without-output",
                "run.complete",
                json!({ "status": "completed", "summary": "Done" }),
            )],
        )
        .await;
    assert!(without_output[0].result.is_error);
    assert!(tool_result_error_text(&without_output[0].result).contains("output_version_id"));

    let stale = store
        .create_project_version_candidate(
            "project-1",
            &repair.id,
            "http://preview.local/stale-repair".to_string(),
            Some("shot-stale-repair".to_string()),
            base.source_snapshot_uri.clone(),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &repair.id,
        &stale.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .set_run_output_version(&repair.id, stale.id.clone())
        .await
        .unwrap();
    let stale_snapshot = executor()
        .execute_calls(
            store.clone(),
            &repair.id,
            vec![ToolCall::new(
                "repair-complete-stale-snapshot",
                "run.complete",
                json!({ "status": "completed", "summary": "Done" }),
            )],
        )
        .await;
    assert!(stale_snapshot[0].result.is_error);
    assert!(tool_result_error_text(&stale_snapshot[0].result)
        .contains("fresh Runtime-owned source snapshot"));

    let fresh = store
        .create_project_version_candidate(
            "project-1",
            &repair.id,
            "http://preview.local/fresh-repair".to_string(),
            Some("shot-fresh-repair".to_string()),
            Some("runtime://source-snapshots/project-1/build-repair".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &repair.id,
        &fresh.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .set_run_output_version(&repair.id, fresh.id.clone())
        .await
        .unwrap();
    let completed = executor()
        .execute_calls(
            store.clone(),
            &repair.id,
            vec![ToolCall::new(
                "repair-complete-fresh-snapshot",
                "run.complete",
                json!({ "status": "completed", "summary": "Done" }),
            )],
        )
        .await;
    assert!(!completed[0].result.is_error);
}

#[test]
fn preview_promote_is_not_registered_as_model_tool() {
    assert!(!control_plane_executor().has_tool("preview.promote"));
}

#[tokio::test]
async fn completed_promotion_commit_atomically_persists_run_version_publish_and_outboxes() {
    let storage = unique_temp_dir("completed-promotion-wal");
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://runtime/artifacts/project-1/current".to_string(),
            Some("shot-complete".to_string()),
            Some("runtime://source-snapshots/project-1/build-complete".to_string()),
        )
        .await;
    store
        .set_run_output_version(&run_id, version.id.clone())
        .await
        .unwrap();
    let publish = store
        .begin_artifact_publish(
            "project-1",
            &run_id,
            "build-complete",
            &version.id,
            &"a".repeat(64),
            "runtime://source-snapshots/project-1/build-complete",
            None,
        )
        .await
        .unwrap();
    let artifact_manifest_hash = "b".repeat(64);
    for status in [
        ArtifactPublishStatus::Staged,
        ArtifactPublishStatus::Validating,
        ArtifactPublishStatus::Ready,
        ArtifactPublishStatus::Promoting,
    ] {
        store
            .transition_artifact_publish(
                &publish.id,
                status,
                (status == ArtifactPublishStatus::Staged)
                    .then_some(artifact_manifest_hash.as_str()),
                (status == ArtifactPublishStatus::Staged).then_some("runtime://staged"),
                (status == ArtifactPublishStatus::Promoting)
                    .then_some("runtime://artifacts/project-1/versions/version-complete"),
                None,
            )
            .await
            .unwrap();
    }

    let (_, preview_outbox, completion_outbox) = store
        .complete_artifact_promotion_cas(
            "project-1",
            &run_id,
            &version.id,
            &publish.id,
            None,
            "Atomic completion.",
        )
        .await
        .unwrap();
    let (replayed_version, replayed_preview_outbox, replayed_completion_outbox) = store
        .complete_artifact_promotion_cas(
            "project-1",
            &run_id,
            &version.id,
            &publish.id,
            None,
            "Atomic completion.",
        )
        .await
        .unwrap();
    assert_eq!(replayed_version.id, version.id);
    assert_eq!(replayed_preview_outbox.id, preview_outbox.id);
    assert_eq!(replayed_completion_outbox.id, completion_outbox.id);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        version.id
    );
    assert_eq!(
        store
            .get_artifact_publish(&publish.id)
            .await
            .unwrap()
            .status,
        ArtifactPublishStatus::Promoted
    );
    assert!(store.events(&run_id).await.is_empty());

    store
        .dispatch_outbox_event(&preview_outbox.id)
        .await
        .unwrap();
    // Simulate a process crash after the terminal event append succeeded but
    // before the outbox Delivered snapshot was persisted.
    store
        .append_event(completion_outbox.event.clone())
        .await
        .unwrap();
    store
        .dispatch_outbox_event(&completion_outbox.id)
        .await
        .unwrap();
    let events = store.events(&run_id).await;
    assert!(matches!(events[0], AgentEvent::PreviewUpdated { .. }));
    assert!(events[1].is_run_completed());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.is_run_completed())
            .count(),
        1,
        "completion outbox recovery must not duplicate the terminal event"
    );
    drop(store);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    assert_eq!(
        restarted.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        restarted
            .current_project_version("project-1")
            .await
            .unwrap()
            .id,
        version.id
    );
    assert_eq!(restarted.reconcile_artifact_promotions().await.unwrap(), 0);
    assert_eq!(restarted.events(&run_id).await.len(), 2);
}

#[tokio::test]
async fn completed_repair_promotion_atomically_persists_fixed_findings() {
    let storage = unique_temp_dir("completed-repair-promotion-wal");
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let build_run_id = create_run(&store, AgentPhase::Build).await;
    let base_version = store
        .create_project_version_candidate(
            "project-1",
            &build_run_id,
            "http://runtime/artifacts/project-1/base".to_string(),
            Some("shot-base".to_string()),
            Some("runtime://source-snapshots/project-1/base".to_string()),
        )
        .await;
    store
        .promote_project_version("project-1", &build_run_id, &base_version.id)
        .await
        .unwrap();
    let review = store
        .create_child_run(
            &build_run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-balanced".to_string(),
            None,
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-1",
            &review.id,
            &base_version.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Repair the promoted candidate",
            None,
            true,
        )
        .await
        .unwrap();
    let repair = store
        .create_repair_run_for_findings(
            &review.id,
            std::slice::from_ref(&finding.id),
            None,
            "repair".to_string(),
            "internal-balanced".to_string(),
        )
        .await
        .unwrap();
    let repaired_version = store
        .create_project_version_candidate(
            "project-1",
            &repair.id,
            "http://runtime/artifacts/project-1/repaired".to_string(),
            Some("shot-repaired".to_string()),
            Some("runtime://source-snapshots/project-1/repaired".to_string()),
        )
        .await;
    let publish = store
        .begin_artifact_publish(
            "project-1",
            &repair.id,
            "build-repaired",
            &repaired_version.id,
            &"c".repeat(64),
            "runtime://source-snapshots/project-1/repaired",
            Some(&base_version.id),
        )
        .await
        .unwrap();
    for status in [
        ArtifactPublishStatus::Staged,
        ArtifactPublishStatus::Validating,
        ArtifactPublishStatus::Ready,
        ArtifactPublishStatus::Promoting,
    ] {
        store
            .transition_artifact_publish(
                &publish.id,
                status,
                (status == ArtifactPublishStatus::Staged).then_some(&*"d".repeat(64)),
                (status == ArtifactPublishStatus::Staged)
                    .then_some("runtime://artifacts/project-1/staged/repaired"),
                (status == ArtifactPublishStatus::Promoting)
                    .then_some("runtime://artifacts/project-1/versions/repaired"),
                None,
            )
            .await
            .unwrap();
    }

    store
        .complete_artifact_promotion_cas(
            "project-1",
            &repair.id,
            &repaired_version.id,
            &publish.id,
            Some(&base_version.id),
            "Repair complete.",
        )
        .await
        .unwrap();
    let completed_repair = store.get_run(&repair.id).await.unwrap();
    assert_eq!(completed_repair.status, AgentRunStatus::Completed);
    assert!(completed_repair.completed_at.is_some());
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::Fixed
    );
    drop(store);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    assert_eq!(
        restarted
            .get_review_finding(&finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Fixed
    );
}

#[tokio::test]
async fn promotion_wal_recovers_current_run_publish_and_pending_outbox_once() {
    let storage = unique_temp_dir("promotion-wal-recovery");
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://runtime/artifacts/project-1/current".to_string(),
            Some("shot-wal".to_string()),
            Some("runtime://source-snapshots/project-1/build-wal".to_string()),
        )
        .await;
    let publish = store
        .begin_artifact_publish(
            "project-1",
            &run_id,
            "build-wal",
            &version.id,
            &"a".repeat(64),
            "runtime://source-snapshots/project-1/build-wal",
            None,
        )
        .await
        .unwrap();
    store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Staged,
            Some(&"b".repeat(64)),
            Some("runtime://artifacts/project-1/staged/version-wal"),
            None,
            None,
        )
        .await
        .unwrap();
    store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Validating,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Promoting,
            None,
            None,
            Some("runtime://artifacts/project-1/versions/version-wal"),
            None,
        )
        .await
        .unwrap();
    let (_, outbox) = store
        .commit_artifact_promotion_cas("project-1", &run_id, &version.id, &publish.id, None)
        .await
        .unwrap();
    let (replayed_version, replayed_outbox) = store
        .commit_artifact_promotion_cas("project-1", &run_id, &version.id, &publish.id, None)
        .await
        .unwrap();
    assert_eq!(replayed_version.id, version.id);
    assert_eq!(replayed_outbox.id, outbox.id);
    assert!(store.events(&run_id).await.is_empty());
    drop(store);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    assert_eq!(
        restarted
            .current_project_version("project-1")
            .await
            .unwrap()
            .id,
        version.id
    );
    assert_eq!(
        restarted.get_run(&run_id).await.unwrap().output_version_id,
        Some(version.id.clone())
    );
    assert_eq!(
        restarted
            .get_artifact_publish(&publish.id)
            .await
            .unwrap()
            .status,
        ArtifactPublishStatus::Promoted
    );
    assert_eq!(restarted.reconcile_artifact_promotions().await.unwrap(), 1);
    assert_eq!(restarted.reconcile_artifact_promotions().await.unwrap(), 0);
    let updated = restarted
        .events(&run_id)
        .await
        .into_iter()
        .filter(|event| {
            matches!(event, AgentEvent::PreviewUpdated { version_id, .. } if version_id == &version.id)
        })
        .count();
    assert_eq!(updated, 1, "outbox {} must be delivered once", outbox.id);
}

#[tokio::test]
async fn startup_reconcile_replays_promotion_after_immutable_bytes_before_cas() {
    let storage = unique_temp_dir("promotion-before-cas-recovery");
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://runtime/artifacts/project-1/current".to_string(),
            None,
            Some("runtime://source-snapshots/project-1/build-before-cas".to_string()),
        )
        .await;
    let publish = store
        .begin_artifact_publish(
            "project-1",
            &run_id,
            "build-before-cas",
            &version.id,
            &"e".repeat(64),
            "runtime://source-snapshots/project-1/build-before-cas",
            None,
        )
        .await
        .unwrap();
    for status in [
        ArtifactPublishStatus::Staged,
        ArtifactPublishStatus::Validating,
    ] {
        store
            .transition_artifact_publish(
                &publish.id,
                status,
                (status == ArtifactPublishStatus::Staged).then_some("hash"),
                (status == ArtifactPublishStatus::Staged).then_some("runtime://staged"),
                None,
                None,
            )
            .await
            .unwrap();
    }
    store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Promoting,
            None,
            None,
            Some("runtime://artifacts/project-1/versions/version-before-cas"),
            None,
        )
        .await
        .unwrap();
    drop(store);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    assert_eq!(restarted.reconcile_artifact_promotions().await.unwrap(), 1);
    assert_eq!(
        restarted
            .current_project_version("project-1")
            .await
            .unwrap()
            .id,
        version.id
    );
    assert_eq!(
        restarted
            .get_artifact_publish(&publish.id)
            .await
            .unwrap()
            .status,
        ArtifactPublishStatus::Promoted
    );
    assert_eq!(
        restarted
            .events(&run_id)
            .await
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::PreviewUpdated { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn startup_reconcile_does_not_promote_completion_coupled_candidate() {
    let storage = unique_temp_dir("completion-coupled-before-cas");
    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let run_id = create_run(&store, AgentPhase::Build).await;
    let version = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://runtime/artifacts/project-1/current".to_string(),
            None,
            Some("runtime://source-snapshots/project-1/build-coupled".to_string()),
        )
        .await;
    store
        .set_run_output_version(&run_id, version.id.clone())
        .await
        .unwrap();
    let publish = store
        .begin_artifact_publish(
            "project-1",
            &run_id,
            "build-coupled",
            &version.id,
            &"e".repeat(64),
            "runtime://source-snapshots/project-1/build-coupled",
            None,
        )
        .await
        .unwrap();
    let artifact_manifest_hash = "f".repeat(64);
    for status in [
        ArtifactPublishStatus::Staged,
        ArtifactPublishStatus::Validating,
        ArtifactPublishStatus::Ready,
        ArtifactPublishStatus::Promoting,
    ] {
        store
            .transition_artifact_publish(
                &publish.id,
                status,
                (status == ArtifactPublishStatus::Staged)
                    .then_some(artifact_manifest_hash.as_str()),
                (status == ArtifactPublishStatus::Staged).then_some("runtime://staged"),
                (status == ArtifactPublishStatus::Promoting)
                    .then_some("runtime://artifacts/project-1/versions/version-coupled"),
                None,
            )
            .await
            .unwrap();
    }
    drop(store);

    let restarted = RuntimeStore::with_checkpoint_dir(&storage);
    assert_eq!(restarted.reconcile_artifact_promotions().await.unwrap(), 0);
    assert!(restarted
        .current_project_version("project-1")
        .await
        .is_none());
    assert_ne!(
        restarted.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        restarted
            .get_project_version(&version.id)
            .await
            .unwrap()
            .status,
        ProjectVersionStatus::Candidate
    );
}

#[tokio::test]
async fn artifact_promotion_cas_prevents_concurrent_run_from_overwriting_current() {
    let store = RuntimeStore::new();
    let first_run = create_run(&store, AgentPhase::Build).await;
    let second_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id;
    let mut candidates = Vec::new();
    for (run_id, build_id, marker) in [
        (&first_run, "build-first", 'c'),
        (&second_run, "build-second", 'd'),
    ] {
        let version = store
            .create_project_version_candidate(
                "project-1",
                run_id,
                format!("http://runtime/{build_id}"),
                None,
                Some(format!("runtime://source-snapshots/project-1/{build_id}")),
            )
            .await;
        let publish = store
            .begin_artifact_publish(
                "project-1",
                run_id,
                build_id,
                &version.id,
                &marker.to_string().repeat(64),
                &format!("runtime://source-snapshots/project-1/{build_id}"),
                None,
            )
            .await
            .unwrap();
        store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::Staged,
                Some(&marker.to_ascii_uppercase().to_string().repeat(64)),
                Some(&format!(
                    "runtime://artifacts/project-1/staged/{}",
                    version.id
                )),
                None,
                None,
            )
            .await
            .unwrap();
        store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::Validating,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        store
            .transition_artifact_publish(
                &publish.id,
                ArtifactPublishStatus::Promoting,
                None,
                None,
                Some(&format!(
                    "runtime://artifacts/project-1/versions/{}",
                    version.id
                )),
                None,
            )
            .await
            .unwrap();
        candidates.push((run_id.clone(), version, publish));
    }
    store
        .commit_artifact_promotion_cas(
            "project-1",
            &candidates[0].0,
            &candidates[0].1.id,
            &candidates[0].2.id,
            None,
        )
        .await
        .unwrap();
    let conflict = store
        .complete_artifact_promotion_cas(
            "project-1",
            &candidates[1].0,
            &candidates[1].1.id,
            &candidates[1].2.id,
            None,
            "Concurrent completion must fail.",
        )
        .await
        .unwrap_err();
    assert!(conflict.to_string().contains("compare-and-swap failed"));
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        candidates[0].1.id
    );
    assert_eq!(
        store
            .get_project_version(&candidates[1].1.id)
            .await
            .unwrap()
            .status,
        ProjectVersionStatus::Candidate
    );
    assert_eq!(
        store.get_run(&candidates[1].0).await.unwrap().status,
        AgentRunStatus::Queued
    );
    assert!(!store
        .events(&candidates[1].0)
        .await
        .iter()
        .any(AgentEvent::is_run_completed));
}

async fn start_preview_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    (format!("http://{addr}/candidate"), handle)
}

fn setup_passing_promotion_workspace(prefix: &str) -> PathBuf {
    let workspace = unique_temp_dir(prefix);
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::create_dir_all(workspace.join("outputs/screenshots")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::write(workspace.join("outputs/build/build.log"), "Build ok\n").unwrap();
    fs::write(
        workspace.join("outputs/build/source-snapshot.txt"),
        "buildId: build-test\nstatus: success\n",
    )
    .unwrap();
    fs::create_dir_all(workspace.join("outputs/build/source-snapshots/build-test")).unwrap();
    fs::write(
        workspace.join("outputs/build/source-snapshots/build-test/package.json"),
        "{}",
    )
    .unwrap();
    let candidate_root = workspace.join("outputs/candidates/build-test");
    fs::create_dir_all(&candidate_root).unwrap();
    fs::write(candidate_root.join("index.html"), "candidate").unwrap();
    let candidate_manifest = serde_json::to_string_pretty(&json!({
        "schemaVersion": "candidate-manifest@1",
        "buildId": "build-test",
        "files": [{
            "path": "index.html",
            "bytes": 9,
            "sha256": sha256_hex(b"candidate")
        }]
    }))
    .unwrap();
    fs::write(
        candidate_root.join(".anydesign-candidate-manifest.json"),
        &candidate_manifest,
    )
    .unwrap();
    let candidate_manifest_hash = sha256_hex(candidate_manifest.as_bytes());
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "buildId": "build-test",
            "status": "success",
            "success": true,
            "cwd": "/workspace/project",
            "argv": ["npm", "run", "build"],
            "logPath": "/workspace/outputs/build/build.log",
            "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-test",
            "candidateOutputPath": "/workspace/outputs/candidates/build-test",
            "candidateManifestPath": "/workspace/outputs/candidates/build-test/.anydesign-candidate-manifest.json",
            "candidateManifestHash": candidate_manifest_hash
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        json!({ "accessible": true, "status": "running" }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/screenshots/shot-1.json"),
        json!({ "blank": false }).to_string(),
    )
    .unwrap();
    workspace
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
