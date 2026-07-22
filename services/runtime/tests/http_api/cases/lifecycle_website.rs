use super::*;
use anydesign_runtime::types::DesignProfile;
use chrono::Utc;

fn dcp_observe_profile(project_id: &str) -> DesignProfile {
    let now = Utc::now();
    DesignProfile {
        id: "http-lifecycle-dcp-profile".to_string(),
        schema_version: "design-profile@1".to_string(),
        name: "HTTP Lifecycle DCP Profile".to_string(),
        status: "active".to_string(),
        version: 1,
        scope: json!({ "projectId": project_id }),
        source: json!({ "kind": "manual" }),
        product: json!({ "name": "HTTP DCP", "category": "test fixture" }),
        brand: json!({}),
        visual: json!({ "direction": "quiet technical confidence" }),
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
            "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)"
        }),
        extended_token_mapping: json!({}),
        components: json!({}),
        website_context: json!({ "enforcementMode": "observe" }),
        content: json!({}),
        accessibility: json!({}),
        technical: json!({ "allowedTemplates": ["next-app"] }),
        governance: json!({ "conflictBehavior": "ask" }),
        signature_rules: Vec::new(),
        overrides: json!({}),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn website_dcp_flag_matrix_is_fail_closed_and_backward_compatible() {
    let store = RuntimeStore::new();
    let cases = [
        ("off-off", false, false, false),
        ("on-off", true, false, true),
        ("on-on-empty-allowlist", true, true, true),
    ];

    for (suffix, master_enabled, enforcement_enabled, expects_dcp) in cases {
        let project_id = format!("project-dcp-flag-matrix-{suffix}");
        let brief_run = store
            .create_run(
                project_id.clone(),
                AgentPhase::Brief,
                "brief".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let brief_id = store
            .write_brief(&brief_run.id, website_brief())
            .await
            .unwrap();
        store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
        let mut profile = dcp_observe_profile(&project_id);
        profile.id = format!("profile-dcp-flag-matrix-{suffix}");
        profile.website_context = json!({
            "enforcementMode": "enforced",
            "craftPacks": ["accessibility-baseline", "responsive-layout"]
        });
        let profile = store.create_design_profile(profile).await.unwrap();
        store
            .bind_project_design_profile(&project_id, &profile.id)
            .await
            .unwrap();

        let mut config = phase_a_contract_config();
        config.enable_design_context_package = master_enabled;
        config.enable_design_context_enforcement = enforcement_enabled;
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
                    .uri("/runs")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "projectId": project_id,
                            "phase": "build",
                            "agentProfile": "build",
                            "inputContext": { "briefId": brief_id }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "flag matrix case {suffix}"
        );
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let run = store
            .get_run(payload["runId"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            run.design_context_manifest.is_some(),
            expects_dcp,
            "flag matrix case {suffix}"
        );
        if expects_dcp {
            assert_eq!(
                run.design_context_effective_compatibility_mode.as_deref(),
                Some("observe"),
                "only an exact enforcement allowlist match may turn on required gates",
            );
        }
    }

    let mut invalid = phase_a_contract_config();
    invalid.enable_design_context_package = false;
    invalid.enable_design_context_enforcement = true;
    assert!(invalid.validate_startup().unwrap_err().contains(
        "RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1 requires RUNTIME_DESIGN_CONTEXT_PACKAGE_V1"
    ));
}

#[tokio::test]
async fn enforced_dcp_unavailable_verifier_blocks_and_records_low_cardinality_metric() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-verifier-unavailable".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_observe_profile("project-dcp-verifier-unavailable");
    profile.id = "profile-dcp-verifier-unavailable".to_string();
    profile.website_context = json!({
        "enforcementMode": "enforced",
        "craftPacks": ["accessibility-baseline", "responsive-layout"]
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-verifier-unavailable", &profile.id)
        .await
        .unwrap();
    let mut config = phase_a_contract_config();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_enforcement_allowlist_json = Some(
        json!([{
            "projectId": "project-dcp-verifier-unavailable",
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-verifier-unavailable",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["status"], "blocked");
    let run_id = payload["runId"].as_str().unwrap();
    assert_eq!(
        store.get_run(run_id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert!(store.events(run_id).await.into_iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded { name, metadata: Some(metadata), .. }
            if name == "design_context_verifier_unavailable_total"
                && metadata["mode"] == "enforced"
                && metadata["missingVerifierCount"].as_u64().is_some_and(|count| count > 0)
    )));
}
