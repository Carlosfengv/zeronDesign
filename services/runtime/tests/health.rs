use anydesign_runtime::{config::RuntimeConfig, http_api};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ready_when_config_loads() {
    let app = http_api::router(RuntimeConfig::from_env());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "ready");
}
