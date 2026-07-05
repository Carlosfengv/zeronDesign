use anydesign_runtime::{
    agent_loop::AgentLoop,
    conversation::RuntimeStore,
    http_api::{self, AppState},
    model_gateway::{MockModelClient, ModelClient, ModelRequest, ModelResponse, ToolCall},
    recovery::{recover_interrupted_runs, RecoveryOutcome},
    types::{
        AgentCheckpoint, AgentEvent, AgentPhase, AgentRunStatus, Brief, BriefStatus, ContentSource,
        ProjectVersionStatus, SandboxBindingStatus, SandboxChannelProtocol,
    },
    RuntimeConfig,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

async fn create_run(store: &RuntimeStore) -> String {
    store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id
}

fn website_brief() -> Brief {
    Brief {
        project_type: "website".to_string(),
        audience: "enterprise designers".to_string(),
        content_hierarchy: vec!["hero".to_string(), "proof".to_string()],
        page_structure: json!([
            {
                "title": "Home",
                "purpose": "Explain the product",
                "keyContent": ["hero", "proof"]
            }
        ]),
        visual_direction: "quiet technical confidence".to_string(),
        recommended_template: "astro-website".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

#[tokio::test]
async fn partial_run_saves_checkpoint_and_updates_run_pointer() {
    let checkpoint_dir = unique_temp_dir("checkpoints-partial");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    let model = MockModelClient::new(vec![
        ModelResponse::TextOnly("Thinking".to_string()),
        ModelResponse::TextOnly("Still thinking".to_string()),
        ModelResponse::TextOnly("No tools".to_string()),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run_id).await.unwrap();

    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Partial);
    let checkpoint_id = run.checkpoint_id.expect("partial run should checkpoint");
    let checkpoint = store.get_checkpoint(&checkpoint_id).await.unwrap();
    assert_eq!(checkpoint.run_id, run_id);
    assert_eq!(checkpoint.project_id, "project-1");
    assert_eq!(checkpoint.phase, AgentPhase::Brief);
    assert!(checkpoint
        .message_window
        .iter()
        .any(|message| message["text"] == "No tools"));
    assert!(checkpoint
        .context_summary
        .contains("No tool calls for 3 consecutive turns"));
    assert!(store.checkpoint_path(&checkpoint_id).exists());
}

#[tokio::test]
async fn run_complete_partial_saves_checkpoint_with_tool_result() {
    let checkpoint_dir = unique_temp_dir("checkpoints-run-complete-partial");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-complete",
        "run.complete",
        json!({ "status": "partial", "summary": "Stopped with partial progress" }),
    )])]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run_id).await.unwrap();

    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Partial);
    let checkpoint_id = run.checkpoint_id.expect("partial run should checkpoint");
    let checkpoint = store.get_checkpoint(&checkpoint_id).await.unwrap();
    assert!(checkpoint
        .message_window
        .iter()
        .any(|message| message["toolUseId"] == "tool-complete"
            && message["toolName"] == "run.complete"
            && message["content"]["summary"] == "Stopped with partial progress"));
    assert_eq!(checkpoint.context_summary, "Stopped with partial progress");
}

#[tokio::test]
async fn checkpoint_file_can_be_reloaded_by_id() {
    let checkpoint_dir = unique_temp_dir("checkpoints-reload");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    let model = MockModelClient::new(vec![
        ModelResponse::TextOnly("One".to_string()),
        ModelResponse::TextOnly("Two".to_string()),
        ModelResponse::TextOnly("Three".to_string()),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));
    loop_runner.run(&run_id).await.unwrap();
    let checkpoint_id = store.get_run(&run_id).await.unwrap().checkpoint_id.unwrap();

    let reloaded_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let checkpoint = reloaded_store.get_checkpoint(&checkpoint_id).await.unwrap();

    assert_eq!(checkpoint.id, checkpoint_id);
    assert_eq!(checkpoint.run_id, run_id);
}

#[tokio::test]
async fn agent_events_are_appended_to_run_log_jsonl() {
    let checkpoint_dir = unique_temp_dir("checkpoints-run-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;

    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run_id.clone(),
            text: "Starting".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run_id.clone(),
            status: "completed".to_string(),
            summary: "Done".to_string(),
            timestamp: Utc::now(),
        })
        .await;

    let run_log_path = store.run_log_path(&run_id);
    assert!(run_log_path.ends_with(format!("{run_id}/run-log.jsonl")));
    let lines = fs::read_to_string(&run_log_path).unwrap();
    let events = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "agent.message");
    assert_eq!(events[0]["runId"], run_id);
    assert_eq!(events[1]["type"], "run.completed");
    assert_eq!(events[1]["summary"], "Done");

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    assert!(reloaded_store.run_log_path(&run_id).exists());
    let reloaded_events = reloaded_store.events(&run_id).await;
    assert_eq!(reloaded_events.len(), 2);
    assert!(matches!(
        reloaded_events[0],
        AgentEvent::AgentMessage { .. }
    ));
    assert!(matches!(
        reloaded_events[1],
        AgentEvent::RunCompleted { .. }
    ));
}

#[tokio::test]
async fn audit_records_are_appended_to_audit_log_jsonl() {
    let checkpoint_dir = unique_temp_dir("checkpoints-audit-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;

    store
        .append_audit_record(
            "project-1",
            &run_id,
            "fs.read",
            "path=/workspace/project/index.md",
            "allow",
            "workspace path allowed",
        )
        .await;
    store
        .append_audit_record(
            "project-1",
            &run_id,
            "shell.run",
            "argv=[kubectl get pods]",
            "deny",
            "command is not allowed",
        )
        .await;

    let audit_log_path = store.audit_log_path();
    assert!(audit_log_path.ends_with("audit-log.jsonl"));
    let lines = fs::read_to_string(&audit_log_path).unwrap();
    let records = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["tool"], "fs.read");
    assert_eq!(records[0]["decision"], "allow");
    assert_eq!(records[1]["tool"], "shell.run");
    assert_eq!(records[1]["decision"], "deny");

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded_records = reloaded_store.audit_records().await;
    assert_eq!(reloaded_records.len(), 2);
    assert_eq!(reloaded_records[0].tool, "fs.read");
    assert_eq!(reloaded_records[1].decision, "deny");
}

#[tokio::test]
async fn conversation_items_are_appended_to_project_jsonl() {
    let checkpoint_dir = unique_temp_dir("checkpoints-conversation-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;

    store
        .append_conversation_item(
            "project-1",
            Some(&run_id),
            "assistant_message",
            Some("assistant"),
            "Workspace change is ready",
            Some(json!({
                "sandboxName": "sandbox-project-1",
                "workspacePvcName": "workspace-project-1",
            })),
        )
        .await;

    let conversation_log_path = store.conversation_log_path("project-1");
    assert!(conversation_log_path.ends_with("project-1/conversation-items.jsonl"));
    let lines = fs::read_to_string(&conversation_log_path).unwrap();
    let items = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["projectId"], "project-1");
    assert_eq!(items[0]["runId"], run_id);
    assert_eq!(items[0]["kind"], "assistant_message");
    assert_eq!(
        items[0]["metadata"]["workspacePvcName"],
        "workspace-project-1"
    );

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded_items = reloaded_store.conversation_items("project-1").await;
    assert_eq!(reloaded_items.len(), 1);
    assert_eq!(reloaded_items[0].run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(reloaded_items[0].text, "Workspace change is ready");
}

#[tokio::test]
async fn content_sources_are_persisted_for_run_recovery() {
    let checkpoint_dir = unique_temp_dir("checkpoints-content-source-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![
                ContentSource::readable("source-1", "prompt", "Make a launch website"),
                ContentSource::readable("source-2", "design_md", "# Visual rules"),
                ContentSource {
                    id: "source-unreadable".to_string(),
                    kind: "attachment_text".to_string(),
                    text: "secret attachment body".to_string(),
                    readable: false,
                },
            ],
        )
        .await;

    let content_source_log_path = store.content_source_log_path();
    assert!(content_source_log_path.ends_with("content-sources.jsonl"));
    let snapshots = fs::read_to_string(&content_source_log_path).unwrap();
    assert!(snapshots.contains("\"runId\":\"run-1\""));
    assert!(snapshots.contains("Make a launch website"));
    assert!(snapshots.contains("\"readable\":false"));

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded_sources = reloaded_store.content_sources(&run.id).await;

    assert_eq!(reloaded_sources.len(), 3);
    assert_eq!(reloaded_sources[0].id, "source-1");
    assert_eq!(reloaded_sources[0].text, "Make a launch website");
    assert_eq!(reloaded_sources[1].kind, "design_md");
    assert!(!reloaded_sources[2].readable);
}

#[tokio::test]
async fn briefs_are_persisted_with_confirmation_status_after_restart() {
    let checkpoint_dir = unique_temp_dir("checkpoints-brief-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;
    let brief_id = store
        .write_brief_draft(&run_id, website_brief())
        .await
        .unwrap();

    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Draft)
    );
    store.confirm_brief(&run_id, &brief_id).await.unwrap();
    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );

    let brief_log_path = store.brief_log_path();
    assert!(brief_log_path.ends_with("briefs.jsonl"));
    let snapshots = fs::read_to_string(&brief_log_path).unwrap();
    assert!(snapshots.contains("\"status\":\"draft\""));
    assert!(snapshots.contains("\"status\":\"confirmed\""));
    assert!(snapshots.contains("\"recommendedTemplate\":\"astro-website\""));

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded_brief = reloaded_store.get_brief(&brief_id).await.unwrap();
    assert_eq!(reloaded_brief.project_type, "website");
    assert_eq!(reloaded_brief.recommended_template, "astro-website");
    assert_eq!(
        reloaded_store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );
    assert_eq!(
        reloaded_store
            .get_run(&run_id)
            .await
            .unwrap()
            .brief_version
            .as_deref(),
        Some(brief_id.as_str())
    );
}

#[tokio::test]
async fn sandbox_bindings_are_persisted_as_project_workspace_scope() {
    let checkpoint_dir = unique_temp_dir("checkpoints-sandbox-binding-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "claim-sandbox-pending".to_string(),
            "claim-project-1".to_string(),
            "workspace-project-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();

    store
        .update_sandbox_binding_runtime_identity(
            &binding.id,
            "actual-sandbox-project-1".to_string(),
            Some("workspace-channel-project-1".to_string()),
        )
        .await
        .unwrap();
    let ready_binding = store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();

    let binding_log_path = store.sandbox_binding_log_path();
    assert!(binding_log_path.ends_with("sandbox-bindings.jsonl"));
    let lines = fs::read_to_string(&binding_log_path).unwrap();
    let snapshots = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[0]["workspacePvcName"], "workspace-project-1");
    assert_eq!(snapshots[2]["status"], "ready");
    assert_eq!(snapshots[2]["sandboxName"], "actual-sandbox-project-1");

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded = reloaded_store
        .get_sandbox_binding(&binding.id)
        .await
        .unwrap();
    assert_eq!(reloaded.id, ready_binding.id);
    assert_eq!(reloaded.project_id, "project-1");
    assert_eq!(reloaded.workspace_pvc_name, "workspace-project-1");
    assert_eq!(reloaded.status, SandboxBindingStatus::Ready);
    assert_eq!(
        reloaded.channel_service_name.as_deref(),
        Some("workspace-channel-project-1")
    );

    let reused = reloaded_store
        .create_sandbox_binding(
            "project-2",
            "claim-sandbox-other".to_string(),
            "claim-project-2".to_string(),
            "workspace-project-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await;
    assert!(reused.is_err());
    assert!(reused
        .unwrap_err()
        .to_string()
        .contains("workspace PVC workspace-project-1 is already bound"));
}

#[tokio::test]
async fn project_versions_are_persisted_for_current_preview_after_restart() {
    let checkpoint_dir = unique_temp_dir("checkpoints-project-version-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/preview/project-1/version-1".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/version-1.tar".to_string()),
        )
        .await;
    let promoted = store
        .promote_project_version("project-1", &run_id, &candidate.id)
        .await
        .unwrap();

    assert_eq!(promoted.status, ProjectVersionStatus::Promoted);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().output_version_id,
        Some(candidate.id.clone())
    );
    let project_version_log_path = store.project_version_log_path();
    assert!(project_version_log_path.ends_with("project-versions.jsonl"));
    let snapshots = fs::read_to_string(&project_version_log_path).unwrap();
    assert!(snapshots.contains("\"status\":\"candidate\""));
    assert!(snapshots.contains("\"status\":\"promoted\""));
    assert!(snapshots.contains("file:///workspace/snapshots/version-1.tar"));

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let reloaded_version = reloaded_store
        .get_project_version(&candidate.id)
        .await
        .unwrap();
    assert_eq!(reloaded_version.status, ProjectVersionStatus::Promoted);
    assert_eq!(
        reloaded_version.source_snapshot_uri.as_deref(),
        Some("file:///workspace/snapshots/version-1.tar")
    );
    let current = reloaded_store
        .current_project_version("project-1")
        .await
        .unwrap();
    assert_eq!(current.id, candidate.id);
    assert_eq!(
        current.preview_url,
        "http://preview.local/preview/project-1/version-1"
    );
}

#[tokio::test]
async fn agent_runs_are_persisted_and_recovered_after_store_restart() {
    let checkpoint_dir = unique_temp_dir("checkpoints-run-state-log");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let run_id = create_run(&store).await;
    store
        .update_run_status(&run_id, AgentRunStatus::Running)
        .await
        .unwrap();
    let checkpoint = AgentCheckpoint {
        id: "checkpoint-recover-from-run-log-1".to_string(),
        run_id: run_id.clone(),
        project_id: "project-1".to_string(),
        phase: AgentPhase::Brief,
        message_window: vec![json!({ "role": "assistant", "text": "resume from persisted run" })],
        conversation_range: None,
        task_list: vec![],
        workspace_snapshot_uri: Some("file:///workspace/snapshots/recover.tar".to_string()),
        build_result: None,
        brief_version: Some("brief-1".to_string()),
        design_version: None,
        last_known_preview_url: Some("http://preview.local/preview/project-1/current".to_string()),
        context_summary: "persisted running run".to_string(),
        created_at: Utc::now(),
    };
    store.save_checkpoint(checkpoint.clone()).await.unwrap();

    let run_state_log_path = store.run_state_log_path();
    assert!(run_state_log_path.ends_with("runs.jsonl"));
    let snapshots = fs::read_to_string(&run_state_log_path).unwrap();
    assert!(snapshots.contains("\"status\":\"running\""));
    assert!(snapshots.contains("checkpoint-recover-from-run-log-1"));

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    assert_eq!(
        reloaded_store
            .get_run(&run_id)
            .await
            .unwrap()
            .checkpoint_id
            .as_deref(),
        Some("checkpoint-recover-from-run-log-1")
    );

    let outcomes = recover_interrupted_runs(&reloaded_store).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(
        &outcomes[0],
        RecoveryOutcome::Resumed {
            run_id: recovered_run_id,
            checkpoint: recovered_checkpoint,
        } if recovered_run_id == &run_id && recovered_checkpoint.id == checkpoint.id
    ));
    let recovered = reloaded_store.get_run(&run_id).await.unwrap();
    assert_eq!(recovered.status, AgentRunStatus::Running);
    assert_eq!(
        recovered.checkpoint_id.as_deref(),
        Some("checkpoint-recover-from-run-log-1")
    );
}

#[tokio::test]
async fn runtime_restart_advances_id_counter_from_persisted_state() {
    let checkpoint_dir = unique_temp_dir("checkpoints-id-counter");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let first = create_run(&store).await;
    let second = create_run(&store).await;
    assert_eq!(first, "run-1");
    assert_eq!(second, "run-4");

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let third = create_run(&reloaded_store).await;

    assert_eq!(third, "run-7");
    assert_eq!(reloaded_store.get_run(&first).await.unwrap().id, first);
    assert_eq!(reloaded_store.get_run(&second).await.unwrap().id, second);
}

#[tokio::test]
async fn persisted_active_run_keeps_workspace_binding_exclusive_after_restart() {
    let checkpoint_dir = unique_temp_dir("checkpoints-workspace-active-after-restart");
    let run_log_dir = checkpoint_dir.join("logs");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let active_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let waiting_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-workspace-active".to_string(),
            "sandbox-claim-workspace-active".to_string(),
            "workspace-project-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&active_run.id, &binding.id)
        .await
        .unwrap();
    store
        .update_run_status(&active_run.id, AgentRunStatus::Running)
        .await
        .unwrap();

    let reloaded_store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let unavailable = reloaded_store
        .ensure_sandbox_binding_available(&binding.id, None)
        .await;
    assert!(unavailable.is_err());
    assert!(unavailable
        .unwrap_err()
        .to_string()
        .contains("already in use by active run"));

    reloaded_store
        .bind_run_to_sandbox(&waiting_run.id, &binding.id)
        .await
        .unwrap();
    let acquire = reloaded_store
        .acquire_sandbox_binding_for_run(&waiting_run.id, None)
        .await;
    assert!(acquire.is_err());
    assert!(acquire
        .unwrap_err()
        .to_string()
        .contains("already in use by active run"));
}

#[tokio::test]
async fn latest_checkpoint_for_run_uses_most_recent_saved_checkpoint() {
    let checkpoint_dir = unique_temp_dir("checkpoints-latest");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    let model = MockModelClient::new(vec![
        ModelResponse::TextOnly("First".to_string()),
        ModelResponse::TextOnly("Second".to_string()),
        ModelResponse::TextOnly("Third".to_string()),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));
    loop_runner.run(&run_id).await.unwrap();

    let latest = store.latest_checkpoint_for_run(&run_id).await.unwrap();
    let run = store.get_run(&run_id).await.unwrap();

    assert_eq!(Some(latest.id), run.checkpoint_id);
}

#[tokio::test]
async fn agent_loop_saves_checkpoint_after_each_turn_transcript() {
    let checkpoint_dir = unique_temp_dir("checkpoints-each-turn");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-list",
            "content.list_sources",
            json!({}),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "done" }),
        )]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run_id).await.unwrap();

    let mut checkpoints = fs::read_dir(&checkpoint_dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("checkpoint-") && name.ends_with(".json"))
                .then_some(path)
        })
        .map(|path| {
            serde_json::from_str::<AgentCheckpoint>(&fs::read_to_string(path).unwrap()).unwrap()
        })
        .filter(|checkpoint| checkpoint.run_id == run_id)
        .collect::<Vec<_>>();
    checkpoints.sort_by_key(|checkpoint| checkpoint.created_at);

    assert!(checkpoints.iter().any(|checkpoint| {
        checkpoint.context_summary == "turn 1 transcript captured"
            && checkpoint.message_window.iter().any(|message| {
                message["role"] == "tool"
                    && message["toolUseId"] == "tool-list"
                    && message["toolName"] == "content.list_sources"
            })
    }));
    let latest = checkpoints.last().expect("checkpoint should exist");
    assert_eq!(latest.context_summary, "turn 2 transcript captured");
    let range = latest
        .conversation_range
        .as_ref()
        .expect("turn checkpoint should preserve a conversation range");
    assert_eq!(
        range.end_index_exclusive,
        latest.message_window.len() as u64
    );
    assert_eq!(range.retained_count, latest.message_window.len() as u64);
    assert!(latest.message_window.iter().any(|message| {
        message["role"] == "tool"
            && message["toolUseId"] == "tool-complete"
            && message["toolName"] == "run.complete"
            && message["content"]["summary"] == "done"
    }));
    assert_eq!(
        store
            .get_run(&run_id)
            .await
            .unwrap()
            .checkpoint_id
            .as_deref(),
        Some(latest.id.as_str())
    );
}

#[tokio::test]
async fn checkpoint_message_window_is_bounded() {
    let checkpoint_dir = unique_temp_dir("checkpoints-window");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    let mut responses = Vec::new();
    for index in 0..25 {
        responses.push(ModelResponse::TextOnly(format!("message {index}")));
    }
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(MockModelClient::new(responses)));

    loop_runner.run(&run_id).await.unwrap();

    let checkpoint_id = store.get_run(&run_id).await.unwrap().checkpoint_id.unwrap();
    let checkpoint_json = fs::read_to_string(store.checkpoint_path(&checkpoint_id)).unwrap();
    let checkpoint: Value = serde_json::from_str(&checkpoint_json).unwrap();

    assert!(checkpoint["messageWindow"].as_array().unwrap().len() <= 20);
}

#[tokio::test]
async fn runtime_restart_resumes_running_run_from_latest_checkpoint() {
    let checkpoint_dir = unique_temp_dir("checkpoints-recover");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    store
        .update_run_status(&run_id, AgentRunStatus::Running)
        .await
        .unwrap();
    let checkpoint = AgentCheckpoint {
        id: "checkpoint-recover-1".to_string(),
        run_id: run_id.clone(),
        project_id: "project-1".to_string(),
        phase: AgentPhase::Brief,
        message_window: vec![json!({ "role": "assistant", "text": "resume from here" })],
        conversation_range: None,
        task_list: vec![],
        workspace_snapshot_uri: Some("file:///workspace/snapshots/recover.tar".to_string()),
        build_result: None,
        brief_version: Some("brief-1".to_string()),
        design_version: None,
        last_known_preview_url: Some("http://preview.local/preview/project-1/current".to_string()),
        context_summary: "turn 2 starting".to_string(),
        created_at: Utc::now(),
    };
    store.save_checkpoint(checkpoint.clone()).await.unwrap();

    let outcomes = recover_interrupted_runs(&store).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(
        &outcomes[0],
        RecoveryOutcome::Resumed {
            run_id: recovered_run_id,
            checkpoint: recovered_checkpoint,
        } if recovered_run_id == &run_id && recovered_checkpoint.id == checkpoint.id
    ));
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Running);
    assert_eq!(run.checkpoint_id.as_deref(), Some("checkpoint-recover-1"));
    assert!(store.events(&run_id).await.iter().any(|event| {
        let event = serde_json::to_value(event).unwrap();
        event["type"] == "state.changed"
            && event["state"] == "recovered_from_checkpoint:checkpoint-recover-1"
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "progress"
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["checkpointId"] == "checkpoint-recover-1")));
}

#[tokio::test]
async fn startup_recovery_spawns_run_with_checkpoint_message_window() {
    let checkpoint_dir = unique_temp_dir("checkpoints-startup-recovery");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    store
        .save_checkpoint(AgentCheckpoint {
            id: "checkpoint-startup-recovery-1".to_string(),
            run_id: run.id.clone(),
            project_id: run.project_id.clone(),
            phase: AgentPhase::Export,
            message_window: vec![json!({
                "role": "assistant",
                "text": "resume this exact transcript"
            })],
            conversation_range: None,
            task_list: vec![],
            workspace_snapshot_uri: None,
            build_result: None,
            brief_version: None,
            design_version: None,
            last_known_preview_url: None,
            context_summary: "startup recovery checkpoint".to_string(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let reloaded_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let captured_requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModelClient::new(
        vec![ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered run completed" }),
        )])],
        captured_requests.clone(),
    );
    let state = AppState {
        config: RuntimeConfig::from_env(),
        store: reloaded_store.clone(),
        model: Arc::new(model),
    };

    http_api::recover_startup_runs(state).await.unwrap();
    for _ in 0..50 {
        if reloaded_store
            .get_run(&run.id)
            .await
            .is_some_and(|run| run.status == AgentRunStatus::Completed)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(
        reloaded_store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    let requests = captured_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .messages
        .iter()
        .any(|message| message["text"] == "resume this exact transcript"));
    assert!(reloaded_store.events(&run.id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "recovered_from_checkpoint:checkpoint-startup-recovery-1"
        )
    }));
}

#[tokio::test]
async fn runtime_restart_reacquires_ready_sandbox_before_resuming_checkpoint() {
    let checkpoint_dir = unique_temp_dir("checkpoints-recover-sandbox");
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
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-recover".to_string(),
            "sandbox-claim-recover".to_string(),
            "workspace-sandbox-claim-recover".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    let checkpoint = AgentCheckpoint {
        id: "checkpoint-recover-sandbox-1".to_string(),
        run_id: run.id.clone(),
        project_id: "project-1".to_string(),
        phase: AgentPhase::Build,
        message_window: vec![json!({ "role": "assistant", "text": "resume build" })],
        conversation_range: None,
        task_list: vec![],
        workspace_snapshot_uri: Some("file:///workspace/snapshots/recover-build.tar".to_string()),
        build_result: None,
        brief_version: Some("brief-1".to_string()),
        design_version: None,
        last_known_preview_url: Some("http://preview.local/preview/project-1/current".to_string()),
        context_summary: "build checkpoint".to_string(),
        created_at: Utc::now(),
    };
    store.save_checkpoint(checkpoint).await.unwrap();

    let outcomes = recover_interrupted_runs(&store).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(&outcomes[0], RecoveryOutcome::Resumed { run_id, .. } if run_id == &run.id));
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Running
    );
}

#[tokio::test]
async fn runtime_restart_fails_sandbox_run_when_binding_is_unavailable() {
    let checkpoint_dir = unique_temp_dir("checkpoints-recover-sandbox-failed");
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
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-failed".to_string(),
            "sandbox-claim-failed".to_string(),
            "workspace-sandbox-claim-failed".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Failed)
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    let checkpoint = AgentCheckpoint {
        id: "checkpoint-recover-sandbox-failed-1".to_string(),
        run_id: run.id.clone(),
        project_id: "project-1".to_string(),
        phase: AgentPhase::Build,
        message_window: vec![json!({ "role": "assistant", "text": "resume build" })],
        conversation_range: None,
        task_list: vec![],
        workspace_snapshot_uri: Some("file:///workspace/snapshots/recover-build.tar".to_string()),
        build_result: None,
        brief_version: Some("brief-1".to_string()),
        design_version: None,
        last_known_preview_url: Some("http://preview.local/preview/project-1/current".to_string()),
        context_summary: "build checkpoint".to_string(),
        created_at: Utc::now(),
    };
    store.save_checkpoint(checkpoint).await.unwrap();

    let outcomes = recover_interrupted_runs(&store).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(
        &outcomes[0],
        RecoveryOutcome::Failed {
            run_id,
            preserved_checkpoint_id,
            reason,
        } if run_id == &run.id
            && preserved_checkpoint_id.as_deref() == Some("checkpoint-recover-sandbox-failed-1")
            && reason.contains("sandbox workspace is unavailable")
    ));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Failed
    );
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Failed
    );
    assert!(store.events(&run.id).await.iter().any(|event| {
        let event = serde_json::to_value(event).unwrap();
        event["type"] == "run.completed" && event["status"] == "failed"
    }));
}

#[tokio::test]
async fn runtime_restart_marks_running_run_failed_when_checkpoint_is_missing() {
    let checkpoint_dir = unique_temp_dir("checkpoints-recover-missing");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run_id = create_run(&store).await;
    store
        .update_run_status(&run_id, AgentRunStatus::Running)
        .await
        .unwrap();

    let outcomes = recover_interrupted_runs(&store).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(
        &outcomes[0],
        RecoveryOutcome::Failed {
            run_id: failed_run_id,
            preserved_checkpoint_id: None,
            reason,
        } if failed_run_id == &run_id && reason.contains("no checkpoint")
    ));
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    assert_eq!(run.project_id, "project-1");
    assert!(!run.input_message_ids.is_empty());
    assert!(store.events(&run_id).await.iter().any(|event| {
        let event = serde_json::to_value(event).unwrap();
        event["type"] == "run.completed" && event["status"] == "failed"
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "error_summary"
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["recoverable"] == true)));
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

#[derive(Debug, Clone)]
struct RecordingModelClient {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModelClient {
    fn new(responses: Vec<ModelResponse>, requests: Arc<Mutex<Vec<ModelRequest>>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            requests,
        }
    }
}

#[async_trait]
impl ModelClient for RecordingModelClient {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().await.push(request);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            anyhow::bail!("recording model response queue exhausted");
        }
        Ok(responses.remove(0))
    }
}
