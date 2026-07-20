use super::*;

fn one_pixel_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[32, 96, 224, 255]).unwrap();
    drop(writer);
    bytes
}

#[tokio::test]
async fn visual_artifact_api_normalizes_authorizes_and_survives_restart() {
    let storage = unique_temp_dir("visual-artifact-api");
    let mut config = phase_a_contract_config();
    config.runtime_storage_dir = storage.clone();
    config.internal_admin_token = Some("visual-service-secret".to_string());
    let mut state = http_api::app_state(config.clone());
    state.model = Arc::new(MockModelClient::new(vec![ModelResponse::TextOnly(
        "VISUAL_REVIEW_PASS".to_string(),
    )]));
    let store = state.store.clone();
    let run = store
        .create_run(
            "project-visual".to_string(),
            AgentPhase::Review,
            "visual-review".to_string(),
            "resource:vision-model".to_string(),
            vec![],
        )
        .await;
    let snapshot = store
        .create_draft_snapshot(
            "project-visual",
            "runtime://snapshots/project-visual/draft-1".to_string(),
            "a".repeat(64),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "runtime-dependency-policy@1".to_string(),
            "b".repeat(64),
            &run.id,
            None,
            None,
        )
        .await
        .unwrap();
    let app = http_api::router_with_state(state);
    let input = one_pixel_png();
    let request_body = json!({
        "contentBase64": BASE64_STANDARD.encode(&input),
        "clientSha256": sha256_hex(&input),
        "originMetadata": { "label": "hero reference" },
    });

    let hash_mismatch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-visual/visual-artifacts")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "contentBase64": BASE64_STANDARD.encode(&input),
                        "clientSha256": "0".repeat(64),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hash_mismatch.status(), StatusCode::BAD_REQUEST);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-visual/visual-artifacts")
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 32_768).await.unwrap();
    let created_json: Value = serde_json::from_slice(&created_body).unwrap();
    let artifact = &created_json["artifact"];
    let artifact_id = artifact["id"].as_str().unwrap();
    assert_eq!(artifact["schemaVersion"], "visual-artifact@1");
    assert_eq!(artifact["projectId"], "project-visual");
    assert_eq!(artifact["mediaType"], "image/png");
    assert_eq!(artifact["width"], 1);
    assert_eq!(artifact["height"], 1);
    assert_eq!(artifact["origin"], "upload");
    assert_eq!(artifact["originMetadata"]["label"], "hero reference");

    let binding_body = json!({
        "artifactId": artifact_id,
        "role": "reference",
        "route": "/",
        "viewport": { "width": 1440, "height": 900, "deviceScaleFactor": 1 },
        "target": {
            "kind": "static-snapshot",
            "snapshotId": snapshot.snapshot_id,
            "sourceHash": snapshot.source_hash,
        },
        "order": 0,
    });
    let bound = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/project-visual/runs/{}/visual-bindings",
                    run.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(binding_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bound.status(), StatusCode::OK);
    let bound_json: Value =
        serde_json::from_slice(&to_bytes(bound.into_body(), 32_768).await.unwrap()).unwrap();
    assert_eq!(bound_json["binding"]["artifactId"], artifact_id);
    assert_eq!(bound_json["binding"]["runId"], run.id);

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/project-visual/runs/{}/visual-bindings",
                    run.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_json: Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 32_768).await.unwrap()).unwrap();
    assert_eq!(listed_json["bindings"].as_array().unwrap().len(), 1);

    let visual_review = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-visual/visual-reviews")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "mode": "advisory",
                        "target": binding_body["target"],
                        "bindings": [{
                            "artifactId": artifact_id,
                            "role": "candidate",
                            "route": "/",
                            "viewport": { "width": 1440, "height": 900, "deviceScaleFactor": 1 },
                            "order": 0
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(visual_review.status(), StatusCode::OK);
    let visual_review_json: Value = serde_json::from_slice(
        &to_bytes(visual_review.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(visual_review_json["state"]["status"], "passed");

    let internal_path = format!(
        "/internal/runs/{}/visual-artifacts/{artifact_id}/content",
        run.id
    );
    let internal_unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&internal_path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(internal_unauthorized.status(), StatusCode::UNAUTHORIZED);
    let internal_content = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&internal_path)
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "visual-service-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(internal_content.status(), StatusCode::OK);
    assert_eq!(internal_content.headers()["x-visual-artifact-width"], "1");

    let protected_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/projects/project-visual/visual-artifacts/{artifact_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(protected_delete.status(), StatusCode::OK);
    let protected_json: Value = serde_json::from_slice(
        &to_bytes(protected_delete.into_body(), 32_768)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(protected_json["artifact"]["retentionState"], "protected");

    let unbound_created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-visual/visual-artifacts")
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let unbound_json: Value =
        serde_json::from_slice(&to_bytes(unbound_created.into_body(), 32_768).await.unwrap())
            .unwrap();
    let unbound_id = unbound_json["artifact"]["id"].as_str().unwrap();
    let pending_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/projects/project-visual/visual-artifacts/{unbound_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let pending_json: Value =
        serde_json::from_slice(&to_bytes(pending_delete.into_body(), 32_768).await.unwrap())
            .unwrap();
    assert_eq!(
        pending_json["artifact"]["retentionState"],
        "deletion_pending"
    );
    let gc = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/visual-artifacts/gc")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "visual-service-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(gc.status(), StatusCode::OK);
    let purged_json: Value =
        serde_json::from_slice(&to_bytes(gc.into_body(), 32_768).await.unwrap()).unwrap();
    assert_eq!(purged_json["purgedArtifactIds"], json!([unbound_id]));
    let purged_get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/project-visual/visual-artifacts/{unbound_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(purged_get.status(), StatusCode::NOT_FOUND);

    let wrong_project = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/other-project/visual-artifacts/{artifact_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_project.status(), StatusCode::NOT_FOUND);

    let restarted = http_api::router(config);
    let metadata = restarted
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/project-visual/visual-artifacts/{artifact_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);

    let content = restarted
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/project-visual/visual-artifacts/{artifact_id}/content"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(content.headers()["content-type"], "image/png");
    assert_eq!(content.headers()["cache-control"], "private, no-store");
    let expected_sha = artifact["sha256"].as_str().unwrap().to_string();
    let content_bytes = to_bytes(content.into_body(), 16_384).await.unwrap();
    assert_eq!(sha256_hex(&content_bytes), expected_sha);

    std::fs::remove_dir_all(storage).unwrap();
}
