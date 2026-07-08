use anydesign_runtime::{
    config::SandboxBackendMode,
    http_api::{self, AppState},
    model_gateway::{MockModelClient, ModelResponse, ToolCall, ToolInputParseFailure},
    preview::{promote_preview, PromotionGateReport},
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, Brief, BriefStatus, ContentSource,
        ReviewFindingCategory, ReviewFindingSeverity, ReviewFindingStatus, SandboxBindingStatus,
        SandboxChannelProtocol,
    },
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tower::ServiceExt;

fn phase_a_contract_config() -> RuntimeConfig {
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config
}

fn website_brief() -> Brief {
    Brief {
        project_type: "website".to_string(),
        audience: "startup founders".to_string(),
        content_hierarchy: vec!["hero".to_string(), "features".to_string()],
        page_structure: json!([
            {
                "title": "Home",
                "purpose": "Explain the product",
                "keyContent": ["hero", "features"]
            }
        ]),
        visual_direction: "clear editorial product story".to_string(),
        recommended_template: "astro-website".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

#[tokio::test]
async fn root_route_returns_runtime_index_for_browser_access() {
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("AnyDesign Runtime"));
    assert!(body.contains("/health"));
    assert!(body.contains("zeron-real-website-1783303319260"));
    assert!(body.contains("zeron-real-docs-1783303417188"));
}

#[tokio::test]
async fn start_run_and_stream_events() {
    let store = RuntimeStore::new();
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-1",
        "run.complete",
        json!({ "status": "completed", "summary": "Brief ready" }),
    )])]);
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(model),
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

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();

    for _ in 0..20 {
        if store
            .get_run(&run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{run_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("run.started"));
    assert!(body.contains("agent.message"));
    assert!(body.contains("run.completed"));
    assert!(body.contains("id:"));
    assert!(body.contains(&format!("id: {run_id}/1")));
}

#[tokio::test]
async fn stream_events_exposes_tool_input_parse_failure_error_kind_without_raw_arguments() {
    let store = RuntimeStore::new();
    let model = MockModelClient::new(vec![
        ModelResponse::ToolInputParseFailed {
            parsed_calls: vec![],
            failures: vec![ToolInputParseFailure {
                tool_call_id: "tool-bad-json".to_string(),
                tool_name: "fs.write".to_string(),
                raw_len: 54,
                raw_sha256: "abc123".to_string(),
                ends_with_json_close: false,
                bracket_balance: 1,
                quote_closed: false,
                likely_truncated: true,
            }],
        },
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-1",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered after parse failure" }),
        )]),
    ]);
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(model),
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

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();

    for _ in 0..20 {
        if store
            .get_run(&run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{run_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("tool.failed"));
    assert!(body.contains("tool.input_json_parse_failed"));
    assert!(body.contains("tool_input_json_parse_failed"));
    assert!(body.contains("rawSha256"));
    assert!(!body.contains("rawArguments"));
    assert!(!body.contains("<html"));
    assert!(!body.contains("fs.write requires path"));
}

#[tokio::test]
async fn start_run_uses_configured_agent_model_for_real_provider_runs() {
    let store = RuntimeStore::new();
    let mut config = RuntimeConfig::from_env();
    config.agent_model = "deepseek-chat".to_string();
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Brief ready" }),
            ),
        ])])),
    });

    let response = app
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

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(run.model, "deepseek-chat");
}

#[tokio::test]
async fn start_run_input_context_binds_existing_sandbox() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-1".to_string(),
            "sandbox-claim-1".to_string(),
            "workspace-sandbox-claim-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
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
    let run_id = payload["runId"].as_str().unwrap();
    assert_eq!(
        store.get_run(run_id).await.unwrap().sandbox_id,
        Some(binding.id.clone())
    );
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
}

#[tokio::test]
async fn start_build_run_auto_provisions_sandbox_workspace_from_brief() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: phase_a_contract_config(),
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
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id
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
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    let binding_id = run.sandbox_id.as_deref().unwrap();
    let binding = store.get_sandbox_binding(binding_id).await.unwrap();
    assert_eq!(binding.project_id, "project-1");
    assert_eq!(binding.status, SandboxBindingStatus::Busy);
    assert_eq!(binding.warm_pool_name, "anydesign-astro-website-pool");
    assert_eq!(
        binding.workspace_pvc_name,
        format!("workspace-{}", binding.sandbox_claim_name)
    );
}

#[tokio::test]
async fn build_run_rejects_unconfirmed_brief_until_continue_confirms_it() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-unconfirmed-brief".to_string(),
            "sandbox-claim-unconfirmed-brief".to_string(),
            "workspace-sandbox-claim-unconfirmed-brief".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource::readable(
                "design-md",
                "design_md",
                "# Visual rules\nUse a polished product website style.",
            )],
        )
        .await;
    let brief_id = store
        .write_brief_draft(&brief_run.id, website_brief())
        .await
        .unwrap();
    assert!(store
        .content_sources_for_brief(&brief_id)
        .await
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
    store
        .update_run_status(&brief_run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let body = to_bytes(rejected.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("requires a confirmed brief"));

    let confirmed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", brief_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "确认这个 brief，可以开始生成 website" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);
    let body = to_bytes(confirmed.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "completed");
    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );
    let inherited_sources = store.content_sources_for_brief(&brief_id).await;
    assert!(inherited_sources
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
    let checkpoint_id = store
        .get_run(&brief_run.id)
        .await
        .unwrap()
        .checkpoint_id
        .unwrap();
    let checkpoint = store.get_checkpoint(&checkpoint_id).await.unwrap();
    assert_eq!(checkpoint.brief_version.as_deref(), Some(brief_id.as_str()));

    let accepted = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = to_bytes(accepted.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let build_run_id = payload["runId"].as_str().unwrap();
    let build_sources = store.content_sources(build_run_id).await;
    assert!(build_sources
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
}

#[tokio::test]
async fn sandbox_binding_is_exclusive_until_run_terminal() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-exclusive".to_string(),
            "sandbox-claim-exclusive".to_string(),
            "workspace-sandbox-claim-exclusive".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let first_run_id = payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let body = to_bytes(second.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("already in use by active run"));

    let cancelled = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{first_run_id}/cancel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Idle
    );
}

#[tokio::test]
async fn start_run_input_context_creates_child_run_with_findings() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent".to_string(),
            "sandbox-claim-parent".to_string(),
            "workspace-sandbox-claim-parent".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&parent.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &parent.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;
    let first_finding = store
        .record_review_finding(
            "project-1",
            &parent.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Build fails with TS2304",
            None,
            true,
        )
        .await
        .unwrap();
    let second_finding = store
        .record_review_finding(
            "project-1",
            &parent.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Hero section is blank",
            None,
            true,
        )
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id,
                            "findingIds": [first_finding.id.clone(), second_finding.id.clone()]
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
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(run.parent_run_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(run.sandbox_id.as_deref(), Some(binding.id.as_str()));
    assert_eq!(
        run.finding_ids,
        Some(vec![first_finding.id.clone(), second_finding.id.clone()])
    );
    assert_eq!(run.project_id, "project-1");
    assert_eq!(
        store
            .get_review_finding(&first_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Repairing
    );
    assert_eq!(
        store
            .get_review_finding(&second_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Repairing
    );
}

#[tokio::test]
async fn start_repair_run_requires_finding_ids() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent-no-finding".to_string(),
            "sandbox-claim-parent-no-finding".to_string(),
            "workspace-sandbox-claim-parent-no-finding".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&parent.id, &binding.id)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("repair run requires at least one finding"));
}

#[tokio::test]
async fn start_repair_run_can_target_review_child_finding() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-review-repair".to_string(),
            "sandbox-claim-review-repair".to_string(),
            "workspace-sandbox-claim-review-repair".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let build = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&build.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &build.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-review".to_string()),
            Some("file:///workspace/snapshots/review-candidate.tar".to_string()),
        )
        .await;
    let review = store
        .create_child_run(
            &build.id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{}", candidate.id)),
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-1",
            &review.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "First viewport is blank",
            None,
            true,
        )
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": review.id,
                            "findingIds": [finding.id.clone()]
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
    let repair = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(repair.parent_run_id.as_deref(), Some(review.id.as_str()));
    assert_eq!(repair.sandbox_id.as_deref(), Some(binding.id.as_str()));
    assert_eq!(repair.finding_ids, Some(vec![finding.id.clone()]));
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::Repairing
    );
}

#[tokio::test]
async fn start_run_rejects_unknown_parent_or_sandbox_binding() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let missing_parent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": "run-missing"
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_parent.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(missing_parent.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "parent run not found: run-missing");

    let missing_sandbox = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": "sandbox-binding-missing"
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_sandbox.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(missing_sandbox.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        "sandbox binding not found: sandbox-binding-missing"
    );

    let cross_project_binding = store
        .create_sandbox_binding(
            "project-2",
            "sandbox-project-2".to_string(),
            "sandbox-claim-project-2".to_string(),
            "workspace-sandbox-claim-project-2".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&cross_project_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let cross_project = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": cross_project_binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_project.status(), StatusCode::CONFLICT);
    let body = to_bytes(cross_project.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("sandbox binding project mismatch"));
}

#[tokio::test]
async fn start_run_rejects_sandbox_phase_without_workspace_binding() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("Build run requires a confirmed briefId"));
}

#[tokio::test]
async fn start_run_rejects_sandbox_phase_before_binding_ready() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-claiming".to_string(),
            "sandbox-claim-claiming".to_string(),
            "workspace-sandbox-claim-claiming".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let error = payload["error"].as_str().unwrap();
    assert!(error.contains("is not ready"));
    assert!(error.contains("wait_ready must complete"));
}

#[tokio::test]
async fn start_run_rejects_child_workspace_binding_mismatch() {
    let store = RuntimeStore::new();
    let parent_binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent".to_string(),
            "sandbox-claim-parent-mismatch".to_string(),
            "workspace-sandbox-claim-parent-mismatch".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&parent_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let child_binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-child".to_string(),
            "sandbox-claim-child-mismatch".to_string(),
            "workspace-sandbox-claim-child-mismatch".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&child_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&parent.id, &parent_binding.id)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id,
                            "sandboxBindingId": child_binding.id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("child run must use parent sandbox binding"));
}

#[tokio::test]
async fn stream_events_reconnect_uses_last_event_id_without_duplicates() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "first".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "second".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "third".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .header("last-event-id", format!("{}/2", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(!body.contains("first"));
    assert!(!body.contains("second"));
    assert!(body.contains("third"));
    assert!(body.contains(&format!("id: {}/3", run.id)));
}

#[tokio::test]
async fn stream_events_rejects_unknown_run() {
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/runs/run-missing/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "run not found: run-missing");
}

#[tokio::test]
async fn project_conversation_returns_user_visible_items_by_default() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .append_conversation_item(
            "project-1",
            Some(&run.id),
            "assistant_message",
            Some("assistant"),
            "Brief is ready.",
            None,
        )
        .await;
    store
        .append_conversation_item_with_visibility(
            "project-1",
            Some(&run.id),
            "tool_summary",
            Some("system"),
            "Debug-only tool detail",
            None,
            "debug",
        )
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["items"].as_array().unwrap().len(), 1);
    assert_eq!(payload["items"][0]["text"], "Brief is ready.");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation?includeDebug=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["items"].as_array().unwrap().len(), 2);
    assert!(payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["visibility"] == "debug"));
}

#[tokio::test]
async fn cancel_run_marks_terminal_cancelled() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "cancelled");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Cancelled
    );
}

#[tokio::test]
async fn cancel_run_cleans_staged_chunk_sessions_for_run() {
    let workspace_root = unique_temp_dir("http-cancel-staged-writes");
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
    let session_dir = workspace_root
        .join("project-1")
        .join("outputs/staged-writes/session-to-clean");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("manifest.json"),
        json!({
            "runId": run.id.clone(),
            "path": "/workspace/project/large.astro",
            "total": 1,
            "chunks": [0]
        })
        .to_string(),
    )
    .unwrap();
    fs::write(session_dir.join("chunk-0.txt"), "large page").unwrap();
    let mut config = RuntimeConfig::from_env();
    config.workspace_root = workspace_root;
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(!session_dir.exists());
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Cancelled
    );
}

#[tokio::test]
async fn cancel_run_rejects_terminal_run_without_reopening_it() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("already terminal"));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_records_user_message_and_resumes() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Resumed" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "userMessage": "Continue" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    for _ in 0..20 {
        if store
            .get_run(&run.id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| item.text == "Continue"));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_rejects_terminal_run_without_recording_message() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Reopen it" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(store.conversation_items("project-1").await.is_empty());
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_on_running_run_queues_message_without_reentrant_session() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not start" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Queue this edit after the current tool" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "running");
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Running
    );
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "user_message"
            && item.text == "Queue this edit after the current tool"));
    assert!(store.events(&run.id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. } if state == "running:continue_queued"
    )));
    assert!(store.continue_interrupt_requested(&run.id).await);
}

#[tokio::test]
async fn resolve_permission_allow_resumes_run() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Permission resolved" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "allow" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    for _ in 0..20 {
        if store
            .get_run(&run.id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "allow" && record.tool == "shell.run"));
}

#[tokio::test]
async fn resolve_permission_after_restart_resumes_same_run() {
    let checkpoint_dir = unique_temp_dir("http-permission-restart");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;

    let reloaded_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: reloaded_store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Permission resolved after restart" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "allow" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    for _ in 0..20 {
        if reloaded_store
            .get_run(&run.id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        reloaded_store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        reloaded_store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "allow"
    );
}

#[tokio::test]
async fn resolve_permission_ask_keeps_run_waiting_for_user_input() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "package.install")
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "decision": "ask", "updatedInput": { "question": "Which registry?" } })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "needs_user_input");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "ask"
    );
    assert!(store.events(&run.id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. }
            if state == "needs_user_input:permission_ask"
    )));
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "ask" && record.tool == "package.install"));
}

#[tokio::test]
async fn resolve_permission_deny_blocks_run_and_writes_conversation_item() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "deny" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "deny" && record.tool == "shell.run"));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| {
        item.kind == "permission_denied"
            && item.text.contains("shell.run")
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["tool"] == "shell.run")
    }));
}

#[tokio::test]
async fn resolve_permission_rejects_expired_terminal_run_permission() {
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
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "allow" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn preview_current_returns_promoted_version_contract() {
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
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            None,
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/preview/project-1/current")
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
        "http://preview.local/project-1/current"
    );
    assert_eq!(payload["status"], "promoted");
}

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
        config: RuntimeConfig::from_env(),
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
        config: RuntimeConfig::from_env(),
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
        config: RuntimeConfig::from_env(),
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
    let mut config = RuntimeConfig::from_env();
    config.enable_internal_promote_api = true;
    config.internal_admin_token = Some("test-token".to_string());
    let app = http_api::router_with_state(AppState {
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
    let mut config = RuntimeConfig::from_env();
    config.enable_internal_promote_api = true;
    config.internal_admin_token = Some("test-token".to_string());
    let app = http_api::router_with_state(AppState {
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

#[tokio::test]
async fn artifact_serving_supports_next_export_routes_and_assets() {
    let workspace = unique_temp_dir("artifact-next-export");
    let project_root = workspace.join("docs-project/project/out");
    fs::create_dir_all(project_root.join("_next/static/css")).unwrap();
    fs::create_dir_all(project_root.join("docs")).unwrap();
    fs::write(project_root.join("index.html"), "<main>Home</main>").unwrap();
    fs::write(
        project_root.join("docs.html"),
        r#"<link rel="stylesheet" href="/_next/static/css/app.css"><a href="/docs/runtime-flow">Runtime Flow</a><script>self.__next_f.push([1,"{\"href\":\"/docs\"}"])</script>"#,
    )
    .unwrap();
    fs::write(
        project_root.join("docs/runtime-flow.html"),
        "<main>Runtime Flow</main>",
    )
    .unwrap();
    fs::write(
        project_root.join("_next/static/css/app.css"),
        ".flex{display:flex}",
    )
    .unwrap();

    let mut config = RuntimeConfig::from_env();
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        config,
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let docs = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(docs.status(), StatusCode::OK);
    let body = String::from_utf8(to_bytes(docs.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(body.contains(r#"href="/artifacts/docs-project/current/_next/static/css/app.css""#));
    assert!(body.contains(r#"href="/artifacts/docs-project/current/docs/runtime-flow""#));
    assert!(body.contains(r#"\"href\":\"/artifacts/docs-project/current/docs\""#));

    let root = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(root.status(), StatusCode::OK);

    let nested_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/docs/runtime-flow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nested_page.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(nested_page.into_body(), 4096)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("Runtime Flow"));

    let css = app
        .oneshot(
            Request::builder()
                .uri("/artifacts/docs-project/current/_next/static/css/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(
        css.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
}

#[tokio::test]
async fn artifact_serving_supports_phase_a_global_workspace_root() {
    let workspace = unique_temp_dir("artifact-phase-a-global");
    let project_root = workspace.join("project/dist");
    fs::create_dir_all(project_root.join("_astro")).unwrap();
    fs::write(
        project_root.join("index.html"),
        r#"<link rel="stylesheet" href="/_astro/app.css"><main>Phase A Artifact</main>"#,
    )
    .unwrap();
    fs::write(project_root.join("_astro/app.css"), "body{color:white}").unwrap();

    let mut config = RuntimeConfig::from_env();
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        config,
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/artifacts/phase-a-project/current/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("Phase A Artifact"));
    assert!(body.contains(r#"href="/artifacts/phase-a-project/current/_astro/app.css""#));

    let css = app
        .oneshot(
            Request::builder()
                .uri("/artifacts/phase-a-project/current/_astro/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(
        css.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}-{}",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
