use anydesign_runtime::{
    http_api::{self, AppState},
    model_gateway::{MockModelClient, ModelResponse, ToolCall},
    preview::{promote_preview, PromotionGateReport},
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, Brief, ContentSource, SandboxBindingStatus,
        SandboxChannelProtocol,
    },
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use chrono::Utc;
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tower::ServiceExt;

fn mock_bff_app(store: RuntimeStore, responses: Vec<ModelResponse>) -> Router {
    http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(responses)),
    })
}

async fn post_json(app: Router, uri: impl AsRef<str>, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri.as_ref())
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    let value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, value)
}

async fn get_json(app: Router, uri: impl AsRef<str>) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri.as_ref())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn get_sse(app: Router, uri: impl AsRef<str>, last_event_id: Option<String>) -> String {
    let mut builder = Request::builder().uri(uri.as_ref());
    if let Some(last_event_id) = last_event_id {
        builder = builder.header("last-event-id", last_event_id);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), 32 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn get_sse_frames(
    app: Router,
    uri: impl AsRef<str>,
    last_event_id: Option<String>,
    frame_count: usize,
) -> String {
    let mut builder = Request::builder().uri(uri.as_ref());
    if let Some(last_event_id) = last_event_id {
        builder = builder.header("last-event-id", last_event_id);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut frames = String::new();
    for _ in 0..frame_count {
        let frame = timeout(Duration::from_secs(1), body.next())
            .await
            .expect("expected SSE frame")
            .expect("expected body frame")
            .expect("SSE frame should be valid");
        frames.push_str(&String::from_utf8(frame.to_vec()).unwrap());
    }
    frames
}

async fn start_run(
    app: Router,
    project_id: &str,
    phase: &str,
    agent_profile: &str,
    input_context: Value,
) -> String {
    let (status, payload) = post_json(
        app,
        "/runs",
        json!({
            "projectId": project_id,
            "phase": phase,
            "agentProfile": agent_profile,
            "inputContext": input_context,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "queued");
    payload["runId"].as_str().unwrap().to_string()
}

async fn create_workspace_binding(store: &RuntimeStore, project_id: &str, suffix: &str) -> String {
    let binding = store
        .create_sandbox_binding(
            project_id,
            format!("sandbox-{suffix}"),
            format!("sandbox-claim-{suffix}"),
            format!("workspace-sandbox-claim-{suffix}"),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap()
        .id;
    store
        .update_sandbox_binding_status(&binding, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    binding
}

async fn create_confirmed_brief(store: &RuntimeStore, project_id: &str) -> String {
    let brief_run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "Internal designers".to_string(),
                content_hierarchy: vec!["Hero".to_string(), "Proof".to_string()],
                page_structure: json!([
                    {
                        "title": "Home",
                        "purpose": "Present the product",
                        "keyContent": ["Hero", "CTA"]
                    }
                ]),
                visual_direction: "Clean editorial website".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap()
}

async fn wait_for_terminal(store: &RuntimeStore, run_id: &str) {
    for _ in 0..50 {
        if store
            .get_run(run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            return;
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn brief_run_streams_and_reconnects_without_duplicate_events() {
    let store = RuntimeStore::new();
    let app = mock_bff_app(
        store.clone(),
        vec![ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-1",
            "run.complete",
            json!({ "status": "completed", "summary": "Brief ready" }),
        )])],
    );

    let run_id = start_run(
        app.clone(),
        "project-1",
        "brief",
        "brief",
        json!({
            "contentSources": [
                ContentSource::readable("source-1", "prompt", "Make a confident product website")
            ]
        }),
    )
    .await;

    wait_for_terminal(&store, &run_id).await;
    let first_stream = get_sse(app.clone(), format!("/runs/{run_id}/events"), None).await;
    assert!(first_stream.contains(&format!("id: {run_id}/1")));
    assert!(first_stream.contains("run.started"));
    assert!(first_stream.contains("agent.message"));
    assert!(first_stream.contains("run.completed"));

    let replay = get_sse(
        app,
        format!("/runs/{run_id}/events"),
        Some(format!("{run_id}/1")),
    )
    .await;
    assert!(!replay.contains("run.started"));
    assert!(replay.contains("run.completed"));
}

#[tokio::test]
async fn build_run_exposes_promoted_preview_current_contract() {
    let store = RuntimeStore::new();
    let sandbox_binding_id = create_workspace_binding(&store, "project-1", "mock-bff-build").await;
    let brief_id = create_confirmed_brief(&store, "project-1").await;
    let app = mock_bff_app(
        store.clone(),
        vec![
            ModelResponse::TextOnly("Building Astro project".to_string()),
            ModelResponse::TextOnly("Still building".to_string()),
            ModelResponse::TextOnly("Checking preview".to_string()),
        ],
    );
    let run_id = start_run(
        app.clone(),
        "project-1",
        "build",
        "build",
        json!({ "sandboxBindingId": sandbox_binding_id, "briefId": brief_id }),
    )
    .await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run_id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/version-1.tar".to_string()),
        )
        .await;

    promote_preview(
        &store,
        "project-1",
        &run_id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();

    let events = get_sse(app.clone(), format!("/runs/{run_id}/events"), None).await;
    assert!(events.contains("preview.updated"));
    assert!(events.contains(&candidate.id));

    let (status, payload) = get_json(app.clone(), "/preview/project-1/current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["versionId"], candidate.id);
    assert_eq!(
        payload["previewUrl"],
        "http://preview.local/preview/project-1/current"
    );
    assert_eq!(payload["status"], "promoted");

    let (status, pinned) = get_json(
        app,
        format!(
            "/preview/project-1/{}",
            payload["versionId"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pinned["projectId"], "project-1");
    assert_eq!(pinned["versionId"], candidate.id);
    assert_eq!(pinned["previewUrl"], payload["previewUrl"]);
    assert_eq!(pinned["status"], "promoted");
}

#[tokio::test]
async fn product_bff_cannot_promote_preview_over_public_runtime_contract() {
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
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;
    let app = mock_bff_app(store.clone(), vec![]);

    let (status, payload) = post_json(
        app.clone(),
        "/internal/previews/promote",
        json!({
            "projectId": "project-1",
            "runId": run.id,
            "candidateVersionId": candidate.id,
            "gateReport": {
                "previewAccessible": true,
                "screenshotAvailable": true
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        payload["error"],
        "internal preview promotion endpoint is disabled"
    );
    assert!(store.current_project_version("project-1").await.is_none());
    assert!(store
        .audit_records()
        .await
        .iter()
        .all(|record| record.tool != "internal.previews.promote"));
}

#[tokio::test]
async fn continue_edit_promotes_new_version_without_changing_preview_url_shape() {
    let store = RuntimeStore::new();
    let build_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let initial = store
        .create_project_version_candidate(
            "project-1",
            &build_run.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-initial".to_string()),
            None,
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &build_run.id,
        &initial.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();

    let edit_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
            None,
            Some(initial.id.clone()),
        )
        .await;
    store
        .update_run_status(&edit_run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = mock_bff_app(
        store.clone(),
        vec![
            ModelResponse::TextOnly("Applying focused edit".to_string()),
            ModelResponse::TextOnly("Rebuilding edited project".to_string()),
            ModelResponse::TextOnly("Waiting for promotion".to_string()),
        ],
    );

    let (status, payload) = post_json(
        app.clone(),
        format!("/runs/{}/continue", edit_run.id),
        json!({ "userMessage": "Make the hero headline sharper" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["runId"], edit_run.id);
    assert_eq!(payload["status"], "running");
    assert_eq!(
        store
            .get_run(&edit_run.id)
            .await
            .unwrap()
            .base_version_id
            .as_deref(),
        Some(initial.id.as_str())
    );

    let edited = store
        .create_project_version_candidate(
            "project-1",
            &edit_run.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-edited".to_string()),
            Some("file:///workspace/snapshots/version-2.tar".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &edit_run.id,
        &edited.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();

    let (status, current) = get_json(app.clone(), "/preview/project-1/current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["versionId"], edited.id);
    assert_eq!(
        current["previewUrl"],
        "http://preview.local/preview/project-1/current"
    );
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation
        .iter()
        .any(|item| item.run_id.as_deref() == Some(&edit_run.id)
            && item.text == "Make the hero headline sharper"));
}

#[tokio::test]
async fn project_conversation_endpoint_exposes_runtime_conversation_items() {
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
    store
        .append_conversation_item(
            "project-1",
            Some(&run.id),
            "approval_request",
            Some("assistant"),
            "Please confirm this Brief before generation.",
            Some(json!({ "briefId": "brief-1" })),
        )
        .await;
    let app = mock_bff_app(store, vec![]);

    let (status, payload) = get_json(app, "/projects/project-1/conversation").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["items"][0]["runId"], run.id);
    assert_eq!(payload["items"][0]["kind"], "approval_request");
    assert_eq!(
        payload["items"][0]["text"],
        "Please confirm this Brief before generation."
    );
}

#[tokio::test]
async fn permission_decision_resumes_the_same_run() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "package.install")
        .await;
    let app = mock_bff_app(
        store.clone(),
        vec![ModelResponse::TextOnly(
            "Continuing after approval".to_string(),
        )],
    );

    let (status, payload) = post_json(
        app.clone(),
        format!("/permissions/{}/decision", permission.id),
        json!({
            "decision": "allow",
            "updatedInput": { "package": "@internal/design-system" }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "running");
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "allow"
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.run_id == run.id
            && record.tool == "package.install"
            && record.decision == "allow"));

    let events = get_sse_frames(app, format!("/runs/{}/events", run.id), None, 1).await;
    assert!(events.contains("state.changed"));
    assert!(events.contains("running"));
}

#[tokio::test]
async fn cancel_run_is_terminal_and_replayable() {
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
    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "Started build".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let app = mock_bff_app(store.clone(), vec![]);

    let (status, payload) =
        post_json(app.clone(), format!("/runs/{}/cancel", run.id), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "cancelled");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Cancelled
    );

    let replay = get_sse(
        app,
        format!("/runs/{}/events", run.id),
        Some(format!("{}/1", run.id)),
    )
    .await;
    assert!(!replay.contains("Started build"));
    assert!(replay.contains("run.completed"));
    assert!(replay.contains("cancelled"));
}
