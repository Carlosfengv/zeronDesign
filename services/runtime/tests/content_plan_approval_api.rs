use anydesign_runtime::{
    config::{PublicPrincipalAuthMode, RuntimePolicyProfile},
    http_api::{self, AppState},
    model_gateway::MockModelClient,
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

fn root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "content-plan-approval-api-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}

fn app(root: &std::path::Path) -> (Router, RuntimeStore) {
    let mut config = RuntimeConfig::from_env();
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
    config.runtime_storage_dir = root.to_path_buf();
    let store = RuntimeStore::with_checkpoint_dir(root);
    let router = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
        config,
    });
    (router, store)
}

fn app_with_producer_auth(root: &std::path::Path) -> Router {
    let mut config = RuntimeConfig::from_env();
    config.policy_profile = RuntimePolicyProfile::Production;
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.internal_admin_token = Some("content-plan-producer-token".to_string());
    config.runtime_storage_dir = root.to_path_buf();
    let store = RuntimeStore::with_checkpoint_dir(root);
    http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
        config,
    })
}

async fn request_json(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn request_json_with_internal_producer(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-anydesign-internal", "true")
        .header("x-runtime-admin-token", "content-plan-producer-token");
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn content_plan_mutations_require_internal_producer_authorization() {
    let root = root();
    let app = app_with_producer_auth(&root);
    let request = json!({
        "planId": "plan-1",
        "revision": 1,
        "contentHash": "a".repeat(64),
        "changeEventId": "change-1"
    });

    let (status, _) = request_json(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-changes",
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, change) = request_json_with_internal_producer(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-changes",
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(change["sequence"], 1);

    let (status, producer) = request_json_with_internal_producer(
        app,
        "GET",
        "/projects/project-1/content-plan-approval-producer",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(producer["ready"], true);
}

#[tokio::test]
async fn content_plan_approval_routes_fail_closed_after_plan_change() {
    let root = root();
    let (app, _) = app(&root);
    let hash_a = "a".repeat(64);
    let hash_b = "b".repeat(64);

    let (status, change) = request_json(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-changes",
        Some(json!({
            "planId": "plan-1",
            "revision": 1,
            "contentHash": hash_a,
            "changeEventId": "change-1"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(change["sequence"], 1);

    let (status, approval) = request_json(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-approvals",
        Some(json!({
            "planId": "plan-1",
            "revision": 1,
            "contentHash": hash_a,
            "confirmationEventId": "confirmation-1"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(approval["schemaVersion"], "content-plan-approval@1");

    let verify_uri = format!(
        "/projects/project-1/content-plan-approvals/verify?planId=plan-1&revision=1&contentHash={hash_a}"
    );
    let (status, verification) = request_json(app.clone(), "GET", &verify_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(verification["state"], "verified");

    let (status, change) = request_json(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-changes",
        Some(json!({
            "planId": "plan-1",
            "revision": 2,
            "contentHash": hash_b,
            "changeEventId": "change-2"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        change["invalidatedApprovalIds"],
        json!([approval["approvalId"]])
    );

    let (status, verification) = request_json(app.clone(), "GET", &verify_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(verification["state"], "invalidated");
    assert_eq!(verification["reason"], "plan_changed");

    let (status, error) = request_json(
        app.clone(),
        "POST",
        "/projects/project-1/content-plan-approvals",
        Some(json!({
            "planId": "plan-1",
            "revision": 1,
            "contentHash": hash_a,
            "confirmationEventId": "confirmation-1"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["errorCode"], "content_plan.confirmation_event_stale");

    let (status, producer) = request_json(
        app,
        "GET",
        "/projects/project-1/content-plan-approval-producer",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(producer["ready"], true);
    assert_eq!(producer["lastSequence"], 3);
}

#[tokio::test]
async fn generation_context_status_does_not_expose_full_context() {
    let root = root();
    let (app, store) = app(&root);
    let run = store
        .create_run(
            "project-1".to_string(),
            anydesign_runtime::types::AgentPhase::Build,
            "build".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;

    let (status, payload) = request_json(
        app,
        "GET",
        &format!("/runs/{}/generation-context-status", run.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["schemaVersion"], "generation-context-status@1");
    assert_eq!(payload["runContractVersion"], "legacy@1");
    assert_eq!(payload["status"], "not_compiled");
    assert!(payload.get("generationContext").is_none());
}
