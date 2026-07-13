use super::*;

#[tokio::test]
async fn preview_version_returns_pinned_project_version_contract() {
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
                .uri(format!("/preview/project-1/{}", candidate.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["versionId"], candidate.id);
    assert_eq!(
        payload["previewUrl"],
        "http://preview.local/project-1/version-1"
    );
    assert_eq!(payload["status"], "candidate");
}

#[tokio::test]
async fn candidate_preview_proxy_enforces_lease_identity_and_manifest_hash() {
    let manifest_hash = "b".repeat(64);
    let (host, port, upstream) = start_candidate_preview_upstream(manifest_hash.clone()).await;
    let _preview_env = SandboxPreviewEnvOverride::set(&host, port);
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "preview-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "preview-project",
            "sandbox-preview-proxy".to_string(),
            "claim-preview-proxy".to_string(),
            "workspace-preview-proxy".to_string(),
            "pool-preview-proxy".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    store
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            "sandbox-preview-proxy".to_string(),
            Some("sandbox-preview-proxy".to_string()),
            Some("sandbox-uid-preview-proxy".to_string()),
            Some("pod-uid-preview-proxy".to_string()),
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let lease = store
        .create_preview_lease(
            &run.id,
            "build-preview-proxy".to_string(),
            manifest_hash,
            900,
        )
        .await
        .unwrap();
    store
        .upsert_project_access(
            "preview-project",
            "principal-preview-owner".to_string(),
            Some("workspace-preview".to_string()),
            None,
        )
        .await
        .unwrap();
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let public_key_path = unique_temp_dir("public-principal-key").join("current.der");
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
    let owner_token = public_principal_token(&issuer, "principal-preview-owner", "preview-project");
    let cross_project_token =
        public_principal_token(&issuer, "principal-preview-owner", "another-project");
    let wrong_owner_token =
        public_principal_token(&issuer, "principal-not-owner", "preview-project");
    let mut config = phase_a_contract_config();
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![public_key_path];
    config.public_principal_max_ttl_seconds = 60;
    let state = AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    };
    let capture_app = http_api::capture_router_with_state(state.clone());
    let app = http_api::router_with_state(state);

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let cross_project = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .header("authorization", format!("Bearer {cross_project_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!("/projects/preview-project/previews/{}", lease.id),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_project.status(), StatusCode::FORBIDDEN);

    let wrong_owner = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .header("authorization", format!("Bearer {wrong_owner_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!("/projects/preview-project/previews/{}", lease.id),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_owner.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .header("authorization", format!("Bearer {owner_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!("/projects/preview-project/previews/{}", lease.id),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-anydesign-preview-lease"], lease.id);
    let html =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(html.contains(&format!(
        "/projects/preview-project/previews/{}/assets/app.js",
        lease.id
    )));

    let capture = capture_app
        .oneshot(
            Request::builder()
                .uri(format!("/preview-captures/{}/", lease.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(capture.status(), StatusCode::OK);
    let capture_html =
        String::from_utf8(to_bytes(capture.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(capture_html.contains(&format!("/preview-captures/{}/assets/app.js", lease.id)));
    let audit_json = serde_json::to_string(&store.audit_records().await).unwrap();
    assert!(!audit_json.contains(&owner_token));
    assert!(!audit_json.contains(&wrong_owner_token));
    assert!(audit_json.contains(&sha256_hex(b"principal-preview-owner")));

    let mismatched_manifest_lease = store
        .create_preview_lease(
            &run.id,
            "build-preview-proxy".to_string(),
            "c".repeat(64),
            900,
        )
        .await
        .unwrap();
    let mismatched_manifest = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", mismatched_manifest_lease.id))
                .header("authorization", format!("Bearer {owner_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!(
                        "/projects/preview-project/previews/{}",
                        mismatched_manifest_lease.id
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched_manifest.status(), StatusCode::CONFLICT);

    store
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            "sandbox-preview-proxy".to_string(),
            Some("sandbox-preview-proxy".to_string()),
            Some("sandbox-uid-preview-proxy".to_string()),
            Some("replacement-pod-uid".to_string()),
        )
        .await
        .unwrap();
    let replaced = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .header("authorization", format!("Bearer {owner_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!("/projects/preview-project/previews/{}", lease.id),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::CONFLICT);

    store.stop_preview_lease(&lease.id).await.unwrap();
    let stopped = app
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .header("authorization", format!("Bearer {owner_token}"))
                .header(
                    "x-anydesign-preview-prefix",
                    format!("/projects/preview-project/previews/{}", lease.id),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    upstream.abort();
    assert_eq!(stopped.status(), StatusCode::NOT_FOUND);
}
