use super::*;

#[tokio::test]
async fn version_reports_runtime_build_identity() {
    let mut config = phase_a_contract_config();
    config.repository_commit = "abc123def456".to_string();
    config.repository_dirty = true;
    config.runtime_image_ref = Some("anydesign/runtime:abc123def456-dirty".to_string());
    let response = http_api::router(config)
        .oneshot(
            Request::builder()
                .uri("/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["service"], "anydesign-runtime");
    assert_eq!(payload["repositoryCommit"], "abc123def456");
    assert_eq!(payload["repositoryDirty"], true);
    assert_eq!(payload["imageRef"], "anydesign/runtime:abc123def456-dirty");
}

#[tokio::test]
async fn root_route_returns_runtime_index_for_browser_access() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("AnyDesign Runtime"));
    assert!(body.contains("/health"));
    assert!(body.contains("zeron-real-website-1783303319260"));
    assert!(body.contains("zeron-real-docs-1783303417188"));
}
