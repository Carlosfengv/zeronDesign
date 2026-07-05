use anydesign_runtime::{
    http_api::{self, AppState},
    model_gateway::{MockModelClient, ModelResponse},
    preview::{promote_preview, PromotionGateReport},
    types::{AgentPhase, AgentRunStatus, Brief},
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn app(store: RuntimeStore, responses: Vec<ModelResponse>) -> Router {
    http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(responses)),
    })
}

fn website_brief() -> Brief {
    Brief {
        project_type: "website".to_string(),
        audience: "enterprise product designers".to_string(),
        content_hierarchy: vec!["hero".to_string(), "proof".to_string()],
        page_structure: json!([
            {
                "title": "Home",
                "purpose": "Explain the product",
                "keyContent": ["hero", "features"]
            }
        ]),
        visual_direction: "quiet technical confidence".to_string(),
        recommended_template: "astro-website".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
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
    (status, serde_json::from_slice(&body).unwrap())
}

async fn seed_brief_and_promoted_version(store: &RuntimeStore) -> (String, String) {
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
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
            Some("file:///workspace/snapshots/initial.tar".to_string()),
        )
        .await;
    promote_preview(
        store,
        "project-1",
        &build_run.id,
        &initial.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    (brief_id, initial.id)
}

#[tokio::test]
async fn edit_run_modifies_existing_project_version_chain() {
    let store = RuntimeStore::new();
    let (brief_id, base_version_id) = seed_brief_and_promoted_version(&store).await;
    let edit_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            Some(base_version_id.clone()),
        )
        .await;
    store
        .update_run_status(&edit_run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = app(
        store.clone(),
        vec![
            ModelResponse::TextOnly("Applying focused edit".to_string()),
            ModelResponse::TextOnly("Rebuilding preview".to_string()),
            ModelResponse::TextOnly("Waiting for promotion".to_string()),
        ],
    );

    let (status, payload) = post_json(
        app,
        format!("/runs/{}/continue", edit_run.id),
        json!({ "userMessage": "Tighten the hero copy and keep the same structure" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["runId"], edit_run.id);
    assert_eq!(payload["status"], "running");
    let run = store.get_run(&edit_run.id).await.unwrap();
    assert_eq!(run.project_id, "project-1");
    assert_eq!(
        run.base_version_id.as_deref(),
        Some(base_version_id.as_str())
    );
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.run_id.as_deref() == Some(&edit_run.id)
            && item.text.contains("Tighten the hero")));

    let edited = store
        .create_project_version_candidate(
            "project-1",
            &edit_run.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-edited".to_string()),
            Some("file:///workspace/snapshots/edited.tar".to_string()),
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
    let current = store.current_project_version("project-1").await.unwrap();
    assert_eq!(current.id, edited.id);
    assert_eq!(
        current.preview_url,
        "http://preview.local/preview/project-1/current"
    );
}

#[tokio::test]
async fn brief_conflict_pauses_edit_run_until_user_confirms_direction_change() {
    let store = RuntimeStore::new();
    let (brief_id, base_version_id) = seed_brief_and_promoted_version(&store).await;
    let edit_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            Some(base_version_id),
        )
        .await;
    store
        .update_run_status(&edit_run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = app(store.clone(), vec![]);

    let (status, payload) = post_json(
        app,
        format!("/runs/{}/continue", edit_run.id),
        json!({ "userMessage": "Turn this into a fumadocs-docs documentation portal" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["runId"], edit_run.id);
    assert_eq!(payload["status"], "needs_user_input");
    assert_eq!(
        store.get_run(&edit_run.id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation
        .iter()
        .any(|item| item.kind == "user_message" && item.text.contains("documentation portal")));
    assert!(conversation
        .iter()
        .any(|item| item.kind == "approval_request" && item.text.contains("confirmed Brief")));
    assert!(store.events(&edit_run.id).await.iter().any(|event| {
        let event = serde_json::to_value(event).unwrap();
        event["type"] == "state.changed" && event["state"] == "needs_user_input:brief_conflict"
    }));
}
