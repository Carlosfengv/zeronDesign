use anydesign_runtime::{
    agent_loop::AgentLoop,
    conversation::RuntimeStore,
    model_gateway::{MockModelClient, ModelClient, ModelRequest, ModelResponse, ToolCall},
    permission::{PermissionReason, PermissionResult, PermissionRules},
    tools::sandbox::sandbox_tools,
    tools::{
        control_plane::control_plane_executor,
        runtime::{
            InterruptBehavior, ProgressSink, Tool, ToolContext, ToolError, ToolExecutor, ToolResult,
        },
    },
    types::{AgentEvent, AgentPhase, AgentRunStatus, Brief, ContentSource},
};
use anyhow::anyhow;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::VecDeque, fs, path::PathBuf, sync::Arc};
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::Mutex, task::JoinHandle};

#[tokio::test]
async fn build_run_bootstraps_confirmed_brief_into_workspace_before_model_turn() {
    let workspace = unique_temp_dir("agent-loop-bootstrap");
    fs::create_dir_all(workspace.join("inputs")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "internal design teams".to_string(),
                content_hierarchy: vec!["Runtime reliability".to_string()],
                page_structure: json!([
                    {
                        "title": "Home",
                        "purpose": "Explain runtime scope",
                        "keyContent": ["hero"]
                    }
                ]),
                visual_direction: "precise and calm".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec!["Use the internal brand system.".to_string()],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    let build_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![
                ContentSource::readable("source-1", "prompt", "Build the runtime page"),
                ContentSource::readable("source-2", "design_md", "# Visual rules"),
                ContentSource {
                    id: "source-unreadable".to_string(),
                    kind: "attachment_text".to_string(),
                    text: "should not enter workspace".to_string(),
                    readable: false,
                },
            ],
            Some(brief_id.clone()),
            None,
        )
        .await;
    let model = MockModelClient::new(vec![ModelResponse::Error(
        "stop after bootstrap assertion".to_string(),
    )]);
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&build_run.id).await.unwrap();

    let brief_md = fs::read_to_string(workspace.join("inputs/brief.md")).unwrap();
    assert!(brief_md.contains(&format!("# Brief {brief_id}")));
    assert!(brief_md.contains("Runtime reliability"));
    assert_eq!(
        fs::read_to_string(workspace.join("inputs/design.md")).unwrap(),
        "# Visual rules"
    );
    let content_sources =
        fs::read_to_string(workspace.join("inputs/content-sources.json")).unwrap();
    assert!(content_sources.contains("Build the runtime page"));
    assert!(content_sources.contains("# Visual rules"));
    assert!(!content_sources.contains("should not enter workspace"));
    assert_eq!(
        fs::read_to_string(workspace.join("state/tasks.json")).unwrap(),
        "[]"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("state/preview.json")).unwrap(),
        "{}"
    );
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.text == "Workspace inputs prepared for sandbox execution."));
    let events = store.events(&build_run.id).await;
    let mut started = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolStarted {
                tool_use_id, tool, ..
            } if tool == "fs.write" && tool_use_id.starts_with("bootstrap:") => {
                Some(tool_use_id.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut completed = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCompleted {
                tool_use_id, tool, ..
            } if tool == "fs.write" && tool_use_id.starts_with("bootstrap:") => {
                Some(tool_use_id.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    started.sort();
    completed.sort();
    assert_eq!(started, completed);
    assert_eq!(started.len(), 5);
    assert!(started.contains(&"bootstrap:inputs/brief.md".to_string()));
    assert!(started.contains(&"bootstrap:inputs/content-sources.json".to_string()));
    assert!(started.contains(&"bootstrap:inputs/design.md".to_string()));
    assert!(started.contains(&"bootstrap:state/tasks.json".to_string()));
    assert!(started.contains(&"bootstrap:state/preview.json".to_string()));
}

#[tokio::test]
async fn bootstrap_workspace_failure_emits_tool_failed_before_run_failed() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "internal design teams".to_string(),
                content_hierarchy: vec!["Runtime reliability".to_string()],
                page_structure: json!([
                    {
                        "title": "Home",
                        "purpose": "Explain runtime scope",
                        "keyContent": ["hero"]
                    }
                ]),
                visual_direction: "precise and calm".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    let build_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let executor = ToolExecutor::new(
        vec![Arc::new(RecoverableFsWriteTool)],
        PermissionRules::default(),
    );
    let loop_runner = AgentLoop::with_tool_executor(
        store.clone(),
        Arc::new(MockModelClient::new(vec![])),
        executor,
    );

    loop_runner.run(&build_run.id).await.unwrap();

    let run = store.get_run(&build_run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    let events = store.events(&build_run.id).await;
    let event_types = events
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    let started_index = event_types
        .iter()
        .position(|event| event == "tool.started")
        .expect("bootstrap fs.write should emit tool.started");
    let failed_index = event_types
        .iter()
        .position(|event| event == "tool.failed")
        .expect("bootstrap fs.write should emit tool.failed");
    let completed_index = event_types
        .iter()
        .position(|event| event == "run.completed")
        .expect("failed bootstrap should emit run.completed");
    assert!(started_index < failed_index);
    assert!(failed_index < completed_index);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolFailed {
            tool,
            tool_use_id,
            recoverable: true,
            ..
        } if tool == "fs.write" && tool_use_id == "bootstrap:inputs/brief.md"
    )));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "tool_failed"
            && item.metadata.as_ref().is_some_and(|metadata| {
                metadata["tool"] == "fs.write"
                    && metadata["toolUseId"] == "bootstrap:inputs/brief.md"
                    && metadata["recoverable"] == true
            })));
}

#[tokio::test]
async fn run_cannot_complete_without_run_complete() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::TextOnly("The brief is ready.".to_string()),
        ModelResponse::TextOnly("But no completion tool was called.".to_string()),
        ModelResponse::TextOnly("Still no completion tool.".to_string()),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_ne!(run.status, AgentRunStatus::Completed);
    assert_eq!(run.status, AgentRunStatus::Partial);
}

#[tokio::test]
async fn three_consecutive_empty_turns_transition_to_partial() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::TextOnly("Thinking".to_string()),
        ModelResponse::TextOnly("Still thinking".to_string()),
        ModelResponse::TextOnly("No tools".to_string()),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Partial);
    let events = store.events(&run.id).await;
    assert!(events
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["status"] == "partial"));
}

#[tokio::test]
async fn model_error_marks_run_failed() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let model = MockModelClient::new(vec![ModelResponse::Error("model unavailable".to_string())]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
}

#[tokio::test]
async fn model_error_after_tool_use_emits_missing_tool_result_before_failure() {
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
    let model = MockModelClient::new(vec![ModelResponse::ToolCallsThenError {
        calls: vec![ToolCall::new("tool-open", "safe.pending", json!({}))],
        error: "model stream disconnected".to_string(),
    }]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    let results = loop_runner.run(&run.id).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_use_id, "tool-open");
    assert!(results[0].is_error);
    assert!(results[0].content["error"]
        .as_str()
        .unwrap()
        .contains("model stream disconnected"));

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    let event_types = store
        .events(&run.id)
        .await
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    let failed_index = event_types
        .iter()
        .position(|event| event == "tool.failed")
        .expect("missing tool result should emit tool.failed");
    let completed_index = event_types
        .iter()
        .position(|event| event == "run.completed")
        .expect("failed run should emit run.completed");
    assert!(
        failed_index < completed_index,
        "missing tool_result must be emitted before failed run completion: {event_types:?}"
    );
}

#[tokio::test]
async fn fallback_discards_old_tool_attempt_and_continues_next_turn() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCallsThenFallback {
            calls: vec![ToolCall::new(
                "tool-stale",
                "stale.should_not_run",
                json!({}),
            )],
            reason: "primary model overloaded".to_string(),
        },
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Fallback completed the run" }),
        )]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    let results = loop_runner.run(&run.id).await.unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_use_id, "tool-stale");
    assert!(results[0].is_error);
    assert!(results[0].content["error"]
        .as_str()
        .unwrap()
        .contains("primary model overloaded"));
    assert_eq!(results[1].tool_use_id, "tool-complete");
    assert!(!results[1].is_error);

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    let events = store.events(&run.id).await;
    let stale_completed = events.iter().any(|event| match event {
        anydesign_runtime::types::AgentEvent::ToolCompleted { tool_use_id, .. } => {
            tool_use_id == "tool-stale"
        }
        _ => false,
    });
    assert!(
        !stale_completed,
        "discarded fallback attempt must not complete"
    );
    let event_types = events
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    let failed_index = event_types
        .iter()
        .position(|event| event == "tool.failed")
        .expect("discarded attempt should emit synthetic tool.failed");
    let completed_index = event_types
        .iter()
        .position(|event| event == "run.completed")
        .expect("fallback run should complete");
    assert!(
        failed_index < completed_index,
        "discarded tool_result must land before fallback completion: {event_types:?}"
    );
}

#[tokio::test]
async fn tool_and_run_events_are_written_to_conversation_items() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-missing",
            "missing.tool",
            json!({}),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Conversation visible completion" }),
        )]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| {
        item.kind == "tool_failed"
            && item.text.contains("missing.tool failed")
            && item.metadata.as_ref().is_some_and(|metadata| {
                metadata["tool"] == "missing.tool" && metadata["toolUseId"] == "tool-missing"
            })
    }));
    assert!(conversation.iter().any(|item| {
        item.kind == "tool_completed"
            && item.text == "Completed run"
            && item.metadata.as_ref().is_some_and(|metadata| {
                metadata["tool"] == "run.complete" && metadata["toolUseId"] == "tool-complete"
            })
    }));
    assert!(conversation.iter().any(|item| {
        item.kind == "run_completed"
            && item.text == "Conversation visible completion"
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["status"] == "completed")
    }));
}

#[tokio::test]
async fn agent_loop_sends_messages_and_tool_snapshot_to_model_gateway() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let captured_requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModelClient::new(
        vec![
            ModelResponse::ToolCalls(vec![ToolCall::new(
                "tool-list",
                "content.list_sources",
                json!({}),
            )]),
            ModelResponse::ToolCalls(vec![ToolCall::new(
                "tool-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Context sent" }),
            )]),
        ],
        captured_requests.clone(),
    );
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let requests = captured_requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].run_id, run.id);
    assert_eq!(requests[0].model, "internal-balanced");
    assert_eq!(requests[0].phase, AgentPhase::Export);
    assert_eq!(requests[0].agent_profile, "export");
    assert!(requests[0]
        .system_prompt
        .contains("AnyDesign runtime export agent"));
    assert!(requests[0].messages.is_empty());
    assert!(requests[0]
        .tools
        .iter()
        .any(|tool| tool.name == "content.list_sources"));
    assert!(requests[0]
        .tools
        .iter()
        .any(|tool| tool.name == "run.complete"
            && tool.input_schema["properties"]["status"]["type"] == "string"));
    assert!(requests[0]
        .deferred_tools
        .iter()
        .all(|tool| !tool.name.is_empty()));
    assert!(requests[1].messages.iter().any(|message| {
        message["role"] == "tool"
            && message["toolUseId"] == "tool-list"
            && message["toolName"] == "content.list_sources"
    }));
}

#[tokio::test]
async fn agent_loop_deterministically_compacts_history_to_workspace_context() {
    let workspace = unique_temp_dir("agent-loop-compact");
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let mut responses = Vec::new();
    for index in 0..9 {
        responses.push(ModelResponse::ToolCalls(vec![ToolCall::new(
            format!("tool-missing-{index}"),
            "missing.tool",
            json!({ "index": index }),
        )]));
    }
    responses.push(ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-complete",
        "run.complete",
        json!({ "status": "completed", "summary": "Compacted run completed" }),
    )]));
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(
        store.clone(),
        Arc::new(MockModelClient::new(responses)),
        executor,
    );

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    let context = fs::read_to_string(workspace.join("state/context.md")).unwrap();
    assert!(context.contains("# Runtime Context Compact"));
    assert!(context.contains("## Previous Compact"));
    assert!(context.contains("tool-missing-0"));
    assert!(context.contains("tool-missing-6"));
    assert!(context.contains("Compacted messages:"));
    let checkpoint = store
        .get_checkpoint(run.checkpoint_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(checkpoint
        .message_window
        .iter()
        .any(|message| message["kind"] == "compact_summary"));
}

#[tokio::test]
async fn terminal_tool_error_marks_tool_failed_as_not_recoverable() {
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
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-terminal",
        "terminal.fail",
        json!({}),
    )])]);
    let executor = ToolExecutor::new(vec![Arc::new(TerminalFailTool)], PermissionRules::default());
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    let failed = store
        .events(&run.id)
        .await
        .into_iter()
        .find_map(|event| match event {
            anydesign_runtime::types::AgentEvent::ToolFailed { recoverable, .. } => {
                Some(recoverable)
            }
            _ => None,
        })
        .expect("terminal tool failure should emit tool.failed");
    assert!(!failed);
}

#[tokio::test]
async fn shell_non_zero_exit_emits_recoverable_tool_failed_event() {
    let workspace = unique_temp_dir("agent-loop-shell");
    fs::create_dir_all(workspace.join("project")).unwrap();
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
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-shell",
        "shell.run",
        json!({
            "argv": ["node", "-e", "process.stderr.write('build failed'); process.exit(5)"],
            "cwd": "project"
        }),
    )])]);
    let executor = ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        PermissionRules::default(),
        &workspace,
    );
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&run.id).await.unwrap();

    let failed = store
        .events(&run.id)
        .await
        .into_iter()
        .find_map(|event| match event {
            anydesign_runtime::types::AgentEvent::ToolFailed {
                error, recoverable, ..
            } => Some((error, recoverable)),
            _ => None,
        })
        .expect("shell non-zero exit should emit tool.failed");
    assert!(failed.0.contains("status Some(5)"));
    assert!(failed.0.contains("build failed"));
    assert!(failed.1);
}

#[tokio::test]
async fn continue_interrupt_synthetic_failure_is_not_recoverable_in_events() {
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
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-interrupt",
            "interrupt.cancel",
            json!({}),
        )]),
        ModelResponse::Error("stop after interrupt assertion".to_string()),
    ]);
    let executor = ToolExecutor::new(
        vec![Arc::new(InterruptCancelTool)],
        PermissionRules::default(),
    );
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&run.id).await.unwrap();

    let failed = store
        .events(&run.id)
        .await
        .into_iter()
        .find_map(|event| match event {
            anydesign_runtime::types::AgentEvent::ToolFailed {
                tool,
                error,
                recoverable,
                ..
            } if tool == "interrupt.cancel" => Some((error, recoverable)),
            _ => None,
        })
        .expect("synthetic interruption should emit tool.failed");
    assert!(failed.0.contains("new user message"));
    assert!(!failed.1);
}

#[tokio::test]
async fn tool_driven_build_run_promotes_preview_before_completion() {
    let workspace = unique_temp_dir("agent-loop-tool-build");
    fs::create_dir_all(workspace.join("project/src/pages")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::create_dir_all(workspace.join("outputs/screenshots")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    let (preview_url, _preview_server) = start_preview_server().await;
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
    let build_script = "const fs=require('fs');\
fs.mkdirSync('../outputs/build',{recursive:true});\
fs.mkdirSync('dist',{recursive:true});\
fs.writeFileSync('../outputs/build/build.log','Build ok\\n');\
fs.writeFileSync('dist/index.html','<!doctype html><title>ok</title>');";
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
        ToolCall::new(
            "tool-rebuilding",
            "preview.rebuilding",
            json!({ "previousVersionId": Value::Null }),
        ),
        ToolCall::new(
            "tool-package",
            "fs.write",
            json!({
                "path": "project/package.json",
                "text": serde_json::to_string_pretty(&json!({
                    "type": "module",
                    "scripts": { "build": "node -e \"console.log('build')\"" }
                })).unwrap()
            }),
        ),
        ToolCall::new(
            "tool-index",
            "fs.write",
            json!({ "path": "project/src/pages/index.astro", "text": "<h1>Design runtime</h1>" }),
        ),
        ToolCall::new(
            "tool-build",
            "shell.run",
            json!({ "argv": ["node", "-e", build_script], "cwd": "project" }),
        ),
        ToolCall::new(
            "tool-preview",
            "preview.start",
            json!({ "url": preview_url, "port": 4321 }),
        ),
        ToolCall::new(
            "tool-browser",
            "browser.open",
            json!({ "url": "http://127.0.0.1:4321" }),
        ),
        ToolCall::new(
            "tool-shot",
            "browser.screenshot",
            json!({ "screenshotId": "shot-tool-build", "blank": false }),
        ),
        ToolCall::new(
            "tool-candidate",
            "preview.report_candidate",
            json!({
                "url": preview_url,
                "screenshotId": "shot-tool-build",
                "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshot.txt"
            }),
        ),
        ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Astro preview promoted" }),
        ),
    ])]);
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    let results = loop_runner.run(&run.id).await.unwrap();

    assert!(results.iter().all(|result| !result.is_error));
    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert!(run.output_version_id.is_some());
    assert!(workspace.join("project/dist/index.html").exists());

    let event_types = store
        .events(&run.id)
        .await
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    let updated_index = event_types
        .iter()
        .position(|event| event == "preview.updated")
        .expect("preview.updated should be emitted");
    let completed_index = event_types
        .iter()
        .position(|event| event == "run.completed")
        .expect("run.completed should be emitted");
    assert!(
        updated_index < completed_index,
        "preview.updated must be emitted before run.completed: {event_types:?}"
    );
}

struct TerminalFailTool;

#[async_trait]
impl Tool for TerminalFailTool {
    fn name(&self) -> &'static str {
        "terminal.fail"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "test allow".to_string(),
            },
        }
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Terminal(
            "sandbox channel disconnected".to_string(),
        ))
    }
}

struct RecoverableFsWriteTool;

#[async_trait]
impl Tool for RecoverableFsWriteTool {
    fn name(&self) -> &'static str {
        "fs.write"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "test allow".to_string(),
            },
        }
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Recoverable(
            "bootstrap write denied by test".to_string(),
        ))
    }
}

struct InterruptCancelTool;

#[async_trait]
impl Tool for InterruptCancelTool {
    fn name(&self) -> &'static str {
        "interrupt.cancel"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: input.clone(),
            reason: PermissionReason::Other {
                reason: "test allow".to_string(),
            },
        }
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "shouldNotRun": true })))
    }
}

#[derive(Debug, Clone)]
struct RecordingModelClient {
    responses: Arc<Mutex<VecDeque<ModelResponse>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModelClient {
    fn new(
        responses: Vec<ModelResponse>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    ) -> RecordingModelClient {
        RecordingModelClient {
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            requests,
        }
    }
}

#[async_trait]
impl ModelClient for RecordingModelClient {
    async fn next_response(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        self.requests.lock().await.push(request);
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("recording model response queue exhausted"))
    }
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
