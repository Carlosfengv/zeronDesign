use super::*;

#[tokio::test]
async fn design_source_artifact_api_is_authorized_immutable_and_restart_safe() {
    let storage = unique_temp_dir("design-source-artifact-api");
    let mut config = phase_a_contract_config();
    config.runtime_storage_dir = storage.clone();
    config.internal_admin_token = Some("source-secret".to_string());
    let app = http_api::router(config.clone());
    let source = b"# AuthKit\n\r\nFrosted glass cathedral at midnight.\n";
    let digest = sha256_hex(source);
    let request_body = json!({
        "scope": { "projectId": "project-source" },
        "fileName": "DESIGN.md",
        "mediaType": "text/markdown",
        "contentBase64": BASE64_STANDARD.encode(source),
        "clientSha256": digest,
    });

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::OK);

    let hash_mismatch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "source-secret")
                .body(Body::from(
                    json!({
                        "scope": { "projectId": "project-source" },
                        "fileName": "DESIGN.md",
                        "mediaType": "text/markdown",
                        "contentBase64": BASE64_STANDARD.encode(source),
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
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "source-secret")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 16_384).await.unwrap();
    let created_json: Value = serde_json::from_slice(&created_body).unwrap();
    let artifact_id = created_json["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        created_json["artifact"]["scope"]["projectId"],
        "project-source"
    );
    assert_eq!(created_json["artifact"]["sha256"], digest);
    assert_eq!(created_json["artifact"]["sizeBytes"], source.len() as u64);

    let traversal = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "source-secret")
                .body(Body::from(
                    json!({
                        "scope": { "projectId": "project-source" },
                        "fileName": "../../DESIGN.md",
                        "mediaType": "text/markdown",
                        "contentBase64": BASE64_STANDARD.encode(source),
                        "clientSha256": digest,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);

    let mut linked_profile = design_profile_request("project-source", vec!["astro-website"]);
    linked_profile["profile"]["source"] = json!({
        "kind": "imported",
        "primarySourceArtifactId": artifact_id,
        "sourceHash": digest,
        "integrity": "verified"
    });
    let linked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(linked_profile.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(linked.status(), StatusCode::OK);

    let mut mismatched_profile = design_profile_request("other-project", vec!["astro-website"]);
    mismatched_profile["profile"]["source"] = linked_profile["profile"]["source"].clone();
    let mismatched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(mismatched_profile.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);

    let restarted = http_api::router(config);
    let metadata = restarted
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/design-source-artifacts/{artifact_id}"))
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "source-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);

    let content = restarted
        .oneshot(
            Request::builder()
                .uri(format!("/design-source-artifacts/{artifact_id}/content"))
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "source-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(content.headers()["x-design-source-sha256"], digest.as_str());
    assert_eq!(
        to_bytes(content.into_body(), 16_384)
            .await
            .unwrap()
            .as_ref(),
        source
    );

    fs::write(
        storage
            .join("design-source-artifacts")
            .join(&artifact_id)
            .join("source.md"),
        b"tampered",
    )
    .unwrap();
    let corrupted = http_api::router({
        let mut config = phase_a_contract_config();
        config.runtime_storage_dir = storage.clone();
        config.internal_admin_token = Some("source-secret".to_string());
        config
    })
    .oneshot(
        Request::builder()
            .uri(format!("/design-source-artifacts/{artifact_id}/content"))
            .header("x-anydesign-internal", "true")
            .header("x-runtime-admin-token", "source-secret")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(corrupted.status(), StatusCode::INTERNAL_SERVER_ERROR);

    fs::remove_dir_all(storage).unwrap();
}
