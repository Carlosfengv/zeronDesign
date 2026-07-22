use super::*;

#[tokio::test]
async fn imported_design_profile_requires_review_before_activation_and_survives_restart() {
    let storage = unique_temp_dir("design-profile-import-activation");
    let mut config = phase_a_contract_config();
    config.runtime_storage_dir = storage.clone();
    config.internal_admin_token = Some("profile-secret".to_string());
    let app = http_api::router(config.clone());
    let source =
        b"# AuthKit\n\n## Tokens\n\n--color-primary: #663af3;\n\nFrosted glass cathedral.\n";
    let create_source = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "profile-secret")
                .body(Body::from(
                    json!({
                        "scope": { "projectId": "project-import" },
                        "fileName": "DESIGN.md",
                        "mediaType": "text/markdown",
                        "contentBase64": BASE64_STANDARD.encode(source),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let source_body = to_bytes(create_source.into_body(), 16_384).await.unwrap();
    let source_json: Value = serde_json::from_slice(&source_body).unwrap();
    let source_id = source_json["artifact"]["id"].as_str().unwrap();

    let import_body = json!({
        "name": "AuthKit Imported",
        "scope": { "projectId": "project-import" },
        "sourceArtifactId": source_id,
    });
    let unauthorized_import = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles/import")
                .header("content-type", "application/json")
                .body(Body::from(import_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized_import.status(), StatusCode::OK);

    let imported = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles/import")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "profile-secret")
                .body(Body::from(import_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let imported_body = to_bytes(imported.into_body(), 128_000).await.unwrap();
    let imported_json: Value = serde_json::from_slice(&imported_body).unwrap();
    let profile_id = imported_json["designProfileDraft"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        imported_json["designProfileDraft"]["schemaVersion"],
        "design-profile@2"
    );
    assert_eq!(imported_json["designProfileDraft"]["status"], "draft");
    assert_eq!(
        imported_json["designProfileDraft"]["candidate"]["tokens"]["color"]["--color-primary"],
        "#663af3"
    );
    assert_eq!(imported_json["conversionReport"]["extractedTokenCount"], 1);
    assert!(imported_json["conversionReport"]["unmappedItems"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let bind_draft = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-import/design-profile")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "designProfileId": profile_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bind_draft.status(), StatusCode::CONFLICT);

    let incomplete_activation = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/design-profiles/{profile_id}/activate"))
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "profile-secret")
                .body(Body::from(json!({ "expectedVersion": 1 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(incomplete_activation.status(), StatusCode::CONFLICT);
    let incomplete_body = to_bytes(incomplete_activation.into_body(), 32_768)
        .await
        .unwrap();
    let incomplete_json: Value = serde_json::from_slice(&incomplete_body).unwrap();
    assert!(incomplete_json["validationIssues"]
        .as_array()
        .is_some_and(|issues| !issues.is_empty()));

    let mut candidate =
        design_profile_request("project-import", vec!["next-app"])["profile"].clone();
    candidate["signatureRules"] = json!([{
        "id": "authkit-primary",
        "category": "color",
        "statement": "The primary action color is AuthKit violet.",
        "priority": "required",
        "appliesTo": ["website"],
        "verification": {
            "kind": "token",
            "token": "color.primary",
            "expected": "#663af3",
            "comparator": { "kind": "color-equivalent" }
        }
    }]);
    candidate["runtimeTokenMapping"]["color.primary"] = json!("#663af3");
    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/design-profiles/{profile_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedVersion": 1,
                        "name": "AuthKit Imported",
                        "profile": candidate,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);

    let stale_update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/design-profiles/{profile_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "expectedVersion": 1,
                        "name": "Stale",
                        "profile": {},
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_update.status(), StatusCode::CONFLICT);

    let activated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/design-profiles/{profile_id}/activate"))
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "profile-secret")
                .body(Body::from(json!({ "expectedVersion": 2 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(activated.status(), StatusCode::OK);
    let activated_body = to_bytes(activated.into_body(), 64_000).await.unwrap();
    let activated_json: Value = serde_json::from_slice(&activated_body).unwrap();
    assert_eq!(activated_json["designProfile"]["version"], 3);
    assert_eq!(activated_json["designProfile"]["status"], "active");
    assert_eq!(
        activated_json["designProfile"]["source"]["integrity"],
        "verified"
    );

    let fidelity = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/design-profiles/{profile_id}/versions/3/fidelity-report?surface=website&template=next-app"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fidelity.status(), StatusCode::OK);
    let fidelity_body = to_bytes(fidelity.into_body(), 64_000).await.unwrap();
    let fidelity_json: Value = serde_json::from_slice(&fidelity_body).unwrap();
    assert_eq!(
        fidelity_json["styleContractVersion"],
        "runtime-style-contract@p3"
    );
    assert_eq!(fidelity_json["sourceHashMatches"], true);
    assert_eq!(
        fidelity_json["requiredSignatureRuleIds"],
        json!(["authkit-primary"])
    );
    assert_eq!(
        fidelity_json["capsuleIncludedRuleIds"],
        json!(["authkit-primary"])
    );
    assert_eq!(fidelity_json["capsuleMissingRuleIds"], json!([]));

    drop(app);
    let restarted = http_api::router(config);
    let recovered = restarted
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/design-profiles/{profile_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered_body = to_bytes(recovered.into_body(), 64_000).await.unwrap();
    let recovered_json: Value = serde_json::from_slice(&recovered_body).unwrap();
    assert_eq!(recovered_json["designProfile"]["version"], 3);

    let recovered_report = restarted
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/design-profiles/{profile_id}/versions/2/conversion-report"
                ))
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "profile-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovered_report.status(), StatusCode::OK);
    let report_body = to_bytes(recovered_report.into_body(), 128_000)
        .await
        .unwrap();
    let report_json: Value = serde_json::from_slice(&report_body).unwrap();
    assert_eq!(report_json["requiredSignatureRuleCount"], 1);

    fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn design_profile_api_create_bind_and_resolve_for_runs() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-1", vec!["next-app"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 16384).await.unwrap();
    let created_payload: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_payload["designProfile"]["id"].as_str().unwrap();
    assert_eq!(created_payload["designProfile"]["version"], 1);
    assert_eq!(
        created_payload["designProfile"]["schemaVersion"],
        "design-profile@1"
    );
    assert_eq!(
        created_payload["designProfile"]["components"]["primitives"]["button"]["role"],
        "clear action"
    );
    assert!(
        created_payload["designProfile"]["components"]["primitives"]["button"]
            .get("intent")
            .is_none()
    );

    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/design-profiles/{profile_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);

    let update_profile = design_profile_request("project-1", vec!["next-app"])["profile"].clone();
    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/design-profiles/{profile_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Harness Calm Ops v2",
                        "profile": update_profile
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = to_bytes(updated.into_body(), 16384).await.unwrap();
    let updated_payload: Value = serde_json::from_slice(&updated_body).unwrap();
    assert_eq!(updated_payload["designProfile"]["id"], profile_id);
    assert_eq!(updated_payload["designProfile"]["version"], 2);
    assert_eq!(
        updated_payload["designProfile"]["name"],
        "Harness Calm Ops v2"
    );

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/design-profiles?projectId=project-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body = to_bytes(listed.into_body(), 16384).await.unwrap();
    let listed_payload: Value = serde_json::from_slice(&listed_body).unwrap();
    assert_eq!(
        listed_payload["designProfiles"].as_array().unwrap().len(),
        1
    );

    let bound = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-1/design-profile")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "designProfileId": profile_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bound.status(), StatusCode::OK);

    let active = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/design-profile")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let active_body = to_bytes(active.into_body(), 16384).await.unwrap();
    let active_payload: Value = serde_json::from_slice(&active_body).unwrap();
    assert_eq!(active_payload["designProfile"]["id"], profile_id);

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "contentSources": [
                                ContentSource::readable("source-1", "prompt", "Make a website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let started_body = to_bytes(started.into_body(), 4096).await.unwrap();
    let started_payload: Value = serde_json::from_slice(&started_body).unwrap();
    let run_id = started_payload["runId"].as_str().unwrap();
    let run = store.get_run(run_id).await.unwrap();
    assert_eq!(run.design_profile_id.as_deref(), Some(profile_id));
    assert_eq!(run.design_profile_version, Some(2));
    assert!(run.design_profile_hash.is_some());

    let archived = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/design-profiles/{profile_id}/archive"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    let archived_body = to_bytes(archived.into_body(), 16384).await.unwrap();
    let archived_payload: Value = serde_json::from_slice(&archived_body).unwrap();
    assert_eq!(archived_payload["designProfile"]["status"], "archived");
    assert_eq!(archived_payload["designProfile"]["version"], 3);

    let versions = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/design-profiles/{profile_id}/versions"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(versions.status(), StatusCode::OK);
    let versions_body = to_bytes(versions.into_body(), 16384).await.unwrap();
    let versions_payload: Value = serde_json::from_slice(&versions_body).unwrap();
    assert_eq!(versions_payload["designProfileId"], profile_id);
    let version_numbers = versions_payload["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| profile["version"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(version_numbers, vec![1, 2, 3]);

    let diff = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/design-profiles/{profile_id}/diff?fromVersion=1&toVersion=2"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diff.status(), StatusCode::OK);
    let diff_body = to_bytes(diff.into_body(), 16384).await.unwrap();
    let diff_payload: Value = serde_json::from_slice(&diff_body).unwrap();
    assert_eq!(diff_payload["fromVersion"], 1);
    assert_eq!(diff_payload["toVersion"], 2);
    assert!(diff_payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["path"] == "name"
            && change["before"] == "Harness Calm Ops"
            && change["after"] == "Harness Calm Ops v2"));
    assert!(!diff_payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| matches!(
            change["path"].as_str(),
            Some("id" | "version" | "createdAt" | "updatedAt")
        )));

    let archive_diff = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/design-profiles/{profile_id}/diff?fromVersion=2&toVersion=3"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archive_diff.status(), StatusCode::OK);
    let archive_diff_body = to_bytes(archive_diff.into_body(), 16384).await.unwrap();
    let archive_diff_payload: Value = serde_json::from_slice(&archive_diff_body).unwrap();
    assert!(archive_diff_payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["path"] == "status"
            && change["before"] == "active"
            && change["after"] == "archived"));

    let listed_active = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/design-profiles?projectId=project-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed_active_body = to_bytes(listed_active.into_body(), 16384).await.unwrap();
    let listed_active_payload: Value = serde_json::from_slice(&listed_active_body).unwrap();
    assert_eq!(
        listed_active_payload["designProfiles"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let listed_with_archived = app
        .oneshot(
            Request::builder()
                .uri("/design-profiles?projectId=project-1&includeArchived=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed_with_archived_body = to_bytes(listed_with_archived.into_body(), 16384)
        .await
        .unwrap();
    let listed_with_archived_payload: Value =
        serde_json::from_slice(&listed_with_archived_body).unwrap();
    assert_eq!(
        listed_with_archived_payload["designProfiles"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
