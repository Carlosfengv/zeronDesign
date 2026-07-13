use super::*;

async fn brief_fixture() -> (RuntimeStore, anydesign_runtime::types::AgentRun, String) {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "brief-project".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief_draft(&run.id, website_brief())
        .await
        .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    (store, run, brief_id)
}

fn brief_app(store: RuntimeStore) -> axum::Router {
    http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    })
}

#[tokio::test]
async fn get_brief_returns_structured_draft_and_owner_context() {
    let (store, run, brief_id) = brief_fixture().await;
    let response = brief_app(store)
        .oneshot(
            Request::builder()
                .uri(format!("/briefs/{brief_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap()).unwrap();
    assert_eq!(payload["briefId"], brief_id);
    assert_eq!(payload["projectId"], "brief-project");
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "draft");
    assert_eq!(payload["runStatus"], "needs_user_input");
    assert_eq!(payload["brief"]["recommendedTemplate"], "astro-website");
}

#[tokio::test]
async fn confirm_brief_reuses_run_lifecycle_and_is_idempotent() {
    let (store, run, brief_id) = brief_fixture().await;
    let app = brief_app(store.clone());

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/briefs/{brief_id}/confirm"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(payload["status"], "confirmed");
        assert_eq!(payload["runStatus"], "completed");
    }

    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        store
            .events(&run.id)
            .await
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::RunCompleted { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn confirm_brief_rejects_draft_that_is_not_awaiting_confirmation() {
    let (store, run, brief_id) = brief_fixture().await;
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    let response = brief_app(store)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/briefs/{brief_id}/confirm"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}
