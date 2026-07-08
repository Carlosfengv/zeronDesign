use anydesign_runtime::{
    agent_loop::AgentLoop,
    conversation::RuntimeStore,
    model_gateway::{
        MockModelClient, ModelClient, ModelRequest, ModelResponse, OpenAiCompatibleModelClient,
        ToolCall, ToolInputParseFailure, ToolInputTooLargeFailure,
    },
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
async fn brief_run_prompt_stays_on_content_sources_without_workspace_exploration() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "deepseek-chat".to_string(),
            vec![ContentSource::readable(
                "source-1",
                "prompt",
                "Create a styled Astro website",
            )],
        )
        .await;
    let captured_requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModelClient::new(
        vec![ModelResponse::Error(
            "stop after prompt assertion".to_string(),
        )],
        captured_requests.clone(),
    );
    let loop_runner = AgentLoop::new(store, Arc::new(model));

    loop_runner.run(&run.id).await.unwrap();

    let requests = captured_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "deepseek-chat");
    assert!(requests[0]
        .system_prompt
        .contains("provided content sources only"));
    assert!(requests[0]
        .system_prompt
        .contains("Do not inspect the filesystem or browser during Brief runs"));
    assert!(requests[0]
        .system_prompt
        .contains("astro-website for website projects"));
    assert!(requests[0]
        .system_prompt
        .contains("fumadocs-docs for docs projects"));
    assert!(requests[0].system_prompt.contains("brief.write_draft"));
    assert!(requests[0]
        .system_prompt
        .contains("brief.request_confirmation"));
}

#[tokio::test]
async fn build_run_bootstraps_confirmed_brief_into_workspace_before_model_turn() {
    let workspace = unique_temp_dir("agent-loop-bootstrap");
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
    let captured_requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModelClient::new(
        vec![ModelResponse::Error(
            "stop after bootstrap assertion".to_string(),
        )],
        captured_requests.clone(),
    );
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&build_run.id).await.unwrap();

    let requests = captured_requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .system_prompt
        .contains("runtime project workflow"));
    assert!(requests[0].system_prompt.contains("call project.init"));
    assert!(requests[0].system_prompt.contains("project.build"));
    assert!(requests[0]
        .system_prompt
        .contains("Do not call Brief tools"));
    assert!(requests[0]
        .system_prompt
        .contains("state/project.json appRoot"));
    assert!(requests[0].system_prompt.contains("Do not use npm create"));

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
async fn tool_input_parse_failure_emits_recoverable_matching_tool_result() {
    let workspace = unique_temp_dir("agent-loop-parse-health");
    fs::create_dir_all(workspace.join("state")).unwrap();
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
        ModelResponse::ToolInputParseFailed {
            parsed_calls: vec![],
            failures: vec![ToolInputParseFailure {
                tool_call_id: "tool-bad-json".to_string(),
                tool_name: "fs.write".to_string(),
                raw_len: 54,
                raw_sha256: "abc123".to_string(),
                ends_with_json_close: false,
                bracket_balance: 1,
                quote_closed: false,
                likely_truncated: true,
            }],
        },
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered after parse failure" }),
        )]),
    ]);
    let loop_runner = AgentLoop::with_tool_executor(
        store.clone(),
        Arc::new(model),
        control_plane_executor().with_workspace_root(&workspace),
    );

    let results = loop_runner.run(&run.id).await.unwrap();

    assert!(results.iter().any(|result| {
        result.tool_use_id == "tool-bad-json"
            && result.is_error
            && result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("errorKind").and_then(Value::as_str))
                == Some("tool.input_json_parse_failed")
    }));
    let events = store.events(&run.id).await;
    let failed_event = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolFailed {
                tool,
                tool_use_id,
                recoverable,
                metadata,
                ..
            } if tool == "fs.write" && tool_use_id == "tool-bad-json" => {
                Some((*recoverable, metadata.clone()))
            }
            _ => None,
        })
        .expect("parse failure should emit tool.failed");
    assert!(failed_event.0);
    assert_eq!(
        failed_event
            .1
            .as_ref()
            .and_then(|metadata| metadata.get("errorKind"))
            .and_then(Value::as_str),
        Some("tool.input_json_parse_failed")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "tool_input_json_parse_failed"
            && metadata.get("tool").and_then(Value::as_str) == Some("fs.write")
            && metadata.get("toolUseId").and_then(Value::as_str) == Some("tool-bad-json")
    )));
    let audit_records = store.audit_records().await;
    let audit_record = audit_records
        .iter()
        .find(|record| {
            record.tool == "fs.write"
                && record.input_summary.contains("toolUseId=tool-bad-json")
                && record.reason.contains("tool.input_json_parse_failed")
        })
        .expect("parse failure should be captured in audit summary");
    assert_eq!(audit_record.decision, "deny");
    assert!(audit_record.input_summary.contains("rawLen=54"));
    assert!(audit_record.input_summary.contains("rawSha256=abc123"));
    let serialized_events = serde_json::to_string(&events).unwrap();
    assert!(!serialized_events.contains("rawArguments"));
    assert!(!serialized_events.contains("<html"));
    let serialized_audit = serde_json::to_string(&audit_records).unwrap();
    assert!(!serialized_audit.contains("rawArguments"));
    assert!(!serialized_audit.contains("<html"));
    let health: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/run-health.json")).unwrap())
            .unwrap();
    assert!(health["toolInputFailures"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |failure| failure["errorKind"] == "tool.input_json_parse_failed"
                && failure["tool"] == "fs.write"
                && failure["toolUseId"] == "tool-bad-json"
                && failure["rawLen"] == 54
                && failure["rawSha256"] == "abc123"
        ));
    let serialized_health = serde_json::to_string(&health).unwrap();
    assert!(!serialized_health.contains("rawArguments"));
    assert!(!serialized_health.contains("<html"));
}

#[tokio::test]
async fn streaming_tool_input_too_large_emits_recoverable_matching_tool_result() {
    let workspace = unique_temp_dir("agent-loop-too-large-health");
    fs::create_dir_all(workspace.join("state")).unwrap();
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
        ModelResponse::ToolInputTooLarge {
            parsed_calls: vec![],
            failures: vec![ToolInputTooLargeFailure {
                tool_call_id: "tool-large-json".to_string(),
                tool_name: "fs.write".to_string(),
                input_chars: 96_001,
                max_input_chars: 96_000,
                raw_sha256: "abc123".to_string(),
            }],
        },
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered after input too large" }),
        )]),
    ]);
    let loop_runner = AgentLoop::with_tool_executor(
        store.clone(),
        Arc::new(model),
        control_plane_executor().with_workspace_root(&workspace),
    );

    let results = loop_runner.run(&run.id).await.unwrap();

    assert!(results.iter().any(|result| {
        result.tool_use_id == "tool-large-json"
            && result.is_error
            && result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("errorKind").and_then(Value::as_str))
                == Some("tool.input_too_large")
    }));
    let events = store.events(&run.id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolFailed {
            tool,
            tool_use_id,
            recoverable: true,
            metadata: Some(metadata),
            ..
        } if tool == "fs.write"
            && tool_use_id == "tool-large-json"
            && metadata.get("errorKind").and_then(Value::as_str)
                == Some("tool.input_too_large")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "tool_input_too_large"
            && metadata.get("tool").and_then(Value::as_str) == Some("fs.write")
            && metadata.get("toolUseId").and_then(Value::as_str) == Some("tool-large-json")
    )));
    let audit_records = store.audit_records().await;
    let audit_record = audit_records
        .iter()
        .find(|record| {
            record.tool == "fs.write"
                && record.input_summary.contains("toolUseId=tool-large-json")
                && record.reason.contains("tool.input_too_large")
        })
        .expect("too-large failure should be captured in audit summary");
    assert_eq!(audit_record.decision, "deny");
    assert!(audit_record.input_summary.contains("inputChars=96001"));
    assert!(audit_record.input_summary.contains("maxInputChars=96000"));
    assert!(audit_record.input_summary.contains("rawSha256=abc123"));
    let serialized_events = serde_json::to_string(&events).unwrap();
    assert!(!serialized_events.contains("rawArguments"));
    let serialized_audit = serde_json::to_string(&audit_records).unwrap();
    assert!(!serialized_audit.contains("rawArguments"));
    let health: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/run-health.json")).unwrap())
            .unwrap();
    assert!(health["toolInputFailures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|failure| failure["errorKind"] == "tool.input_too_large"
            && failure["tool"] == "fs.write"
            && failure["toolUseId"] == "tool-large-json"
            && failure["inputChars"] == 96_001
            && failure["maxInputChars"] == 96_000
            && failure["rawSha256"] == "abc123"));
    let serialized_health = serde_json::to_string(&health).unwrap();
    assert!(!serialized_health.contains("rawArguments"));
}

#[tokio::test]
async fn tool_input_schema_invalid_failure_records_safe_run_health() {
    let workspace = unique_temp_dir("agent-loop-schema-health");
    fs::create_dir_all(workspace.join("state")).unwrap();
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
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-schema-invalid",
            "fs.write",
            json!({ "arguments": "not real fs.write input" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered after schema failure" }),
        )]),
    ]);
    let executor = ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        PermissionRules::default(),
        &workspace,
    );
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&run.id).await.unwrap();

    let events = store.events(&run.id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolFailed {
            tool,
            tool_use_id,
            recoverable: true,
            metadata: Some(metadata),
            ..
        } if tool == "fs.write"
            && tool_use_id == "tool-schema-invalid"
            && metadata.get("errorKind").and_then(Value::as_str)
                == Some("tool.input_schema_invalid")
    )));
    let health: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/run-health.json")).unwrap())
            .unwrap();
    assert!(health["toolInputFailures"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |failure| failure["errorKind"] == "tool.input_schema_invalid"
                && failure["tool"] == "fs.write"
                && failure["toolUseId"] == "tool-schema-invalid"
        ));
    let serialized_health = serde_json::to_string(&health).unwrap();
    assert!(!serialized_health.contains("not real fs.write input"));
    assert!(!serialized_health.contains("rawArguments"));
}

#[tokio::test]
async fn repeated_recoverable_large_write_failure_stops_run_as_partial() {
    let workspace = unique_temp_dir("agent-loop-large-write-guard");
    fs::create_dir_all(workspace.join("project")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
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
    let large_text = "x".repeat(48_001);
    let repeated_call = |id: &str| {
        ToolCall::new(
            id,
            "fs.write",
            json!({ "path": "project/src/pages/index.astro", "text": large_text.clone() }),
        )
    };
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![repeated_call("tool-large-1")]),
        ModelResponse::ToolCalls(vec![repeated_call("tool-large-2")]),
        ModelResponse::ToolCalls(vec![repeated_call("tool-large-3")]),
        ModelResponse::Error("guard should stop before this response".to_string()),
    ]);
    let executor = ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        PermissionRules::default(),
        &workspace,
    );
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    let results = loop_runner.run(&run.id).await.unwrap();

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|result| result.is_error));
    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Partial);
    let events = store.events(&run.id).await;
    let recovery_attempts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolRecoverySuggested {
                tool,
                error_kind,
                attempt,
                metadata,
                ..
            } => Some((tool, error_kind, *attempt, metadata)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recovery_attempts.len(), 2);
    assert_eq!(recovery_attempts[0].0, "fs.write");
    assert_eq!(recovery_attempts[0].1, "tool.input_too_large");
    assert_eq!(recovery_attempts[0].2, 2);
    assert_eq!(recovery_attempts[1].2, 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::MetricRecorded { name, .. }
                    if name == "tool_input_retry_same_large_write"
            ))
            .count(),
        2
    );
    assert_eq!(
        recovery_attempts[0]
            .3
            .as_ref()
            .and_then(|metadata| metadata.get("normalizedPath"))
            .and_then(Value::as_str),
        Some("project/src/pages/index.astro")
    );
    let partial_summary = events
        .iter()
        .find_map(|event| {
            let value = serde_json::to_value(event).unwrap();
            if value["type"] == "run.completed" && value["status"] == "partial" {
                value["summary"].as_str().map(str::to_string)
            } else {
                None
            }
        })
        .expect("partial run.completed event should include a summary");
    assert!(partial_summary.contains("已停止自动重试"));
    assert!(partial_summary.contains("fs.patch"));
    assert!(partial_summary.contains("fs.write_chunk"));
    assert!(partial_summary.contains("partial"));
    let health: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/run-health.json")).unwrap())
            .unwrap();
    assert_eq!(
        health["toolInputFailures"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|failure| failure["errorKind"] == "tool.input_too_large"
                && failure["tool"] == "fs.write"
                && failure["path"] == "project/src/pages/index.astro")
            .count(),
        3
    );
}

#[tokio::test]
async fn failed_run_cleans_its_staged_chunk_sessions() {
    let workspace = unique_temp_dir("agent-loop-chunk-cleanup");
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
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-chunk",
            "fs.write_chunk",
            json!({
                "path": "project/chunked.md",
                "sessionId": "cleanup-test",
                "index": 0,
                "total": 1,
                "text": "not committed\n",
            }),
        )]),
        ModelResponse::Error("stop after staged chunk".to_string()),
    ]);
    let executor = ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        PermissionRules::default(),
        &workspace,
    );
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    loop_runner.run(&run.id).await.unwrap();

    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Failed);
    assert!(!workspace
        .join("outputs/staged-writes/cleanup-test")
        .exists());
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
    for index in 0..40 {
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
                    "scripts": { "build": format!("node -e {:?}", build_script) }
                })).unwrap()
            }),
        ),
        ToolCall::new(
            "tool-index",
            "fs.write",
            json!({ "path": "project/src/pages/index.astro", "text": "<h1>Design runtime</h1>" }),
        ),
        ToolCall::new("tool-build", "project.build", json!({ "cwd": "project" })),
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

#[tokio::test]
async fn chunked_large_page_build_run_promotes_preview_before_completion() {
    let workspace = unique_temp_dir("agent-loop-chunked-build");
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
const source=fs.readFileSync('src/pages/index.astro','utf8');\
fs.mkdirSync('../outputs/build',{recursive:true});\
fs.mkdirSync('dist',{recursive:true});\
fs.writeFileSync('../outputs/build/build.log','Build ok\\n');\
fs.writeFileSync('dist/index.html','<!doctype html><title>chunked</title>'+source);";
    let large_page = format!(
        "<main><h1>Chunked SaaS Page</h1>{}</main>",
        "<section><h2>Runtime Reliability</h2><p>Chunked write keeps long generated pages out of a single tool-call JSON argument.</p></section>\n"
            .repeat(520)
    );
    assert!(large_page.chars().count() > 48_000);
    let chunks = large_page
        .as_bytes()
        .chunks(16_000)
        .map(|chunk| String::from_utf8(chunk.to_vec()).unwrap())
        .collect::<Vec<_>>();
    assert!(chunks.len() > 1);
    let mut calls = vec![
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
                    "scripts": { "build": format!("node -e {:?}", build_script) }
                })).unwrap()
            }),
        ),
    ];
    for (index, chunk) in chunks.iter().enumerate() {
        calls.push(ToolCall::new(
            format!("tool-chunk-{index}"),
            "fs.write_chunk",
            json!({
                "path": "project/src/pages/index.astro",
                "sessionId": "chunked-large-page",
                "index": index,
                "total": chunks.len(),
                "text": chunk,
            }),
        ));
    }
    calls.extend([
        ToolCall::new(
            "tool-commit",
            "fs.commit_chunks",
            json!({
                "path": "project/src/pages/index.astro",
                "sessionId": "chunked-large-page",
                "total": chunks.len(),
            }),
        ),
        ToolCall::new("tool-build", "project.build", json!({ "cwd": "project" })),
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
            json!({ "screenshotId": "shot-chunked-build", "blank": false }),
        ),
        ToolCall::new(
            "tool-candidate",
            "preview.report_candidate",
            json!({
                "url": preview_url,
                "screenshotId": "shot-chunked-build",
                "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshot.txt"
            }),
        ),
        ToolCall::new(
            "tool-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Chunked Astro preview promoted" }),
        ),
    ]);
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(calls)]);
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(model), executor);

    let results = loop_runner.run(&run.id).await.unwrap();

    assert!(results.iter().all(|result| !result.is_error));
    let run = store.get_run(&run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert!(run.output_version_id.is_some());
    assert_eq!(
        fs::read_to_string(workspace.join("project/src/pages/index.astro")).unwrap(),
        large_page
    );
    assert!(workspace.join("project/dist/index.html").exists());
    let health: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/run-health.json")).unwrap())
            .unwrap();
    assert!(health["chunkWrites"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |write| write["path"] == "/workspace/project/src/pages/index.astro"
                && write["status"] == "committed"
                && write["total"] == chunks.len()
        ));

    let events = store.events(&run.id).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ChunkReceived { .. }))
            .count(),
        chunks.len()
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ChunkCommitted {
            path,
            session_id,
            ..
        } if path == "/workspace/project/src/pages/index.astro"
            && session_id == "chunked-large-page"
    )));
    let event_types = events
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

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY and network access"]
async fn real_deepseek_design_md_website_generation_e2e() {
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set for this test");
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model_name = std::env::var("DEEPSEEK_E2E_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let workspace = unique_temp_dir("real-deepseek-website-e2e");
    prepare_minimal_buildable_project(&workspace);

    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let brief_run = store
        .create_run(
            "real-deepseek-website".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            model_name.clone(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "harness engineers evaluating runtime reliability".to_string(),
                content_hierarchy: vec![
                    "Hero".to_string(),
                    "Runtime protections".to_string(),
                    "Observable generation".to_string(),
                ],
                page_structure: json!([
                    {
                        "title": "Home",
                        "purpose": "Explain the website/docs generation harness",
                        "keyContent": ["hero", "tool call recovery", "preview promotion"]
                    }
                ]),
                visual_direction: "SaaS style, calm technical confidence, polished spacing"
                    .to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![
                    "Workspace already contains a buildable minimal project; do not install packages."
                        .to_string(),
                ],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    let build_run = store
        .create_run_with_context(
            "real-deepseek-website".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            model_name.clone(),
            vec![
                ContentSource::readable(
                    "prompt",
                    "prompt",
                    "Generate a concise SaaS website for the runtime harness. Use project.write_page for route /. Then run project.build, preview.start, browser.open, browser.screenshot, preview.report_candidate, and run.complete. Do not use package.install and do not rewrite package.json.",
                ),
                ContentSource::readable(
                    "design-md",
                    "design_md",
                    "# Design requirements\n- SaaS dashboard/operations style\n- Clear hero, proof points, and operational metrics\n- Mention JSON tool-call recovery, chunked writes, and preview promotion\n- Keep the page compact enough for the direct tool budget; prefer project.write_page\n",
                ),
            ],
            Some(brief_id),
            None,
        )
        .await;
    let client = OpenAiCompatibleModelClient::new(base_url, api_key, Some("deepseek"))
        .with_streaming(env_flag("MODEL_STREAMING"))
        .with_strict_tools(env_flag("MODEL_STRICT_TOOLS"));
    let executor = control_plane_executor().with_workspace_root(&workspace);
    let loop_runner = AgentLoop::with_tool_executor(store.clone(), Arc::new(client), executor);

    let results = loop_runner.run(&build_run.id).await.unwrap();
    let run = store.get_run(&build_run.id).await.unwrap();
    let events = store.events(&build_run.id).await;
    let serialized_events = serde_json::to_string(&events).unwrap();

    assert!(
        matches!(
            run.status,
            AgentRunStatus::Completed | AgentRunStatus::Partial
        ),
        "real DeepSeek website E2E should end completed or partial, got {:?}; results={results:?}; events={serialized_events}",
        run.status
    );
    assert!(
        !serialized_events.contains("fs.write requires path"),
        "run-167 regression: stream must not misclassify malformed/oversized tool input as fs.write requires path"
    );
    assert!(
        run.status == AgentRunStatus::Partial
            || events
                .iter()
                .any(|event| { serde_json::to_value(event).unwrap()["type"] == "preview.updated" }),
        "completed real DeepSeek website E2E should promote a preview; events={serialized_events}"
    );
    if run.status == AgentRunStatus::Partial {
        assert!(
            events.iter().any(|event| {
                let value = serde_json::to_value(event).unwrap();
                value["type"] == "tool.failed"
                    && value
                        .get("metadata")
                        .and_then(|metadata| metadata.get("errorKind"))
                        .is_some()
            }) || events.iter().any(|event| {
                serde_json::to_value(event).unwrap()["type"] == "tool.recovery_suggested"
            }),
            "partial real DeepSeek E2E should include actionable typed recovery evidence"
        );
    }
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

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn prepare_minimal_buildable_project(workspace: &std::path::Path) {
    fs::create_dir_all(workspace.join("project/scripts")).unwrap();
    fs::create_dir_all(workspace.join("project/src/pages")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::create_dir_all(workspace.join("outputs")).unwrap();
    fs::write(
        workspace.join("state/project.json"),
        json!({
            "projectId": "real-deepseek-website",
            "template": "astro-website",
            "appRoot": "project"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "type": "module",
            "scripts": {
                "build": "node scripts/build.mjs"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        workspace.join("project/scripts/build.mjs"),
        r#"import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
const source = readFileSync('src/pages/index.astro', 'utf8');
const body = source.replace(/^---[\s\S]*?---\s*/, '');
mkdirSync('../outputs/build', { recursive: true });
mkdirSync('dist', { recursive: true });
writeFileSync('../outputs/build/build.log', 'Build ok\n');
writeFileSync('dist/index.html', `<!doctype html><html><head><title>Runtime Harness</title></head><body>${body}</body></html>`);
"#,
    )
    .unwrap();
    fs::write(
        workspace.join("project/src/pages/index.astro"),
        "<main><h1>Runtime Harness Placeholder</h1></main>\n",
    )
    .unwrap();
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
