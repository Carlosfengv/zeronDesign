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
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use chrono::Utc;
use futures::StreamExt;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::time::{timeout, Duration};
use tower::ServiceExt;

fn mock_bff_app(store: RuntimeStore, responses: Vec<ModelResponse>) -> Router {
    http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(responses)),
    })
}

#[derive(Clone)]
struct MockBffProxyState {
    runtime_app: Router,
}

fn mock_bff_proxy_app(runtime_app: Router) -> Router {
    Router::new()
        .route("/bff/runs/{run_id}/events", get(proxy_run_events))
        .with_state(MockBffProxyState { runtime_app })
}

async fn proxy_run_events(
    State(state): State<MockBffProxyState>,
    Path(run_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let mut builder = Request::builder().uri(format!("/runs/{run_id}/events"));
    if let Some(last_event_id) = query.get("lastEventId") {
        builder = builder.header("last-event-id", last_event_id);
    }
    state
        .runtime_app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
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

async fn put_json(app: Router, uri: impl AsRef<str>, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
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

fn design_profile_payload(scope: Value) -> Value {
    json!({
        "name": "Harness Calm Ops",
        "profile": {
            "status": "active",
            "scope": scope,
            "source": { "kind": "manual" },
            "product": {
                "name": "AnyDesign Runtime",
                "category": "agent harness",
                "audience": ["internal builders"],
                "primaryUseCases": ["generate websites", "edit docs"],
                "productQualities": ["reliable", "inspectable"]
            },
            "brand": {
                "voice": {
                    "tone": ["clear", "precise"],
                    "sentenceStyle": "technical",
                    "vocabulary": { "prefer": ["runtime", "evidence"], "avoid": ["magic"] },
                    "writingRules": ["Use concrete status text."]
                },
                "messaging": {
                    "headlineStyle": "specific",
                    "bodyStyle": "concise",
                    "ctaStyle": "verb first",
                    "proofStyle": "evidence based",
                    "forbiddenClaims": ["guaranteed"]
                }
            },
            "visual": {
                "direction": "quiet operational interface",
                "principles": ["scan friendly"],
                "moodKeywords": ["calm"],
                "avoidKeywords": ["flashy"],
                "composition": {},
                "imagery": {},
                "motion": {}
            },
            "tokens": {
                "color": {},
                "typography": {},
                "radius": {},
                "shadow": {},
                "spacing": {}
            },
            "runtimeTokenMapping": {
                "color.background": "#ffffff",
                "color.surface": "#f8fafc",
                "color.surfaceStrong": "#e2e8f0",
                "color.text": "#0f172a",
                "color.muted": "#475569",
                "color.primary": "#2563eb",
                "color.primaryContrast": "#ffffff",
                "color.border": "#cbd5e1",
                "radius.card": "8px",
                "radius.control": "6px",
                "font.sans": "Inter, sans-serif",
                "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)"
            },
            "components": {
                "primitives": {
                    "button": { "intent": "clear action", "usage": ["primary actions"], "avoid": ["overuse"] },
                    "input": { "intent": "precise entry", "usage": ["forms"], "avoid": ["placeholder-only labels"] },
                    "card": { "intent": "group repeated items", "usage": ["lists"], "avoid": ["nested cards"] },
                    "badge": { "intent": "show status", "usage": ["statuses"], "avoid": ["decorative noise"] }
                }
            },
            "content": {},
            "accessibility": {},
            "technical": {
                "allowedTemplates": ["astro-website", "fumadocs-docs"],
                "preferredTemplates": { "website": "astro-website", "docs": "fumadocs-docs" },
                "cssStrategy": "runtime-style-contract",
                "dependencyPolicy": {},
                "filePolicy": {
                    "designProfilePath": "/workspace/inputs/design-profile.json",
                    "designMarkdownPath": "/workspace/inputs/design.md",
                    "styleContractPath": "/workspace/state/style-contract.json"
                }
            },
            "governance": { "conflictBehavior": "ask" }
        }
    })
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
async fn bff_events_route_proxies_runtime_live_sse_with_last_event_id_query() {
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
    for text in ["first", "second"] {
        store
            .append_event(AgentEvent::AgentMessage {
                run_id: run.id.clone(),
                text: text.to_string(),
                timestamp: Utc::now(),
            })
            .await
            .unwrap();
    }
    let runtime_app = mock_bff_app(store.clone(), vec![]);
    let bff_app = mock_bff_proxy_app(runtime_app);
    let response = bff_app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/bff/runs/{}/events?lastEventId={}%2F1",
                    run.id, run.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let replay = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected replay frame")
        .expect("expected replay body")
        .expect("replay frame should be valid");
    let replay = String::from_utf8(replay.to_vec()).unwrap();
    assert!(!replay.contains("first"));
    assert!(replay.contains("second"));
    assert!(replay.contains(&format!("id: {}/2", run.id)));

    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "third".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let live = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected live frame")
        .expect("expected live body")
        .expect("live frame should be valid");
    let live = String::from_utf8(live.to_vec()).unwrap();
    assert!(live.contains("third"));
    assert!(live.contains(&format!("id: {}/3", run.id)));

    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let terminal = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected terminal frame")
        .expect("expected terminal body")
        .expect("terminal frame should be valid");
    let terminal = String::from_utf8(terminal.to_vec()).unwrap();
    assert!(terminal.contains("run.completed"));
    assert!(terminal.contains(&format!("id: {}/4", run.id)));
}

#[tokio::test]
async fn mock_bff_can_manage_design_profile_context_over_runtime_contract() {
    let store = RuntimeStore::new();
    let app = mock_bff_app(store.clone(), vec![]);

    let (status, created) = post_json(
        app.clone(),
        "/design-profiles",
        design_profile_payload(json!({ "projectId": "project-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let design_profile_id = created["designProfile"]["id"].as_str().unwrap();
    assert_eq!(created["designProfile"]["version"], 1);

    let (status, listed) = get_json(app.clone(), "/design-profiles?projectId=project-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["designProfiles"].as_array().unwrap().len(), 1);

    let mut update = design_profile_payload(json!({ "projectId": "project-1" }));
    update["name"] = json!("Harness Calm Ops v2");
    let (status, updated) = put_json(
        app.clone(),
        format!("/design-profiles/{design_profile_id}"),
        update,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["designProfile"]["version"], 2);

    let (status, versions) = get_json(
        app.clone(),
        format!("/design-profiles/{design_profile_id}/versions"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);

    let (status, diff) = get_json(
        app.clone(),
        format!("/design-profiles/{design_profile_id}/diff?fromVersion=1&toVersion=2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["path"] == "name"));

    let (status, bound) = post_json(
        app.clone(),
        "/projects/project-1/design-profile",
        json!({ "designProfileId": design_profile_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bound["designProfile"]["id"], design_profile_id);

    let run_id = start_run(
        app.clone(),
        "project-1",
        "brief",
        "brief",
        json!({
            "contentSources": [
                ContentSource::readable("source-1", "prompt", "Make a website")
            ]
        }),
    )
    .await;
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.design_profile_id.as_deref(), Some(design_profile_id));
    assert_eq!(run.design_profile_version, Some(2));

    let (status, archived) = post_json(
        app.clone(),
        format!("/design-profiles/{design_profile_id}/archive"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["designProfile"]["status"], "archived");

    let (status, listed_active) =
        get_json(app.clone(), "/design-profiles?projectId=project-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed_active["designProfiles"].as_array().unwrap().len(), 0);
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
