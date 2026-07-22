use super::*;
use anydesign_runtime::{
    design_context::{
        compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
    },
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::DesignContextEnforcementBinding,
};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};

#[tokio::test]
async fn generation_context_prometheus_export_is_admin_only_and_low_cardinality() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "metrics-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "resource:deepseek-v4-pro".to_string(),
            vec![],
        )
        .await;
    store
        .mark_run_generation_context_fallback(&run.id, None, "not_required", None)
        .await
        .unwrap();
    store
        .append_event(AgentEvent::MetricRecorded {
            run_id: run.id.clone(),
            name: "edit_plan.replacement_required".to_string(),
            value: 1,
            metadata: Some(json!({ "errorKind": "edit.plan_scope_violation" })),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(AgentEvent::MetricRecorded {
            run_id: run.id.clone(),
            name: "efficiency.time_to_iframe_applied_ms".to_string(),
            value: 750,
            metadata: Some(json!({ "executionProfile": "warm_hmr" })),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("generation-metrics-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/metrics/generation-context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let allowed = app
        .oneshot(
            Request::builder()
                .uri("/internal/metrics/generation-context")
                .header("authorization", "Bearer generation-metrics-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        allowed
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let body = String::from_utf8(
        to_bytes(allowed.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(
        "generation_context_compile_total{status=\"fallback_legacy_protocol\",phase=\"build\",template=\"unknown\"} 1"
    ));
    assert!(
        body.contains("edit_impact_plan_replaced_total{reason=\"edit.plan_scope_violation\"} 1")
    );
    assert!(body.contains(
        "draft_hmr_iframe_applied_seconds_count{phase=\"build\",template=\"unknown\"} 1"
    ));
    assert!(!body.contains("metrics-project"));
    assert!(!body.contains(&run.id));
    assert!(!body.contains("deepseek-v4-pro"));
}

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
        "workspaceNamespace": "ws-one"
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
        .clone()
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

    let namespace_change = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/projects/project-access-1/access")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-project-access-token")
                .body(Body::from(
                    json!({
                        "ownerPrincipalId": "principal-owner-1",
                        "workspaceNamespace": "ws-two"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(namespace_change.status(), StatusCode::CONFLICT);

    drop(store);
    let restarted = RuntimeStore::with_checkpoint_dir(storage);
    let record = restarted
        .get_project_access("project-access-1")
        .await
        .unwrap();
    assert_eq!(record.owner_principal_id, "principal-owner-1");
    assert_eq!(record.workspace_namespace, "ws-one");
}

#[tokio::test]
async fn design_context_enforcement_policy_requires_admin_uses_cas_and_survives_restart() {
    let storage = unique_temp_dir("design-context-enforcement-policy");
    let store = RuntimeStore::with_checkpoint_dir(storage.clone());
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("admin-design-context-policy-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
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
                    design_profile_request("project-policy-1", vec!["next-app"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = to_bytes(created.into_body(), 32_768).await.unwrap();
    let created: Value = serde_json::from_slice(&body).unwrap();
    let profile_id = created["designProfile"]["id"].as_str().unwrap();
    let profile_version = created["designProfile"]["version"].as_u64().unwrap();
    let policy_uri = "/internal/projects/project-policy-1/design-context-enforcement";
    let create_body = json!({
        "designProfileId": profile_id,
        "designProfileVersion": profile_version,
        "enabled": true,
        "expectedRevision": 0,
        "updatedBy": "release-operator-1"
    })
    .to_string();

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(policy_uri)
                .header("content-type", "application/json")
                .body(Body::from(create_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let created_policy = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(policy_uri)
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-design-context-policy-token")
                .body(Body::from(create_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created_policy.status(), StatusCode::OK);
    let body = to_bytes(created_policy.into_body(), 32_768).await.unwrap();
    let created_policy: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(created_policy["policy"]["revision"], 1);
    assert_eq!(created_policy["policy"]["enabled"], true);

    let stale = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(policy_uri)
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-design-context-policy-token")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let disabled = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(policy_uri)
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-design-context-policy-token")
                .body(Body::from(
                    json!({
                        "designProfileId": profile_id,
                        "designProfileVersion": profile_version,
                        "enabled": false,
                        "expectedRevision": 1,
                        "updatedBy": "release-operator-2"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::OK);

    drop(store);
    let restarted = RuntimeStore::with_checkpoint_dir(storage);
    let policy = restarted
        .get_design_context_enforcement_policy(
            "project-policy-1",
            profile_id,
            profile_version as u32,
        )
        .await
        .unwrap();
    assert!(!policy.enabled);
    assert_eq!(policy.revision, 2);
    assert_eq!(policy.updated_by, "release-operator-2");
}

#[tokio::test]
async fn design_context_canary_metrics_export_is_admin_only_and_aggregates_frozen_cohort() {
    let store = RuntimeStore::new();
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("admin-canary-metrics-token".to_string());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let mut profile_request = design_profile_request("project-canary-1", vec!["next-app"]);
    profile_request["profile"]["websiteContext"] = json!({ "enforcementMode": "enforced" });
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(profile_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 32_768).await.unwrap()).unwrap();
    let profile_id = created["designProfile"]["id"].as_str().unwrap();
    let profile = store.get_design_profile(profile_id).await.unwrap();
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("next-app").unwrap())
        .unwrap();
    let now = Utc::now();
    let baseline_started_at = now - ChronoDuration::hours(4);
    let baseline_ended_at = now - ChronoDuration::hours(3);
    let observation_started_at = now - ChronoDuration::hours(2);
    let observation_ended_at = now;
    let mut enforced_run_id = None;

    for (mode, policy_revision, event_at, version_id) in [
        (
            "observe",
            1_u64,
            baseline_started_at + ChronoDuration::minutes(30),
            "version-baseline",
        ),
        (
            "enforced",
            2_u64,
            observation_started_at + ChronoDuration::minutes(30),
            "version-enforced",
        ),
        (
            "enforced",
            3_u64,
            observation_started_at + ChronoDuration::minutes(45),
            "version-wrong-policy-revision",
        ),
    ] {
        let run = store
            .create_run(
                "project-canary-1".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                Vec::new(),
            )
            .await;
        store
            .attach_run_effective_design_profile(
                &run.id,
                &profile,
                Some("website"),
                Some("next-app"),
            )
            .await
            .unwrap();
        let dcp = compile_website_design_context(
            &profile.effective_for("website", "next-app").unwrap(),
            &website_brief(),
            &template,
            &DesignContextCompileOptions {
                enforcement_enabled: mode == "enforced",
                ..DesignContextCompileOptions::default()
            },
        )
        .unwrap();
        store
            .attach_run_design_context_with_enforcement_binding(
                &run.id,
                &dcp,
                &VerifierRegistry::discover(),
                Some(DesignContextEnforcementBinding {
                    source: "persistent".to_string(),
                    enabled: mode == "enforced",
                    policy_revision: Some(policy_revision),
                    policy_updated_by: Some("operator-1".to_string()),
                }),
            )
            .await
            .unwrap();
        store
            .set_run_output_version(&run.id, version_id.to_string())
            .await
            .unwrap();
        store
            .append_event(AgentEvent::PreviewUpdated {
                run_id: run.id.clone(),
                url: format!("http://preview/{version_id}"),
                version_id: version_id.to_string(),
                screenshot_id: Some(format!("screenshot-{version_id}")),
                timestamp: event_at,
            })
            .await
            .unwrap();
        if mode == "enforced" {
            store
                .append_event(AgentEvent::MetricRecorded {
                    run_id: run.id.clone(),
                    name: "design_context_verifier_unavailable_total".to_string(),
                    value: 1,
                    metadata: Some(json!({
                        "mode": "enforced",
                        "surface": "website",
                        "reason": "worker_unavailable"
                    })),
                    timestamp: event_at,
                })
                .await
                .unwrap();
            if policy_revision == 2 {
                enforced_run_id = Some(run.id.clone());
                store
                    .append_event(AgentEvent::MetricRecorded {
                        run_id: run.id.clone(),
                        name: format!("custom_high_cardinality_metric_{}", run.id),
                        value: 1,
                        metadata: Some(json!({
                            "mode": "enforced",
                            "surface": "website",
                            "untrustedLabel": "must-not-be-exported"
                        })),
                        timestamp: event_at,
                    })
                    .await
                    .unwrap();
            }
        }
        store
            .update_run_status(&run.id, AgentRunStatus::Completed)
            .await
            .unwrap();
    }

    let query = format!(
        "/internal/projects/project-canary-1/design-context-canary-metrics?designProfileId={profile_id}&designProfileVersion=1&observePolicyRevision=1&policyRevision=2&baselineStartedAt={}&baselineEndedAt={}&observationStartedAt={}&observationEndedAt={}&conclusionRecordedBy=operator-1",
        baseline_started_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        baseline_ended_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        observation_started_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        observation_ended_at.to_rfc3339_opts(SecondsFormat::Millis, true),
    );
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&query)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let allowed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&query)
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-canary-metrics-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(allowed.into_body(), 128 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        body["schemaVersion"],
        "design-context-canary-operational-export@1"
    );
    assert_eq!(body["source"]["kind"], "runtime-durable-store");
    assert_eq!(body["publish"]["baselinePublishCount"], 1);
    assert_eq!(body["publish"]["enforcedPublishCount"], 1);
    assert_eq!(body["publish"]["samples"].as_array().unwrap().len(), 2);
    assert_eq!(body["metrics"]["verifierUnavailableCount"], 1);
    assert_eq!(body["metrics"]["verifierRuntimeLostCount"], 0);
    assert_eq!(body["alertsTriggered"], true);
    assert!(!body.to_string().contains("custom_high_cardinality_metric"));
    assert!(!body.to_string().contains("must-not-be-exported"));
    assert_eq!(
        body["alerts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|alert| alert["code"] == "verifier_unavailable")
            .unwrap()["triggered"],
        true
    );

    store
        .append_event(AgentEvent::MetricRecorded {
            run_id: enforced_run_id.unwrap(),
            name: "design_context_required_read_block_total".to_string(),
            value: 1,
            metadata: Some(json!({
                "mode": "observe",
                "surface": "website",
                "reason": "read_required"
            })),
            timestamp: observation_started_at + ChronoDuration::minutes(40),
        })
        .await
        .unwrap();
    let inconsistent = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&query)
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "admin-canary-metrics-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inconsistent.status(), StatusCode::CONFLICT);
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
    config.database_url = "postgres://runtime@postgres/runtime".to_string();
    config.object_storage_url = "s3://runtime-artifacts/test".to_string();
    config.object_storage_endpoint = "https://object-store.example".to_string();
    config.object_storage_access_key = Some("test-access-key".to_string());
    config.object_storage_secret_key = Some("test-secret-key".to_string());
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
            "ws-owned".to_string(),
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
    assert_eq!(drifted.status(), StatusCode::OK);
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
                        "template": "next-app",
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
                        "template": "next-app",
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
