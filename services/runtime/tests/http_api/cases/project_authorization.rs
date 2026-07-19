use super::*;
use anydesign_runtime::{
    design_context::{
        compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
    },
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::DesignProfile,
};
use chrono::Utc;

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

fn authorization_dcp_profile(id: &str, scope_project_id: &str) -> DesignProfile {
    let now = Utc::now();
    DesignProfile {
        id: id.to_string(),
        schema_version: "design-profile@1".to_string(),
        name: "Authorization DCP profile".to_string(),
        status: "active".to_string(),
        version: 1,
        scope: json!({ "projectId": scope_project_id }),
        source: json!({ "kind": "manual" }),
        product: json!({
            "name": "Authorization fixture",
            "category": "test",
            "privateFixtureValue": "private-profile-copy"
        }),
        brand: json!({}),
        visual: json!({ "direction": "clear" }),
        tokens: json!({}),
        runtime_token_mapping: json!({
            "color.background": "#ffffff",
            "color.surface": "#f8fafc",
            "color.surfaceStrong": "#e2e8f0",
            "color.text": "#0f172a",
            "color.muted": "#475569",
            "color.primary": "#2563eb",
            "color.primaryContrast": "#ffffff",
            "color.border": "#cbd5e1",
            "radius.card": "8px",
            "radius.control": "6px",
            "font.sans": "Inter, sans-serif",
            "shadow.soft": "none"
        }),
        extended_token_mapping: json!({}),
        components: json!({}),
        website_context: json!({ "enforcementMode": "observe" }),
        content: json!({}),
        accessibility: json!({}),
        technical: json!({ "allowedTemplates": ["astro-website"] }),
        governance: json!({ "conflictBehavior": "ask" }),
        signature_rules: Vec::new(),
        overrides: json!({}),
        created_at: now,
        updated_at: now,
    }
}

async fn attach_authorization_dcp(
    store: &RuntimeStore,
    run_project_id: &str,
    profile: &DesignProfile,
) -> anyhow::Result<String> {
    let run = store
        .create_run(
            run_project_id.to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .attach_run_effective_design_profile(
            &run.id,
            profile,
            Some("website"),
            Some("astro-website"),
        )
        .await?;
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("astro-website").unwrap())
        .unwrap();
    let dcp = compile_website_design_context(
        &profile.effective_for("website", "astro-website").unwrap(),
        &website_brief(),
        &template,
        &DesignContextCompileOptions::default(),
    )
    .map_err(anyhow::Error::msg)?;
    store
        .attach_run_design_context(
            &run.id,
            &dcp,
            &VerifierRegistry::discover_with_executables(
                Some("/private/runtime-browser"),
                Some("/bin/true"),
            ),
        )
        .await?;
    Ok(run.id)
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
        .upsert_project_access("project-1", "owner-1".into(), "ws-test".into())
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
async fn design_profiles_are_project_private_and_platform_profiles_are_read_only_to_users() {
    let key_path = unique_temp_dir("design-profile-principal-key").join("current.der");
    std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
    let signing_key = SigningKey::from_bytes(&[47_u8; 32]);
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
        .upsert_project_access(
            "project-private",
            "owner-private".into(),
            "ws-private".into(),
        )
        .await
        .unwrap();
    state
        .store
        .upsert_project_access("project-other", "owner-other".into(), "ws-other".into())
        .await
        .unwrap();
    let private_profile = authorization_dcp_profile("profile-private", "project-private");
    state
        .store
        .create_design_profile(private_profile.clone())
        .await
        .unwrap();
    let mut platform_profile = authorization_dcp_profile("profile-platform", "unused");
    platform_profile.scope = json!({ "platform": true });
    state
        .store
        .create_design_profile(platform_profile.clone())
        .await
        .unwrap();

    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let private_read = scoped_token(
        &issuer,
        "owner-private",
        "project-private",
        PROJECT_READ_OPERATION,
    );
    let other_read = scoped_token(
        &issuer,
        "owner-other",
        "project-other",
        PROJECT_READ_OPERATION,
    );
    let other_write = scoped_token(
        &issuer,
        "owner-other",
        "project-other",
        PROJECT_WRITE_OPERATION,
    );
    let app = http_api::router_with_state(state);

    let private_visible = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/design-profiles/profile-private")
                .header("authorization", format!("Bearer {private_read}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(private_visible.status(), StatusCode::OK);

    let cross_project = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/design-profiles/profile-private")
                .header("authorization", format!("Bearer {other_read}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_project.status(), StatusCode::FORBIDDEN);

    let platform_visible = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/design-profiles/profile-platform")
                .header("authorization", format!("Bearer {other_read}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(platform_visible.status(), StatusCode::OK);

    let platform_write = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/design-profiles/profile-platform")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {other_write}"))
                .body(Body::from(
                    json!({ "name": "changed", "profile": {} }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(platform_write.status(), StatusCode::FORBIDDEN);

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
        .upsert_project_access("project-preview", "owner-1".into(), "ws-test".into())
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
        .upsert_project_access("brief-project", "owner-1".into(), "ws-test".into())
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

#[tokio::test]
async fn design_context_diagnostics_require_run_project_read_and_expose_only_frozen_summary() {
    let key_path = unique_temp_dir("dcp-principal-key").join("current.der");
    std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
    let signing_key = SigningKey::from_bytes(&[44_u8; 32]);
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
        .upsert_project_access("dcp-auth-project", "owner-1".into(), "ws-test".into())
        .await
        .unwrap();
    state
        .store
        .upsert_project_access("another-project", "owner-1".into(), "ws-other".into())
        .await
        .unwrap();

    // The profile is intentionally not persisted in the current profile store. Historical
    // diagnostics must remain available from the immutable Run snapshot alone.
    let profile = authorization_dcp_profile("dcp-auth-profile", "dcp-auth-project");
    let run_id = attach_authorization_dcp(&state.store, "dcp-auth-project", &profile)
        .await
        .unwrap();
    state
        .store
        .append_conversation_item(
            "dcp-auth-project",
            Some(&run_id),
            "design_profile_fidelity_checked",
            Some("assistant"),
            "Fixture fidelity failure.",
            Some(json!({
                "status": "failed",
                "checkedAt": "2026-07-15T00:00:00Z",
                "outputVersionId": "version-fidelity-1",
                "previewUrl": "http://private-preview.invalid/private-profile-copy",
                "browserExecutable": "/private/runtime-browser",
                "requiredFailedRuleIds": ["craft:accessibility-baseline:image-alt"],
                "assertions": [{
                    "ruleId": "craft:accessibility-baseline:image-alt",
                    "recipeId": "accessibility-baseline",
                    "priority": "required",
                    "kind": "a11y",
                    "route": "/",
                    "viewport": null,
                    "selector": "main img",
                    "property": null,
                    "rawActual": "private-profile-copy",
                    "normalizedActual": ["private-profile-copy"],
                    "expected": [],
                    "comparator": "equals",
                    "passed": false,
                    "reason": "Image alternative text is required."
                }],
                "repairContext": {
                    "globalCssFile": "/workspace/project/src/styles/global.css",
                    "componentRoot": "/private/component-root",
                    "instructions": ["Repair the imported page source."],
                    "runtimeTokenMapping": { "color.primary": "private-profile-copy" }
                }
            })),
        )
        .await;
    let wrong_scope_profile = authorization_dcp_profile("wrong-scope-profile", "another-project");
    let wrong_scope_error =
        attach_authorization_dcp(&state.store, "dcp-auth-project", &wrong_scope_profile)
            .await
            .unwrap_err();
    assert!(wrong_scope_error
        .to_string()
        .contains("not visible to the run project"));

    let issuer = PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    );
    let read_token = scoped_token(
        &issuer,
        "owner-1",
        "dcp-auth-project",
        PROJECT_READ_OPERATION,
    );
    let cross_project_token = scoped_token(
        &issuer,
        "owner-1",
        "another-project",
        PROJECT_READ_OPERATION,
    );
    let wrong_owner_token = scoped_token(
        &issuer,
        "owner-2",
        "dcp-auth-project",
        PROJECT_READ_OPERATION,
    );
    let app = http_api::router_with_state(state);

    for endpoint in ["design-context-manifest", "design-context-diagnostics"] {
        let uri = format!("/runs/{run_id}/{endpoint}");
        for (token, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(cross_project_token.as_str()), StatusCode::FORBIDDEN),
            (Some(wrong_owner_token.as_str()), StatusCode::FORBIDDEN),
        ] {
            let mut request = Request::builder().uri(&uri);
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{endpoint}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header("authorization", format!("Bearer {read_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{endpoint}");
        let body = to_bytes(response.into_body(), 32_768).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("private-profile-copy"), "{endpoint}");
        assert!(!body.contains("/private/runtime-browser"), "{endpoint}");
        assert!(!body.contains("runtimeTokenMapping"), "{endpoint}");
        assert!(!body.contains("browserExecutable"), "{endpoint}");
        assert!(!body.contains("environment"), "{endpoint}");
        if endpoint == "design-context-diagnostics" {
            let diagnostics: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(diagnostics["fidelity"]["status"], json!("failed"));
            assert_eq!(
                diagnostics["fidelity"]["assertions"][0]["actualSummary"],
                json!("1 item(s)")
            );
            assert_eq!(
                diagnostics["fidelity"]["repairContext"]["targets"],
                json!(["project/src/styles/global.css"])
            );
            assert!(!body.contains("/workspace/"));
            assert!(!body.contains("runtimeTokenMapping"));
        }
    }

    std::fs::remove_dir_all(key_path.parent().unwrap()).unwrap();
}
