use super::*;

async fn release_fixture() -> (AppState, String) {
    let mut config = public_auth_disabled_config();
    config.runtime_storage_dir = unique_temp_dir("release-packaging-http");
    config.release_base_image_digest = Some(format!("sha256:{}", "b".repeat(64)));
    config.release_packager_version = Some("packager@1".to_string());
    config.release_registry_repository = Some("registry.example/works".to_string());
    config.release_scan_policy_version = Some("scan@1".to_string());
    let state = http_api::app_state(config);
    let run = state
        .store
        .create_run(
            "release-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    state
        .store
        .upsert_project_runtime_state_with_template_identity(
            "release-project",
            "project".to_string(),
            "astro-website".to_string(),
            "astro-website@runtime-p3".to_string(),
            Some("a".repeat(64)),
            "astro".to_string(),
            Some("astro-website".to_string()),
            Some("0.1.0".to_string()),
            "npm".to_string(),
            "package-lock.json".to_string(),
            "https://registry.npmjs.org/".to_string(),
        )
        .await
        .unwrap();
    let version = state
        .store
        .create_project_version_candidate(
            "release-project",
            &run.id,
            "http://preview.invalid".to_string(),
            None,
            Some("runtime://snapshots/release-project/version".to_string()),
        )
        .await;
    let publish = state
        .store
        .begin_artifact_publish(
            "release-project",
            &run.id,
            "build-1",
            &version.id,
            &"c".repeat(64),
            "runtime://snapshots/release-project/version",
            None,
        )
        .await
        .unwrap();
    state
        .store
        .transition_artifact_publish(
            &publish.id,
            anydesign_runtime::types::ArtifactPublishStatus::Staged,
            Some(&"d".repeat(64)),
            Some("runtime://artifacts/release-project/staged/version"),
            None,
            None,
        )
        .await
        .unwrap();
    state
        .store
        .transition_artifact_publish(
            &publish.id,
            anydesign_runtime::types::ArtifactPublishStatus::Validating,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    state
        .store
        .transition_artifact_publish(
            &publish.id,
            anydesign_runtime::types::ArtifactPublishStatus::Promoting,
            None,
            None,
            Some("runtime://artifacts/release-project/versions/version"),
            None,
        )
        .await
        .unwrap();
    state
        .store
        .commit_artifact_promotion_cas("release-project", &run.id, &version.id, &publish.id, None)
        .await
        .unwrap();
    (state, version.id)
}

#[tokio::test]
async fn create_release_is_idempotent_and_packaging_is_queryable() {
    let (state, version_id) = release_fixture().await;
    let app = http_api::router_with_state(state);
    let mut packaging_id = String::new();
    let mut release_id = String::new();

    for key in ["release-click-1", "release-click-retry"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/projects/release-project/versions/{version_id}/releases"
                    ))
                    .header("content-type", "application/json")
                    .header("idempotency-key", key)
                    .body(Body::from(r#"{"runtimeProfileId":"static-web-v1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 32 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(payload["release"]["versionId"], version_id);
        assert_eq!(payload["release"]["status"], "packaging");
        assert_eq!(payload["packaging"]["status"], "prepared");
        if packaging_id.is_empty() {
            packaging_id = payload["packaging"]["id"].as_str().unwrap().to_string();
            release_id = payload["release"]["id"].as_str().unwrap().to_string();
        } else {
            assert_eq!(payload["packaging"]["id"], packaging_id);
            assert_eq!(payload["release"]["id"], release_id);
        }
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/release-packagings/{packaging_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32 * 1024).await.unwrap()).unwrap();
    assert_eq!(payload["release"]["id"], release_id);
    assert_eq!(payload["packaging"]["id"], packaging_id);
}

#[tokio::test]
async fn create_release_requires_client_idempotency_key() {
    let (state, version_id) = release_fixture().await;
    let response = http_api::router_with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/release-project/versions/{version_id}/releases"
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"runtimeProfileId":"static-web-v1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8 * 1024).await.unwrap()).unwrap();
    assert_eq!(payload["error"], "Idempotency-Key header is required");
}

#[tokio::test]
async fn create_release_rejects_candidate_version_before_packaging() {
    let mut config = public_auth_disabled_config();
    config.runtime_storage_dir = unique_temp_dir("release-candidate-http");
    config.release_base_image_digest = Some(format!("sha256:{}", "b".repeat(64)));
    config.release_packager_version = Some("packager@1".to_string());
    config.release_registry_repository = Some("registry.example/works".to_string());
    config.release_scan_policy_version = Some("scan@1".to_string());
    let state = http_api::app_state(config);
    let run = state
        .store
        .create_run(
            "candidate-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let version = state
        .store
        .create_project_version_candidate(
            "candidate-project",
            &run.id,
            "http://preview.invalid".to_string(),
            None,
            Some("runtime://snapshots/candidate-project/version".to_string()),
        )
        .await;
    let response = http_api::router_with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/candidate-project/versions/{}/releases",
                    version.id
                ))
                .header("content-type", "application/json")
                .header("idempotency-key", "candidate-release")
                .body(Body::from(r#"{"runtimeProfileId":"static-web-v1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}
