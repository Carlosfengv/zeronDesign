use super::*;
use anydesign_runtime::draft_preview::StartDraftPreview;

#[tokio::test]
async fn draft_preview_api_is_durable_authorized_and_streams_terminal_events() {
    let config = phase_a_contract_config();
    let state = http_api::app_state(config.clone());
    let session = state
        .store
        .draft_preview_store()
        .start(StartDraftPreview {
            project_id: "draft-project".to_string(),
            sandbox_binding_id: "binding-draft".to_string(),
            template_id: "next-app".to_string(),
            base_snapshot_id: "snapshot-base".to_string(),
            base_version_id: None,
            proxy_url: "https://runtime.test/previews/lease-draft/".to_string(),
            writer_ttl_seconds: 120,
        })
        .unwrap();
    let app = http_api::router_with_state(state.clone());

    let current = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/draft-project/draft-preview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    let current: Value =
        serde_json::from_slice(&to_bytes(current.into_body(), 32 * 1024).await.unwrap()).unwrap();
    assert_eq!(current["sessionId"], session.session_id);

    let heartbeat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/draft-preview-sessions/{}/heartbeat",
                    session.session_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "writerLeaseId": session.writer_lease_id,
                        "sessionEpoch": session.session_epoch,
                        "ttlSeconds": 180,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::OK);

    let premature_takeover = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/draft-preview-sessions/{}/takeover",
                    session.session_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "expectedSessionEpoch": 1, "ttlSeconds": 120 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(premature_takeover.status(), StatusCode::CONFLICT);

    state
        .store
        .draft_preview_store()
        .stop(&session.session_id, "test completed".to_string())
        .unwrap();
    let events = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/draft-preview-sessions/{}/events",
                    session.session_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    assert_eq!(events.headers()["content-type"], "text/event-stream");
    let body = String::from_utf8(
        to_bytes(events.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("event: preview.dev_starting"));
    assert!(body.contains("event: preview.dev_stopped"));

    let restarted = http_api::router(config);
    let restored = restarted
        .oneshot(
            Request::builder()
                .uri(format!("/draft-preview-sessions/{}", session.session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
}
