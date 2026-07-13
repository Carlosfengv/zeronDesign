use super::*;

#[tokio::test]
async fn project_access_internal_route_requires_admin_and_persists_across_store_restart() {
    let storage = unique_temp_dir("project-access-persistence");
    let store = RuntimeStore::with_checkpoint_dir(storage.clone());
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("admin-project-access-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let body = json!({
        "ownerPrincipalId": "principal-owner-1",
        "workspaceId": "workspace-1",
        "organizationId": "organization-1"
    })
    .to_string();

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/projects/project-access-1/access")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let allowed = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/projects/project-access-1/access")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-project-access-token")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);

    drop(store);
    let restarted = RuntimeStore::with_checkpoint_dir(storage);
    let record = restarted
        .get_project_access("project-access-1")
        .await
        .unwrap();
    assert_eq!(record.owner_principal_id, "principal-owner-1");
    assert_eq!(record.workspace_id.as_deref(), Some("workspace-1"));
    assert_eq!(record.organization_id.as_deref(), Some("organization-1"));
}

#[tokio::test]
async fn release_evidence_and_sandbox_release_routes_fail_closed_without_admin_identity() {
    let store = RuntimeStore::new();
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("release-evidence-admin".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    for (method, uri) in [
        ("GET", "/internal/projects/project-1/release-evidence"),
        ("POST", "/internal/projects/project-1/release-sandbox"),
    ] {
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-anydesign-internal", "true")
                    .header("x-runtime-admin-token", "wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn production_initial_run_requires_registered_project_access() {
    let store = RuntimeStore::new();
    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let public_key_path = unique_temp_dir("initial-run-public-key").join("current.der");
    fs::create_dir_all(public_key_path.parent().unwrap()).unwrap();
    fs::write(
        &public_key_path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let token = scoped_project_token(
        &issuer,
        "principal-owned",
        "owned-project",
        PROJECT_WRITE_OPERATION,
    );
    let mut config = phase_a_contract_config();
    config.policy_profile = RuntimePolicyProfile::Production;
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![public_key_path];
    config.validate_startup().unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let request_body = |workspace_id: &str| {
        json!({
            "projectId": "owned-project",
            "phase": "brief",
            "agentProfile": "brief",
            "inputContext": { "workspaceId": workspace_id }
        })
        .to_string()
    };

    let missing_auth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(request_body("workspace-owned")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);

    let missing_access = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(request_body("workspace-owned")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_access.status(), StatusCode::FORBIDDEN);

    store
        .upsert_project_access(
            "owned-project",
            "principal-owned".to_string(),
            Some("workspace-owned".to_string()),
            None,
        )
        .await
        .unwrap();
    let drifted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(request_body("workspace-other")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(drifted.status(), StatusCode::CONFLICT);

    let allowed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(request_body("workspace-owned")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

fn scoped_project_token(
    issuer: &PublicPrincipalJwtIssuer,
    principal_id: &str,
    project_id: &str,
    operation: &str,
) -> String {
    issuer
        .issue(PublicPrincipalClaims {
            iss: String::new(),
            aud: String::new(),
            sub: principal_id.to_string(),
            jti: format!("initial-run-{principal_id}-0001"),
            exp: 0,
            iat: 0,
            project_id: project_id.to_string(),
            operations: vec![operation.to_string()],
        })
        .unwrap()
}

#[tokio::test]
async fn preview_version_rejects_cross_project_version_lookup() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/version-1".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/preview/project-2/{}", candidate.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("not found for project: project-2"));
}

#[tokio::test]
async fn product_promote_http_route_is_not_exposed() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/previews/promote")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "runId": "run-1",
                        "candidateVersionId": "version-1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn internal_template_build_route_is_disabled_by_default() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/template-build")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "template": "astro-website",
                        "audience": "Internal teams",
                        "visualDirection": "Clear technical site"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        "internal template build endpoint is disabled"
    );
}

#[tokio::test]
async fn internal_template_build_route_requires_service_authorization_when_enabled() {
    let store = RuntimeStore::new();
    let mut config = public_auth_disabled_config();
    config.enable_internal_template_build_api = true;
    config.internal_admin_token = Some("test-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/template-build")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "template": "astro-website",
                        "audience": "Internal teams",
                        "visualDirection": "Clear technical site"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        "internal template build requires service authorization"
    );
    let audit = store.audit_records().await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].tool, "internal.template_build");
    assert_eq!(audit[0].decision, "deny");
}

#[tokio::test]
async fn internal_promote_route_requires_service_authorization_when_enabled() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/candidate".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;
    let mut config = public_auth_disabled_config();
    config.enable_internal_promote_api = true;
    config.internal_admin_token = Some("test-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/previews/promote")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "runId": run.id,
                        "candidateVersionId": candidate.id
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        "internal preview promotion requires service authorization"
    );
    let audit = store.audit_records().await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].tool, "internal.previews.promote");
    assert_eq!(audit[0].decision, "deny");
}

#[tokio::test]
async fn internal_promote_route_promotes_candidate_with_audit_when_authorized() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/candidate".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;
    let mut config = public_auth_disabled_config();
    config.enable_internal_promote_api = true;
    config.internal_admin_token = Some("test-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/previews/promote")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "test-token")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "runId": run.id,
                        "candidateVersionId": candidate.id,
                        "gateReport": {
                            "previewAccessible": true,
                            "screenshotAvailable": true
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["versionId"], candidate.id);
    assert_eq!(payload["status"], "promoted");
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        candidate.id
    );
    assert!(store
        .events(&run.id)
        .await
        .iter()
        .any(|event| { serde_json::to_value(event).unwrap()["type"] == "preview.updated" }));
    let audit = store.audit_records().await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].tool, "internal.previews.promote");
    assert_eq!(audit[0].decision, "allow");
}
