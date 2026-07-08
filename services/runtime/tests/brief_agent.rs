use anydesign_runtime::{
    agent_loop::AgentLoop,
    conversation::RuntimeStore,
    model_gateway::{MockModelClient, ModelResponse, ToolCall},
    types::{AgentEvent, AgentPhase, AgentRunStatus, Brief, BriefStatus, ContentSource},
};
use serde_json::json;
use std::sync::Arc;

fn valid_brief() -> serde_json::Value {
    json!({
        "projectType": "website",
        "audience": "enterprise designers",
        "contentHierarchy": ["hero", "features"],
        "pageStructure": [
            {
                "title": "Home",
                "purpose": "Explain the product",
                "keyContent": ["hero", "proof"]
            }
        ],
        "visualDirection": "quiet technical confidence",
        "recommendedTemplate": "astro-website",
        "assumptions": [],
        "missingInformation": []
    })
}

#[tokio::test]
async fn prompt_and_markdown_produce_structured_brief_draft_and_wait_for_confirmation() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource::readable(
                "source-1",
                "markdown",
                "# Product\nBuild a website for designers.",
            )],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![
            ToolCall::new("tool-1", "content.list_sources", json!({})),
            ToolCall::new("tool-2", "content.read_source", json!({ "id": "source-1" })),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new("tool-3", "brief.write_draft", valid_brief()),
            ToolCall::new(
                "tool-4",
                "brief.request_confirmation",
                json!({ "message": "Please confirm this Brief before generation." }),
            ),
        ]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model.clone()));

    let results = loop_runner.run(&run.id).await.unwrap();

    model.assert_all_consumed().await;
    assert_eq!(results.len(), 4);
    let paused = store.get_run(&run.id).await.unwrap();
    assert_eq!(paused.status, AgentRunStatus::NeedsUserInput);
    let brief_id = paused.brief_version.expect("brief should be stored");
    let brief: Brief = store.get_brief(&brief_id).await.unwrap();
    assert_eq!(brief.project_type, "website");
    assert_eq!(brief.recommended_template, "astro-website");
    assert!(!brief.content_hierarchy.is_empty());
    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Draft)
    );
    if let Some(checkpoint_id) = paused.checkpoint_id.as_deref() {
        let checkpoint = store.get_checkpoint(checkpoint_id).await.unwrap();
        assert!(!checkpoint
            .message_window
            .iter()
            .any(|message| message["text"] == "Brief confirmed."));
    }
    assert!(store.events(&run.id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::AgentMessage { text, .. } if text.contains("confirm this Brief")
        )
    }));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation
        .iter()
        .any(|item| item.kind == "approval_request" && item.text.contains("confirm this Brief")));
}

#[tokio::test]
async fn brief_write_draft_normalizes_common_model_field_aliases() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource::readable(
                "source-1",
                "prompt",
                "Make a website",
            )],
        )
        .await;
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
        ToolCall::new(
            "tool-1",
            "brief.write_draft",
            json!({
                "project_type": "Website",
                "audience": "enterprise designers",
                "content_hierarchy": ["hero", "features"],
                "page_structure": [
                    {
                        "title": "Home",
                        "purpose": "Explain the product",
                        "keyContent": ["hero", "proof"]
                    }
                ],
                "visual_direction": "polished product launch",
                "template": "website",
                "assumptions": [],
                "missing_information": []
            }),
        ),
        ToolCall::new("tool-2", "brief.request_confirmation", json!({})),
    ])]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model.clone()));

    loop_runner.run(&run.id).await.unwrap();

    model.assert_all_consumed().await;
    let paused = store.get_run(&run.id).await.unwrap();
    let brief_id = paused.brief_version.expect("brief should be stored");
    let brief: Brief = store.get_brief(&brief_id).await.unwrap();
    assert_eq!(brief.project_type, "website");
    assert_eq!(brief.recommended_template, "astro-website");
    assert_eq!(brief.visual_direction, "polished product launch");
}

#[tokio::test]
async fn empty_input_pauses_with_needs_user_input() {
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
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-1",
        "user.ask",
        json!({ "message": "Please provide source content before I draft the brief." }),
    )])]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model.clone()));

    loop_runner.run(&run.id).await.unwrap();

    model.assert_all_consumed().await;
    let paused = store.get_run(&run.id).await.unwrap();
    assert_eq!(paused.status, AgentRunStatus::NeedsUserInput);
    let events = store.events(&run.id).await;
    assert!(events
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "state.changed"));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation
        .iter()
        .any(|item| item.text.contains("Please provide source content")));
}

#[tokio::test]
async fn unreadable_content_source_blocks_run_when_agent_completes_blocked() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource {
                id: "source-1".to_string(),
                kind: "attachment_text".to_string(),
                text: String::new(),
                readable: false,
            }],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-1",
            "content.read_source",
            json!({ "id": "source-1" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-2",
            "run.complete",
            json!({ "status": "blocked", "summary": "Attachment source-1 is unreadable" }),
        )]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model.clone()));

    let results = loop_runner.run(&run.id).await.unwrap();

    model.assert_all_consumed().await;
    assert!(results.iter().any(|result| result.is_error));
    let blocked = store.get_run(&run.id).await.unwrap();
    assert_eq!(blocked.status, AgentRunStatus::Blocked);
}

#[tokio::test]
async fn invalid_brief_json_is_recoverable_and_not_completed() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource::readable(
                "source-1",
                "prompt",
                "Make a website",
            )],
        )
        .await;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-1",
            "brief.write_draft",
            json!({
                "projectType": "website",
                "audience": "",
                "contentHierarchy": [],
                "pageStructure": [],
                "visualDirection": "",
                "recommendedTemplate": "astro-website",
                "assumptions": [],
                "missingInformation": []
            }),
        )]),
        ModelResponse::ToolCalls(vec![]),
        ModelResponse::ToolCalls(vec![]),
        ModelResponse::ToolCalls(vec![]),
    ]);
    let loop_runner = AgentLoop::new(store.clone(), Arc::new(model));

    let results = loop_runner.run(&run.id).await.unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    let partial = store.get_run(&run.id).await.unwrap();
    assert_eq!(partial.status, AgentRunStatus::Partial);
    assert!(partial.brief_version.is_none());
}
