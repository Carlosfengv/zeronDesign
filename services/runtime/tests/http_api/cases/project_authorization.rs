use super::*;

fn scoped_token(
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
            jti: format!("project-auth-{principal_id}-{operation}-0001"),
            exp: 0,
            iat: 0,
            project_id: project_id.to_string(),
            operations: vec![operation.to_string()],
        })
        .unwrap()
}

#[tokio::test]
async fn required_public_auth_protects_project_reads_and_permission_mutations() {
    let key_path = unique_temp_dir("project-principal-key").join("current.der");
    std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
    let signing_key = SigningKey::from_bytes(&[41_u8; 32]);
    std::fs::write(
        &key_path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let mut config = phase_a_contract_config();
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![key_path.clone()];
    config.public_principal_issuer = "anydesign-bff".into();
    config.public_principal_audience = "anydesign-runtime-public".into();
    let state = http_api::app_state(config);
    state
        .store
        .upsert_project_access("project-1", "owner-1".into(), None, None)
        .await
        .unwrap();
    let run = state
        .store
        .create_run(
            "project-1".into(),
            AgentPhase::Export,
            "export".into(),
            "internal-balanced".into(),
            vec![],
        )
        .await;
    state
        .store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = state
        .store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    state
        .store
        .append_conversation_item(
            "project-1",
            Some(&run.id),
            "agent_message",
            Some("assistant"),
            "private project message",
            None,
        )
        .await;

    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let read_token = scoped_token(&issuer, "owner-1", "project-1", PROJECT_READ_OPERATION);
    let write_token = scoped_token(&issuer, "owner-1", "project-1", PROJECT_WRITE_OPERATION);
    let wrong_project_token =
        scoped_token(&issuer, "owner-1", "project-2", PROJECT_WRITE_OPERATION);
    let app = http_api::router_with_state(state.clone());

    let unauthorized_read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized_read.status(), StatusCode::UNAUTHORIZED);
    let authorized_read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation")
                .header("authorization", format!("Bearer {read_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized_read.status(), StatusCode::OK);

    let decision_body = json!({ "decision": "ask" }).to_string();
    for (token, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(read_token.as_str()), StatusCode::FORBIDDEN),
        (Some(wrong_project_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/permissions/{}/decision", permission.id))
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(decision_body.clone())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(
            state
                .store
                .pending_permission(&permission.id)
                .await
                .unwrap()
                .status,
            "pending"
        );
    }

    let authorized_decision = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {write_token}"))
                .body(Body::from(decision_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized_decision.status(), StatusCode::OK);
    assert_eq!(
        state
            .store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "ask"
    );

    std::fs::remove_dir_all(key_path.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn promoted_preview_metadata_requires_preview_scope() {
    let key_path = unique_temp_dir("promoted-preview-key").join("current.der");
    std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
    let signing_key = SigningKey::from_bytes(&[42_u8; 32]);
    std::fs::write(
        &key_path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let mut config = phase_a_contract_config();
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![key_path.clone()];
    config.public_principal_issuer = "anydesign-bff".into();
    config.public_principal_audience = "anydesign-runtime-public".into();
    let state = http_api::app_state(config);
    state
        .store
        .upsert_project_access("project-preview", "owner-1".into(), None, None)
        .await
        .unwrap();
    let run = state
        .store
        .create_run(
            "project-preview".into(),
            AgentPhase::Build,
            "build".into(),
            "internal-balanced".into(),
            vec![],
        )
        .await;
    let version = state
        .store
        .create_project_version_candidate(
            "project-preview",
            &run.id,
            "http://preview.invalid".into(),
            None,
            Some("fixture://source".into()),
        )
        .await;
    state
        .store
        .promote_project_version("project-preview", &run.id, &version.id)
        .await
        .unwrap();
    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let project_read = scoped_token(
        &issuer,
        "owner-1",
        "project-preview",
        PROJECT_READ_OPERATION,
    );
    let preview_read = scoped_token(
        &issuer,
        "owner-1",
        "project-preview",
        PREVIEW_READ_OPERATION,
    );
    let app = http_api::router_with_state(state);

    for (token, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(project_read.as_str()), StatusCode::FORBIDDEN),
        (Some(preview_read.as_str()), StatusCode::OK),
    ] {
        let mut request = Request::builder().uri("/preview/project-preview/current");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    std::fs::remove_dir_all(key_path.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn structured_brief_read_and_confirmation_enforce_project_scopes() {
    let key_path = unique_temp_dir("brief-principal-key").join("current.der");
    std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
    let signing_key = SigningKey::from_bytes(&[43_u8; 32]);
    std::fs::write(
        &key_path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let mut config = phase_a_contract_config();
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Required;
    config.public_principal_public_key_files = vec![key_path.clone()];
    config.public_principal_issuer = "anydesign-bff".into();
    config.public_principal_audience = "anydesign-runtime-public".into();
    let state = http_api::app_state(config);
    state
        .store
        .upsert_project_access("brief-project", "owner-1".into(), None, None)
        .await
        .unwrap();
    let run = state
        .store
        .create_run(
            "brief-project".into(),
            AgentPhase::Brief,
            "brief".into(),
            "internal-balanced".into(),
            vec![],
        )
        .await;
    let brief_id = state
        .store
        .write_brief_draft(&run.id, website_brief())
        .await
        .unwrap();
    state
        .store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let read_token = scoped_token(&issuer, "owner-1", "brief-project", PROJECT_READ_OPERATION);
    let write_token = scoped_token(&issuer, "owner-1", "brief-project", PROJECT_WRITE_OPERATION);
    let app = http_api::router_with_state(state.clone());

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/briefs/{brief_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let authorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/briefs/{brief_id}"))
                .header("authorization", format!("Bearer {read_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);

    let read_only_confirmation = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/briefs/{brief_id}/confirm"))
                .header("authorization", format!("Bearer {read_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read_only_confirmation.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        state.store.brief_status(&brief_id).await,
        Some(BriefStatus::Draft)
    );

    let confirmed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/briefs/{brief_id}/confirm"))
                .header("authorization", format!("Bearer {write_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);
    assert_eq!(
        state.store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );

    std::fs::remove_dir_all(key_path.parent().unwrap()).unwrap();
}
