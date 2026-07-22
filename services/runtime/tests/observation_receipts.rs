use anydesign_runtime::{
    model_gateway::ToolCall,
    tools::control_plane::control_plane_executor,
    tools::streaming::StreamingToolExecutor,
    types::{AgentEvent, AgentPhase, ObservationOutcome, ObservationPurpose, ObservationView},
    RuntimeStore,
};
use chrono::Utc;
use serde_json::json;
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "observation-receipts-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}

#[tokio::test]
async fn provider_parallel_duplicate_reads_are_serialized_into_one_full_delivery() {
    let root = root();
    let workspace = root.join("workspace");
    let runtime = root.join("runtime");
    fs::create_dir_all(workspace.join("project/app")).unwrap();
    fs::write(
        workspace.join("project/app/page.tsx"),
        "export default function Page() { return <main>Parallel</main>; }\n",
    )
    .unwrap();
    let store = RuntimeStore::with_checkpoint_dir(&runtime);
    let run = store
        .create_run(
            "project-parallel".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;
    let executor = control_plane_executor()
        .with_workspace_root(&workspace)
        .with_observation_receipts_enabled(true);
    let streaming = StreamingToolExecutor::new(executor);

    let results = streaming
        .execute_calls(
            store.clone(),
            &run.id,
            vec![
                ToolCall::new(
                    "parallel-read-1",
                    "fs.read",
                    json!({ "path": "project/app/page.tsx" }),
                ),
                ToolCall::new(
                    "parallel-read-2",
                    "fs.read",
                    json!({ "path": "project/app/page.tsx" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.content.get("text").is_some());
    assert_eq!(results[1].result.content["unchanged"], true);
    let receipts = store
        .events(&run.id)
        .await
        .into_iter()
        .filter(|event| matches!(event, AgentEvent::ObservationReceipt { .. }))
        .count();
    assert_eq!(receipts, 2);
}

#[tokio::test]
async fn repeated_full_reads_return_unchanged_stub_until_context_epoch_advances() {
    let root = root();
    let workspace = root.join("workspace");
    let runtime = root.join("runtime");
    fs::create_dir_all(workspace.join("project/app")).unwrap();
    fs::write(
        workspace.join("project/app/page.tsx"),
        "export default function Page() { return <main>Hello</main>; }\n",
    )
    .unwrap();
    let store = RuntimeStore::with_checkpoint_dir(&runtime);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;
    store
        .append_event(AgentEvent::ModelTurnStarted {
            run_id: run.id.clone(),
            turn: 1,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let executor = control_plane_executor()
        .with_workspace_root(&workspace)
        .with_observation_receipts_enabled(true);

    let first = executor
        .execute(
            store.clone(),
            &run.id,
            "read-1",
            "fs.read",
            json!({ "path": "project/app/page.tsx" }),
        )
        .await;
    let second = executor
        .execute(
            store.clone(),
            &run.id,
            "read-2",
            "fs.read",
            json!({ "path": "project/app/page.tsx" }),
        )
        .await;

    assert!(!first.result.is_error);
    assert!(!second.result.is_error);
    assert!(first.result.content.get("text").is_some());
    assert_eq!(second.result.content["unchanged"], true);
    assert!(second.result.content.get("text").is_none());
    let receipts = store
        .events(&run.id)
        .await
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::ObservationReceipt { receipt, .. } => Some(receipt),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].normalized_path, "project/app/page.tsx");
    assert_eq!(receipts[0].view, ObservationView::Full);
    assert_eq!(
        receipts[0].last_outcome,
        ObservationOutcome::ContentReturned
    );
    assert_eq!(receipts[0].purpose, ObservationPurpose::Source);
    assert_eq!(receipts[0].read_count, 1);
    assert!(!receipts[0].duplicate_delivery);
    assert_eq!(receipts[1].read_count, 2);
    assert!(receipts[1].duplicate_delivery);
    assert_eq!(receipts[1].content_sha256, receipts[0].content_sha256);
    assert_eq!(receipts[1].last_outcome, ObservationOutcome::Unchanged);
    assert_eq!(receipts[1].estimated_tokens, 0);

    store
        .append_event(AgentEvent::MetricRecorded {
            run_id: run.id.clone(),
            name: "context_window_epoch_advanced".to_string(),
            value: 1,
            metadata: Some(json!({ "epoch": 1 })),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let third = executor
        .execute(
            store.clone(),
            &run.id,
            "read-3",
            "fs.read",
            json!({ "path": "project/app/page.tsx" }),
        )
        .await;
    assert!(!third.result.is_error);
    assert!(third.result.content.get("text").is_some());
}
