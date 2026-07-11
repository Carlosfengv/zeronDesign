use anydesign_runtime::{
    config::{RuntimePolicyProfile, SandboxBackendMode},
    http_api::{self, AppState},
    model_gateway::{
        MockModelClient, ModelResponse, OpenAiCompatibleModelClient, ToolCall,
        ToolInputParseFailure,
    },
    preview::{promote_preview, PromotionGateReport},
    types::{
        sha256_hex, AgentEvent, AgentPhase, AgentRunStatus, Brief, BriefStatus, ContentSource,
        ReviewFindingCategory, ReviewFindingSeverity, ReviewFindingStatus, SandboxBindingStatus,
        SandboxChannelProtocol,
    },
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    task::JoinHandle,
    time::{timeout, Duration},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tower::ServiceExt;

static SANDBOX_CHANNEL_ENV_LOCK: Mutex<()> = Mutex::new(());
const REAL_PROVIDER_STAGE_TIMEOUT_SECS: u64 = 420;

struct SandboxChannelEnvOverride;

impl SandboxChannelEnvOverride {
    fn set(host: String, port: u16) -> Self {
        unsafe {
            std::env::set_var("SANDBOX_CHANNEL_HOST_OVERRIDE", host);
            std::env::set_var("SANDBOX_CHANNEL_PORT_OVERRIDE", port.to_string());
        }
        Self
    }
}

struct SandboxPreviewEnvOverride;

impl SandboxPreviewEnvOverride {
    fn set(host: &str, port: u16) -> Self {
        unsafe {
            std::env::set_var("SANDBOX_PREVIEW_HOST_OVERRIDE", host);
            std::env::set_var("SANDBOX_PREVIEW_PORT_OVERRIDE", port.to_string());
        }
        Self
    }
}

impl Drop for SandboxPreviewEnvOverride {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("SANDBOX_PREVIEW_HOST_OVERRIDE");
            std::env::remove_var("SANDBOX_PREVIEW_PORT_OVERRIDE");
        }
    }
}

impl Drop for SandboxChannelEnvOverride {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("SANDBOX_CHANNEL_HOST_OVERRIDE");
            std::env::remove_var("SANDBOX_CHANNEL_PORT_OVERRIDE");
        }
    }
}

fn phase_a_contract_config() -> RuntimeConfig {
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.runtime_storage_dir = unique_temp_dir("http-runtime-storage");
    config
}

#[tokio::test]
async fn version_reports_runtime_build_identity() {
    let mut config = phase_a_contract_config();
    config.repository_commit = "abc123def456".to_string();
    config.repository_dirty = true;
    config.runtime_image_ref = Some("anydesign/runtime:abc123def456-dirty".to_string());
    let response = http_api::router(config)
        .oneshot(
            Request::builder()
                .uri("/version")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["service"], "anydesign-runtime");
    assert_eq!(payload["repositoryCommit"], "abc123def456");
    assert_eq!(payload["repositoryDirty"], true);
    assert_eq!(payload["imageRef"], "anydesign/runtime:abc123def456-dirty");
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

fn docs_brief() -> Brief {
    Brief {
        project_type: "docs".to_string(),
        audience: "developer operators".to_string(),
        content_hierarchy: vec!["overview".to_string(), "lifecycle".to_string()],
        page_structure: json!([
            {
                "title": "Overview",
                "level": 1,
                "content": "Explain the runtime lifecycle"
            }
        ]),
        visual_direction: "clear technical documentation".to_string(),
        recommended_template: "fumadocs-docs".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

fn runtime_style_contract_json(
    token_file: &str,
    global_css_file: &str,
    component_root: &str,
) -> Value {
    json!({
        "version": "runtime-style-contract@p2",
        "tokenFile": token_file,
        "globalCssFile": global_css_file,
        "componentRoot": component_root,
        "tailwind": {
            "version": "4",
            "entryImport": "@import \"tailwindcss\"",
            "themeSource": "css-variables"
        },
        "tokens": {
            "color.primary": "--runtime-primary"
        }
    })
}

fn design_profile_request(project_id: &str, allowed_templates: Vec<&str>) -> Value {
    design_profile_request_for_scope(
        Some(project_id),
        json!({ "projectId": project_id }),
        allowed_templates,
    )
}

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
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

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
    assert_eq!(unauthorized_import.status(), StatusCode::UNAUTHORIZED);

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
        design_profile_request("project-import", vec!["astro-website"])["profile"].clone();
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
                    "/design-profiles/{profile_id}/versions/3/fidelity-report?surface=website&template=astro-website"
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

fn design_profile_request_for_scope(
    project_id: Option<&str>,
    scope: Value,
    allowed_templates: Vec<&str>,
) -> Value {
    let profile = json!({
        "status": "active",
        "scope": scope,
        "source": { "kind": "manual" },
        "product": {
            "name": "AnyDesign Runtime",
            "category": "agent harness",
            "audience": ["internal builders"],
            "primaryUseCases": ["generate websites", "edit docs"],
            "productQualities": ["reliable", "inspectable"]
        },
        "brand": {
            "voice": {
                "tone": ["clear", "precise"],
                "sentenceStyle": "technical",
                "vocabulary": { "prefer": ["runtime", "evidence"], "avoid": ["magic"] },
                "writingRules": ["Use concrete status text."]
            },
            "messaging": {
                "headlineStyle": "specific",
                "bodyStyle": "concise",
                "ctaStyle": "verb first",
                "proofStyle": "evidence based",
                "forbiddenClaims": ["guaranteed"]
            }
        },
        "visual": {
            "direction": "quiet operational interface",
            "principles": ["scan friendly"],
            "moodKeywords": ["calm"],
            "avoidKeywords": ["flashy"],
            "composition": {},
            "imagery": {},
            "motion": {}
        },
        "tokens": {
            "color": {},
            "typography": {},
            "radius": {},
            "shadow": {},
            "spacing": {}
        },
        "runtimeTokenMapping": {
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
        },
        "components": {
            "primitives": {
                "button": { "intent": "clear action", "usage": ["primary actions"], "avoid": ["overuse"] },
                "input": { "intent": "precise entry", "usage": ["forms"], "avoid": ["placeholder-only labels"] },
                "card": { "intent": "group repeated items", "usage": ["lists"], "avoid": ["nested cards"] },
                "badge": { "intent": "show status", "usage": ["statuses"], "avoid": ["decorative noise"] }
            }
        },
        "content": {},
        "accessibility": {},
        "technical": {
            "allowedTemplates": allowed_templates,
            "preferredTemplates": { "website": "astro-website", "docs": "fumadocs-docs" },
            "cssStrategy": "runtime-style-contract",
            "dependencyPolicy": {},
            "filePolicy": {
                "designProfilePath": "/workspace/inputs/design-profile.json",
                "designMarkdownPath": "/workspace/inputs/design.md",
                "styleContractPath": "/workspace/state/style-contract.json"
            }
        },
        "governance": { "conflictBehavior": "ask" }
    });
    let mut request = json!({
        "name": "Harness Calm Ops",
        "profile": profile
    });
    if let Some(project_id) = project_id {
        request["projectId"] = json!(project_id);
    }
    request
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
async fn design_profile_api_create_bind_and_resolve_for_runs() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                    design_profile_request("project-1", vec!["astro-website"]).to_string(),
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

    let update_profile =
        design_profile_request("project-1", vec!["astro-website"])["profile"].clone();
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

#[tokio::test]
async fn required_unsupported_extended_token_blocks_build_with_capability_state() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let mut create_request = design_profile_request("project-capability", vec!["astro-website"]);
    create_request["profile"]["schemaVersion"] = json!("design-profile@2");
    create_request["profile"]["extendedTokenMapping"] =
        json!({ "imagery.unsupportedShader": "required" });
    create_request["profile"]["signatureRules"] = json!([{
        "id": "unsupported-required-token",
        "category": "imagery",
        "statement": "The unsupported shader token is required.",
        "priority": "required",
        "appliesTo": ["website"],
        "verification": {
            "kind": "token",
            "token": "imagery.unsupportedShader",
            "expected": "required",
            "comparator": { "kind": "exact" }
        }
    }]);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(create_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 32_768).await.unwrap();
    let created_json: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_json["designProfile"]["id"].as_str().unwrap();

    let brief_run = store
        .create_run(
            "project-capability".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "builders".to_string(),
                content_hierarchy: vec!["Home".to_string()],
                page_structure: json!([]),
                visual_direction: "specific".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store
        .update_run_status(&brief_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let started = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-capability",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id,
                            "designProfileId": profile_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let started_status = started.status();
    let started_body = to_bytes(started.into_body(), 16_384).await.unwrap();
    assert_eq!(
        started_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&started_body)
    );
    let started_json: Value = serde_json::from_slice(&started_body).unwrap();
    assert_eq!(started_json["status"], "needs_user_input");
    let run_id = started_json["runId"].as_str().unwrap();
    let run = store.get_run(run_id).await.unwrap();
    assert_eq!(
        run.design_profile_blocking_capability_rule_ids,
        vec!["unsupported-required-token"]
    );
    assert!(store.events(run_id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. }
            if state == "needs_user_input:design_profile_capability_gap"
    )));
}

#[tokio::test]
async fn design_profile_rejects_multiple_active_profiles_for_same_project() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-unique", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = to_bytes(first.into_body(), 16384).await.unwrap();
    let first_payload: Value = serde_json::from_slice(&first_body).unwrap();
    let first_profile_id = first_payload["designProfile"]["id"].as_str().unwrap();

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Second active profile",
                        "profile": design_profile_request("project-unique", vec!["astro-website"])["profile"].clone()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let second_body = to_bytes(second.into_body(), 4096).await.unwrap();
    let second_payload: Value = serde_json::from_slice(&second_body).unwrap();
    assert!(second_payload["error"]
        .as_str()
        .unwrap()
        .contains("already has active design profile"));

    let archived = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/design-profiles/{first_profile_id}/archive"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);

    let second_after_archive = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Second active profile",
                        "profile": design_profile_request("project-unique", vec!["astro-website"])["profile"].clone()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_after_archive.status(), StatusCode::OK);
}

#[tokio::test]
async fn start_run_resolves_design_profile_by_workspace_then_project_precedence() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let workspace_created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request_for_scope(
                        None,
                        json!({ "workspaceId": "workspace-1" }),
                        vec!["astro-website"],
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let workspace_body = to_bytes(workspace_created.into_body(), 16384)
        .await
        .unwrap();
    let workspace_payload: Value = serde_json::from_slice(&workspace_body).unwrap();
    let workspace_profile_id = workspace_payload["designProfile"]["id"].as_str().unwrap();

    let project_created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-1", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let project_body = to_bytes(project_created.into_body(), 16384).await.unwrap();
    let project_payload: Value = serde_json::from_slice(&project_body).unwrap();
    let project_profile_id = project_payload["designProfile"]["id"].as_str().unwrap();

    let workspace_run = app
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
                            "workspaceId": "workspace-1",
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
    assert_eq!(workspace_run.status(), StatusCode::OK);
    let workspace_run_body = to_bytes(workspace_run.into_body(), 4096).await.unwrap();
    let workspace_run_payload: Value = serde_json::from_slice(&workspace_run_body).unwrap();
    let workspace_run_id = workspace_run_payload["runId"].as_str().unwrap();
    assert_eq!(
        store
            .get_run(workspace_run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(workspace_profile_id)
    );
    store
        .update_run_status(workspace_run_id, AgentRunStatus::Completed)
        .await
        .unwrap();

    let bound = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-1/design-profile")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "designProfileId": project_profile_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bound.status(), StatusCode::OK);

    let project_run = app
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
                            "workspaceId": "workspace-1",
                            "contentSources": [
                                ContentSource::readable("source-2", "prompt", "Make another website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(project_run.status(), StatusCode::OK);
    let project_run_body = to_bytes(project_run.into_body(), 4096).await.unwrap();
    let project_run_payload: Value = serde_json::from_slice(&project_run_body).unwrap();
    let project_run_id = project_run_payload["runId"].as_str().unwrap();
    assert_eq!(
        store
            .get_run(project_run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(project_profile_id)
    );
}

#[tokio::test]
async fn start_run_resolves_design_profile_by_organization_fallback() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                    design_profile_request_for_scope(
                        None,
                        json!({ "organizationId": "org-1" }),
                        vec!["astro-website"],
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let created_body = to_bytes(created.into_body(), 16384).await.unwrap();
    let created_payload: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_payload["designProfile"]["id"].as_str().unwrap();

    let started = app
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
                            "organizationId": "org-1",
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
    assert_eq!(
        store
            .get_run(run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(profile_id)
    );
}

#[tokio::test]
async fn start_run_with_missing_explicit_design_profile_returns_not_found() {
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: RuntimeStore::new(),
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
                        "projectId": "project-1",
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "designProfileId": "design-profile-missing",
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn start_run_design_profile_template_conflict_enters_needs_user_input() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    store
        .update_run_status(&brief_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                    design_profile_request("project-1", vec!["fumadocs-docs"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(created.into_body(), 16384).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let profile_id = payload["designProfile"]["id"].as_str().unwrap();

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
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id,
                            "designProfileId": profile_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response_body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&response_body)
    );
    let response_payload: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(response_payload["status"], "needs_user_input");
    let run_id = response_payload["runId"].as_str().unwrap();
    let run = store.get_run(run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::NeedsUserInput);
    assert_eq!(run.design_profile_id.as_deref(), Some(profile_id));
    assert!(store.events(run_id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(
            |item| item.kind == "approval_request" && item.text.contains("DesignProfile conflict")
        ));
}

#[tokio::test]
async fn continue_edit_run_design_profile_conflict_enters_needs_user_input() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
                    design_profile_request("project-1", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(created.into_body(), 16384).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let profile_id = payload["designProfile"]["id"].as_str().unwrap();
    let profile = store.get_design_profile(profile_id).await.unwrap();
    let edit_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
            None,
            Some("version-1".to_string()),
        )
        .await;
    let edit_run = store
        .attach_run_design_profile(&edit_run.id, &profile)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", edit_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Make the page flashy and loud." }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), 4096).await.unwrap();
    let response_payload: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(response_payload["status"], "needs_user_input");
    let run = store.get_run(&edit_run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::NeedsUserInput);
    assert!(store.events(&edit_run.id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "approval_request"
            && item.text.contains("visual keyword \"flashy\"")));

    let override_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", edit_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "临时覆盖 DesignProfile，继续执行" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(override_response.status(), StatusCode::OK);
    let override_body = to_bytes(override_response.into_body(), 4096).await.unwrap();
    let override_payload: Value = serde_json::from_slice(&override_body).unwrap();
    assert_eq!(override_payload["status"], "running");
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "design_profile_override"
            && item.text.contains("override accepted")
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["designProfileId"] == profile_id)));
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

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&body)
    );
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
async fn start_run_rejects_empty_contract_identifiers() {
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: RuntimeStore::new(),
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
                        "projectId": " ",
                        "phase": "brief",
                        "agentProfile": "brief"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "projectId must not be empty");
}

#[tokio::test]
async fn continue_run_rejects_empty_user_message() {
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
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "userMessage": " " }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "userMessage must not be empty");
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

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&body)
    );
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

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    if status != StatusCode::OK {
        panic!(
            "unexpected start run response {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
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
    store
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            binding.sandbox_name.clone(),
            Some(binding.sandbox_name.clone()),
            Some("sandbox-uid-1".to_string()),
            Some("pod-uid-1".to_string()),
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
        config: phase_a_contract_config(),
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
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "second".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "third".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
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
async fn stream_events_replays_then_fans_out_without_duplicates() {
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
    for text in ["first", "second", "third"] {
        store
            .append_event(AgentEvent::AgentMessage {
                run_id: run.id.clone(),
                text: text.to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
    }
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: store.clone(),
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
    let mut body = response.into_body().into_data_stream();

    let replay = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected replay event")
        .expect("expected replay frame")
        .expect("replay frame should be valid");
    let replay = String::from_utf8(replay.to_vec()).unwrap();
    assert!(replay.contains("third"));
    assert!(replay.contains(&format!("id: {}/3", run.id)));

    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "fourth".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let live = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected live event")
        .expect("expected live frame")
        .expect("live frame should be valid");
    let live = String::from_utf8(live.to_vec()).unwrap();
    assert!(live.contains("fourth"));
    assert!(live.contains(&format!("id: {}/4", run.id)));

    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let terminal = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected terminal event")
        .expect("expected terminal frame")
        .expect("terminal frame should be valid");
    let terminal = String::from_utf8(terminal.to_vec()).unwrap();
    assert!(terminal.contains("run.completed"));
    assert!(terminal.contains(&format!("id: {}/5", run.id)));

    let end = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("terminal stream should close");
    assert!(end.is_none());
}

#[tokio::test]
async fn stream_events_recovered_active_run_uses_next_persisted_sequence() {
    let checkpoint_dir = unique_temp_dir("http-sse-recovered-sequence");
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
    for text in ["first", "second", "third"] {
        store
            .append_event(AgentEvent::AgentMessage {
                run_id: run.id.clone(),
                text: text.to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
    }

    let recovered_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    recovered_store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "fourth".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
        store: recovered_store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .header("last-event-id", format!("{}/3", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let frame = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected recovered replay frame")
        .expect("expected body frame")
        .expect("frame should be valid");
    let frame = String::from_utf8(frame.to_vec()).unwrap();
    assert!(frame.contains("fourth"));
    assert!(frame.contains(&format!("id: {}/4", run.id)));
}

#[tokio::test]
async fn stream_events_terminal_status_without_terminal_event_waits_for_terminal_event() {
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
                .uri(format!("/runs/{}/events", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(
        timeout(Duration::from_millis(100), body.next())
            .await
            .is_err(),
        "terminal status without run.completed should not close the stream before terminal event"
    );

    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let terminal = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected terminal event")
        .expect("expected body frame")
        .expect("frame should be valid");
    let terminal = String::from_utf8(terminal.to_vec()).unwrap();
    assert!(terminal.contains("run.completed"));
    assert!(terminal.contains(&format!("id: {}/1", run.id)));
}

#[tokio::test]
async fn append_event_does_not_broadcast_when_run_log_append_fails() {
    let checkpoint_dir = unique_temp_dir("http-sse-bad-checkpoints");
    let storage_parent = unique_temp_dir("http-sse-bad-run-log-parent");
    let run_log_file = storage_parent.join("not-a-directory");
    fs::write(&run_log_file, "occupied").unwrap();
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_file);
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
                .uri(format!("/runs/{}/events", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let result = store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id,
            text: "should not broadcast".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    assert!(result.is_err());
    let maybe_frame = timeout(Duration::from_millis(100), body.next()).await;
    assert!(
        maybe_frame.is_err() || maybe_frame.unwrap().is_none(),
        "SSE stream should not receive an event that failed durable append"
    );
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
    let mut config = phase_a_contract_config();
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
async fn project_runtime_state_exposes_editable_lifecycle_metadata() {
    let workspace = unique_temp_dir("runtime-state-lifecycle");
    let project_workspace = workspace.join("project-1");
    fs::create_dir_all(project_workspace.join("state")).unwrap();
    fs::create_dir_all(project_workspace.join("outputs/build")).unwrap();
    fs::write(
        project_workspace.join("state/style-contract.json"),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        )
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("state/dependency-state.json"),
        json!({
            "needsRestore": false,
            "packageManager": "npm",
            "success": true
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("state/preview.json"),
        json!({
            "url": "http://127.0.0.1:4321",
            "status": "running"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "buildId": "build-1",
            "sourceSnapshotUri": "runtime://snapshots/project-1/version-1"
        })
        .to_string(),
    )
    .unwrap();
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
    let run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
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
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/project-1/version-1".to_string()),
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
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["currentVersionId"], candidate.id);
    assert_eq!(payload["sandboxBindingId"], binding.id);
    assert_eq!(
        payload["sourceSnapshotUri"],
        "runtime://snapshots/project-1/version-1"
    );
    assert_eq!(payload["appRoot"], "project");
    assert_eq!(payload["templateKey"], "astro-website");
    assert_eq!(
        payload["styleContractPath"],
        "/workspace/state/style-contract.json"
    );
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["globalCssFile"],
        "project/src/styles/global.css"
    );
    assert_eq!(
        payload["styleContract"]["componentRoot"],
        "project/src/components/ui"
    );
    assert_eq!(payload["styleContract"]["tailwind"]["version"], "4");
    assert_eq!(
        payload["styleContract"]["tailwind"]["entryImport"],
        "@import \"tailwindcss\""
    );
    assert_eq!(
        payload["styleContract"]["tailwind"]["themeSource"],
        "css-variables"
    );
    assert_eq!(payload["dependencyState"]["needsRestore"], false);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
}

#[tokio::test]
async fn project_runtime_state_reads_phase_a_global_workspace_lifecycle_state() {
    let workspace = unique_temp_dir("runtime-state-phase-a-global");
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::write(
        workspace.join("state/style-contract.json"),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        )
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/dependency-state.json"),
        json!({ "needsRestore": true }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        json!({ "status": "running" }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "sourceSnapshotUri": "runtime://snapshots/phase-a-project/version-1"
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "phase-a-project".to_string(),
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
    let run = store
        .create_run_with_context(
            "phase-a-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "phase-a-project",
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
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "phase-a-project",
            &run.id,
            "http://preview.local/phase-a-project/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/phase-a-project/version-1".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "phase-a-project",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/phase-a-project/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["componentRoot"],
        "project/src/components/ui"
    );
    assert_eq!(
        payload["styleContract"]["tailwind"]["themeSource"],
        "css-variables"
    );
    assert_eq!(payload["dependencyState"]["needsRestore"], true);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
}

#[tokio::test(flavor = "current_thread")]
async fn project_runtime_state_reads_kubernetes_workspace_channel_lifecycle_state() {
    let _env_guard = SANDBOX_CHANNEL_ENV_LOCK.lock().unwrap();
    let mut files = HashMap::new();
    files.insert(
        "/workspace/state/style-contract.json".to_string(),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        ),
    );
    files.insert(
        "/workspace/state/dependency-state.json".to_string(),
        json!({ "needsRestore": false }),
    );
    files.insert(
        "/workspace/state/preview.json".to_string(),
        json!({ "status": "running" }),
    );
    files.insert(
        "/workspace/outputs/build/latest.json".to_string(),
        json!({
            "status": "success",
            "sourceSnapshotUri": "runtime://snapshots/k8s-project/version-1"
        }),
    );
    let (host, port, _requests, server) = start_runtime_state_workspace_channel_server(files).await;
    let _channel_env = SandboxChannelEnvOverride::set(host, port);

    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "k8s-project".to_string(),
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
    let run = store
        .create_run_with_context(
            "k8s-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "k8s-project",
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
    store
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            binding.sandbox_name.clone(),
            Some(binding.sandbox_name.clone()),
            Some("sandbox-uid-k8s-project".to_string()),
            Some("pod-uid-k8s-project".to_string()),
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "k8s-project",
            &run.id,
            "http://preview.local/k8s-project/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/k8s-project/version-1".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "k8s-project",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::Kubernetes;
    let app = http_api::router_with_state(AppState {
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/k8s-project/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    server.abort();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["globalCssFile"],
        "project/src/styles/global.css"
    );
    assert_eq!(payload["styleContract"]["tailwind"]["version"], "4");
    assert_eq!(payload["dependencyState"]["needsRestore"], false);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
    assert_eq!(_requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn start_edit_rejects_stale_base_version() {
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
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/project-1/current".to_string()),
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
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
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
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "baseVersionId": "version-stale"
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
        .contains("baseVersionId version-stale is stale"));
}

#[tokio::test]
async fn start_edit_waits_for_continue_before_spawning_agent() {
    let workspace = unique_temp_dir("http-edit-waits-restore");
    fs::create_dir_all(
        workspace
            .join("project-1")
            .join("outputs/build/source-snapshots/current"),
    )
    .unwrap();
    fs::write(
        workspace
            .join("project-1")
            .join("outputs/build/source-snapshots/current/package.json"),
        "{}",
    )
    .unwrap();
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
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/outputs/build/source-snapshots/current".to_string()),
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
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let model = MockModelClient::new(vec![ModelResponse::Error(
        "edit agent should wait for continue".to_string(),
    )]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(model.clone()),
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
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "baseVersionId": candidate.id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    if status != StatusCode::OK {
        panic!(
            "unexpected start edit response {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = payload["runId"].as_str().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        store.get_run(edit_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );
}

#[tokio::test]
async fn start_mutable_run_rejects_existing_project_mutation() {
    let store = RuntimeStore::new();
    let active = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    assert_eq!(active.status, AgentRunStatus::Queued);
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
                            "briefId": brief_id
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
    assert_eq!(
        payload["error"],
        format!(
            "project project-1 already has active mutable run {}",
            active.id
        )
    );
}

#[tokio::test]
async fn public_runtime_lifecycle_build_runtime_state_edit_and_rebuilds() {
    let workspace = unique_temp_dir("http-lifecycle-edit");
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let (preview_url, _preview_server) = start_preview_server().await;
    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); const html=fs.readFileSync('src/pages/index.astro','utf8'); const tokens=fs.existsSync('src/styles/tokens.css')?fs.readFileSync('src/styles/tokens.css','utf8'):''; fs.writeFileSync('dist/index.html', `${html}\n<style>${tokens}</style>`);"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "build-init",
                "project.init",
                json!({ "template": "astro-website" }),
            ),
            ToolCall::new(
                "build-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "build-tokens",
                "fs.write",
                json!({
                    "path": "project/src/styles/tokens.css",
                    "text": ":root {\n  --runtime-primary: #2563eb;\n}\n"
                }),
            ),
            ToolCall::new(
                "build-page",
                "fs.write",
                json!({ "path": "project/src/pages/index.astro", "text": "<main><h1>Initial hero</h1></main>" }),
            ),
            ToolCall::new("build-run", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "build-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "build-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "build-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-build", "blank": false }),
            ),
            ToolCall::new(
                "build-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-build"
                }),
            ),
            ToolCall::new(
                "build-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Initial preview promoted" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "edit-read",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
            ToolCall::new(
                "edit-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "Initial hero",
                    "newStr": "Edited hero"
                }),
            ),
            ToolCall::new("edit-build", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "edit-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "edit-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "edit-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-edit", "blank": false }),
            ),
            ToolCall::new(
                "edit-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-edit"
                }),
            ),
            ToolCall::new(
                "edit-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Edited preview promoted" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "theme-style-update",
                "style.update_tokens",
                json!({
                    "tokens": {
                        "color.primary": "#f97316"
                    }
                }),
            ),
            ToolCall::new("theme-build", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "theme-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "theme-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "theme-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-theme-edit", "blank": false }),
            ),
            ToolCall::new(
                "theme-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-theme-edit"
                }),
            ),
            ToolCall::new(
                "theme-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Theme preview promoted" }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let build_response = app
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
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(build_response.status(), StatusCode::OK);
    let body = to_bytes(build_response.into_body(), 4096).await.unwrap();
    let build_payload: Value = serde_json::from_slice(&body).unwrap();
    let build_run_id = build_payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &build_run_id).await;
    let build_run = store.get_run(&build_run_id).await.unwrap();
    assert_eq!(
        build_run.status,
        AgentRunStatus::Completed,
        "build run failed: {build_run:?} events={:?}",
        store.events(&build_run_id).await
    );
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runtime_state.status(), StatusCode::OK);
    let body = to_bytes(runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let runtime_state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(runtime_state["currentVersionId"], initial_version_id);
    assert_eq!(runtime_state["templateKey"], "astro-website");
    fs::write(
        workspace.join("project-1/project/src/pages/index.astro"),
        "<main><h1>Corrupted workspace</h1></main>",
    )
    .unwrap();

    let edit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "briefId": brief_id,
                            "baseVersionId": runtime_state["currentVersionId"],
                            "sandboxBindingId": runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit_response.status(), StatusCode::OK);
    let body = to_bytes(edit_response.into_body(), 4096).await.unwrap();
    let edit_payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = edit_payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_run(&edit_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    let continue_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Change the hero title to Edited hero" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(edit_run.status, AgentRunStatus::Completed);
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        edited_version_id
    );
    let html = fs::read_to_string(workspace.join("project-1/project/dist/index.html")).unwrap();
    assert!(html.contains("Edited hero"));
    assert!(!html.contains("Initial hero"));
    assert!(store
        .events(&edit_run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "preview.updated"));

    let hero_runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hero_runtime_state.status(), StatusCode::OK);
    let body = to_bytes(hero_runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let hero_runtime_state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(hero_runtime_state["currentVersionId"], edited_version_id);
    assert_eq!(
        hero_runtime_state["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );

    let theme_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "briefId": brief_id,
                            "baseVersionId": hero_runtime_state["currentVersionId"],
                            "sandboxBindingId": hero_runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(theme_response.status(), StatusCode::OK);
    let body = to_bytes(theme_response.into_body(), 4096).await.unwrap();
    let theme_payload: Value = serde_json::from_slice(&body).unwrap();
    let theme_run_id = theme_payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_run(&theme_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    let continue_theme_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{theme_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Change the primary theme color token to #f97316" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_theme_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &theme_run_id).await;
    let theme_run = store.get_run(&theme_run_id).await.unwrap();
    assert_eq!(
        theme_run.status,
        AgentRunStatus::Completed,
        "theme run failed: {theme_run:?} events={:?}",
        store.events(&theme_run_id).await
    );
    let theme_version_id = theme_run.output_version_id.clone().unwrap();
    assert_ne!(theme_version_id, edited_version_id);
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        theme_version_id
    );
    let tokens =
        fs::read_to_string(workspace.join("project-1/project/src/styles/tokens.css")).unwrap();
    assert!(tokens.contains("--runtime-primary: #f97316;"));
    assert!(!tokens.contains("--runtime-primary: #2563eb;"));
    let themed_html =
        fs::read_to_string(workspace.join("project-1/project/dist/index.html")).unwrap();
    assert!(themed_html.contains("Edited hero"));
    assert!(themed_html.contains("--runtime-primary: #f97316;"));
    assert!(store
        .events(&theme_run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "preview.updated"));
}

#[tokio::test]
async fn phase_a_public_run_uses_project_scoped_workspace_root() {
    let workspace = unique_temp_dir("http-project-workspace-isolation");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-a".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let model = MockModelClient::new(
        (0..8)
            .map(|index| {
                ModelResponse::ToolCalls(vec![
                    ToolCall::new(
                        format!("init-website-{index}"),
                        "project.init",
                        json!({ "template": "astro-website" }),
                    ),
                    ToolCall::new(
                        format!("complete-website-{index}"),
                        "run.complete",
                        json!({ "status": "completed", "summary": "website initialized" }),
                    ),
                ])
            })
            .collect(),
    );
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let website_run_id = start_public_run(
        app.clone(),
        "project-a",
        "build",
        json!({ "briefId": brief_id }),
    )
    .await;
    assert!(
        wait_for_terminal_with_timeout(&store, &website_run_id, 5).await,
        "website run should finish"
    );

    assert!(workspace
        .join("project-a/project/src/pages/index.astro")
        .exists());
    assert!(!workspace.join("project").exists());
    assert!(!workspace.join("project-a/project/app").exists());
}

#[tokio::test]
async fn public_runtime_docs_lifecycle_build_runtime_state_edit_and_rebuilds() {
    let workspace = unique_temp_dir("http-docs-lifecycle-edit");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "docs-project".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, docs_brief())
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let (preview_url, _preview_server) = start_preview_server().await;
    let build_script = "const fs=require('fs'); fs.mkdirSync('out/docs',{recursive:true}); const mdx=fs.readFileSync('content/docs/index.mdx','utf8'); fs.writeFileSync('out/docs.html', `<main>${mdx}</main>`); fs.writeFileSync('out/index.html', '<a href=\"/docs\">Docs</a>');";
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-init",
            "project.init",
            json!({ "template": "fumadocs-docs" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "docs-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "docs-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "docs-mdx",
                "fs.write",
                json!({
                    "path": "project/content/docs/index.mdx",
                    "text": "---\ntitle: Overview\n---\n\n# Initial docs title\n\nInitial lifecycle section"
                }),
            ),
            ToolCall::new("docs-build", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "docs-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "docs-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "docs-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-docs-build", "blank": false }),
            ),
            ToolCall::new(
                "docs-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-docs-build"
                }),
            ),
            ToolCall::new(
                "docs-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Docs preview promoted" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "docs-edit-read",
                "fs.read",
                json!({ "path": "project/content/docs/index.mdx" }),
            ),
            ToolCall::new(
                "docs-edit-patch",
                "fs.patch",
                json!({
                    "path": "project/content/docs/index.mdx",
                    "oldStr": "Initial docs title",
                    "newStr": "Edited docs title"
                }),
            ),
            ToolCall::new(
                "docs-edit-build",
                "project.build",
                json!({ "cwd": "project" }),
            ),
            ToolCall::new(
                "docs-edit-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "docs-edit-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "docs-edit-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-docs-edit", "blank": false }),
            ),
            ToolCall::new(
                "docs-edit-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-docs-edit"
                }),
            ),
            ToolCall::new(
                "docs-edit-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Edited docs preview promoted" }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let build_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "docs-project",
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
    assert_eq!(build_response.status(), StatusCode::OK);
    let body = to_bytes(build_response.into_body(), 4096).await.unwrap();
    let build_payload: Value = serde_json::from_slice(&body).unwrap();
    let build_run_id = build_payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &build_run_id).await;
    let build_run = store.get_run(&build_run_id).await.unwrap();
    assert_eq!(build_run.status, AgentRunStatus::Completed);
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/docs-project/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runtime_state.status(), StatusCode::OK);
    let body = to_bytes(runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let runtime_state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(runtime_state["currentVersionId"], initial_version_id);
    assert_eq!(runtime_state["templateKey"], "fumadocs-docs");
    fs::write(
        workspace.join("docs-project/project/content/docs/index.mdx"),
        "# Corrupted docs title\n\nCorrupted content",
    )
    .unwrap();

    let edit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "docs-project",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "briefId": brief_id,
                            "baseVersionId": runtime_state["currentVersionId"],
                            "sandboxBindingId": runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit_response.status(), StatusCode::OK);
    let body = to_bytes(edit_response.into_body(), 4096).await.unwrap();
    let edit_payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = edit_payload["runId"].as_str().unwrap().to_string();
    let continue_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Rename the overview page to Edited docs title" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(edit_run.status, AgentRunStatus::Completed);
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);
    assert_eq!(
        store
            .current_project_version("docs-project")
            .await
            .unwrap()
            .id,
        edited_version_id
    );
    let mdx =
        fs::read_to_string(workspace.join("docs-project/project/content/docs/index.mdx")).unwrap();
    assert!(mdx.contains("Edited docs title"));
    assert!(!mdx.contains("Initial docs title"));
    let html = fs::read_to_string(workspace.join("docs-project/project/out/docs.html")).unwrap();
    assert!(html.contains("Edited docs title"));
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY, network access, and npm registry access"]
async fn real_provider_public_runtime_website_and_docs_lifecycle_matrix() {
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set for this test");
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model_name = std::env::var("DEEPSEEK_E2E_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let workspace = unique_temp_dir("real-provider-http-lifecycle");
    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.agent_model = model_name;
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.npm_registry = std::env::var("RUNTIME_E2E_NPM_REGISTRY")
        .unwrap_or_else(|_| "https://registry.npmjs.org/".to_string());
    let model = OpenAiCompatibleModelClient::new(base_url, api_key, Some("deepseek"))
        .with_streaming(env_flag("MODEL_STREAMING"))
        .with_strict_tools(env_flag("MODEL_STRICT_TOOLS"));
    let app = http_api::router_with_state(AppState {
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    run_real_provider_lifecycle_project(
        app.clone(),
        &store,
        &workspace,
        "real-http-website",
        website_brief(),
        vec![
            ContentSource::readable(
                "website-design",
                "design_md",
                "# Website design\n- Build a compact operational SaaS website for runtime harness engineers.\n- Use Tailwind/token styling and local UI primitives from the runtime template.\n- Hero title should initially include Runtime Harness.\n- Include sections for lifecycle, typed recovery, preview promotion, and evidence.\n",
            ),
            ContentSource::readable(
                "website-instructions",
                "prompt",
                "Use project.init with astro-website if needed, inspect the project, and use preview.publish for build/screenshot/candidate/promotion. Prefer style.update_tokens for color edits. Do not use raw npm/pnpm install commands through shell.run.",
            ),
        ],
        "Acceptance criteria: the promoted website artifact must contain the literal text TESTXXX in the hero title. Change the hero title to TESTXXX 标题内容, set the primary theme color token to #f97316 using style.update_tokens when possible, then rebuild and promote with preview.publish exactly once. Do not call run.complete until the served artifact contains TESTXXX.",
        "/artifacts/real-http-website/current/",
        "project/dist/index.html",
        "TESTXXX",
    )
    .await;

    run_real_provider_lifecycle_project(
        app,
        &store,
        &workspace,
        "real-http-docs",
        docs_brief(),
        vec![
            ContentSource::readable(
                "docs-design",
                "design_md",
                "# Docs design\n- Build a Fumadocs documentation portal for runtime lifecycle operations.\n- The overview page should explain create, generate, edit, build, screenshot, and promote.\n- Include a section on typed recoverable errors and preview evidence.\n",
            ),
            ContentSource::readable(
                "docs-instructions",
                "prompt",
                "Use project.init with fumadocs-docs if needed, inspect the project, and use preview.publish for build/screenshot/candidate/promotion. Keep Docs source editable and tokenized.",
            ),
        ],
        "Rename the overview page to Edited docs title and add one short section about browser computed-style verification. Rebuild and promote with preview.publish.",
        "/artifacts/real-http-docs/current/docs",
        "project/out/docs.html",
        "Edited docs title",
    )
    .await;
}

async fn run_real_provider_lifecycle_project(
    app: axum::Router,
    store: &RuntimeStore,
    workspace_root: &Path,
    project_id: &str,
    brief: Brief,
    content_sources: Vec<ContentSource>,
    edit_prompt: &str,
    artifact_path: &str,
    local_artifact_relative: &str,
    expected_artifact_text: &str,
) {
    let brief_run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store.write_brief(&brief_run.id, brief).await.unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();

    let build_run_id = start_public_run(
        app.clone(),
        project_id,
        "build",
        json!({
            "briefId": brief_id,
            "contentSources": content_sources
        }),
    )
    .await;
    if !wait_for_terminal_with_timeout(store, &build_run_id, REAL_PROVIDER_STAGE_TIMEOUT_SECS).await
    {
        emit_real_provider_run_stream(store, &build_run_id, project_id, "build").await;
        panic!(
            "run {build_run_id} did not reach terminal status within {REAL_PROVIDER_STAGE_TIMEOUT_SECS}s"
        );
    }
    emit_real_provider_run_stream(store, &build_run_id, project_id, "build").await;
    let build_run = store.get_run(&build_run_id).await.unwrap();
    assert_eq!(
        build_run.status,
        AgentRunStatus::Completed,
        "real provider build run {build_run_id} should complete; events={}",
        serde_json::to_string(&store.events(&build_run_id).await).unwrap()
    );
    assert_preview_updated_before_completed(store, &build_run_id).await;
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = get_json(
        app.clone(),
        &format!("/projects/{project_id}/runtime-state"),
        8192,
    )
    .await;
    assert_eq!(runtime_state["currentVersionId"], initial_version_id);
    let initial_snapshot = runtime_state["sourceSnapshotUri"]
        .as_str()
        .expect("runtime state should include source snapshot")
        .to_string();
    assert_eq!(
        runtime_state["latestBuild"]["sourceSnapshotUri"], initial_snapshot,
        "build runtime-state latestBuild sourceSnapshotUri should match promoted sourceSnapshotUri"
    );
    let build_current_preview =
        get_json(app.clone(), &format!("/preview/{project_id}/current"), 8192).await;
    assert_eq!(build_current_preview["versionId"], initial_version_id);
    assert_eq!(build_current_preview["status"], "promoted");
    let build_artifact = get_text(app.clone(), artifact_path, 256_000).await;
    let build_artifact_byte_length = build_artifact.len();
    assert!(
        build_artifact_byte_length > 0,
        "build artifact {artifact_path} should be non-empty"
    );
    let build_local_artifact_url =
        local_artifact_url(workspace_root, project_id, local_artifact_relative);
    emit_real_provider_evidence(
        project_id,
        "build",
        &build_run_id,
        json!({
            "runtimeState": runtime_state.clone(),
            "currentPreview": build_current_preview,
            "sourceSnapshotUri": initial_snapshot,
            "artifactPath": artifact_path,
            "localArtifactUrl": build_local_artifact_url,
            "artifactServed": true,
            "artifactByteLength": build_artifact_byte_length,
            "previewUpdatedBeforeCompleted": true
        }),
    );

    let edit_run_id = start_public_run(
        app.clone(),
        project_id,
        "edit",
        json!({
            "briefId": brief_id,
            "baseVersionId": runtime_state["currentVersionId"],
            "sandboxBindingId": runtime_state["sandboxBindingId"]
        }),
    )
    .await;
    post_continue(app.clone(), &edit_run_id, edit_prompt).await;
    if !wait_for_terminal_with_timeout(store, &edit_run_id, REAL_PROVIDER_STAGE_TIMEOUT_SECS).await
    {
        emit_real_provider_run_stream(store, &edit_run_id, project_id, "edit").await;
        panic!(
            "run {edit_run_id} did not reach terminal status within {REAL_PROVIDER_STAGE_TIMEOUT_SECS}s"
        );
    }
    emit_real_provider_run_stream(store, &edit_run_id, project_id, "edit").await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(
        edit_run.status,
        AgentRunStatus::Completed,
        "real provider edit run {edit_run_id} should complete; events={}",
        serde_json::to_string(&store.events(&edit_run_id).await).unwrap()
    );
    assert_preview_updated_before_completed(store, &edit_run_id).await;
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);

    let edited_state = get_json(
        app.clone(),
        &format!("/projects/{project_id}/runtime-state"),
        8192,
    )
    .await;
    assert_eq!(edited_state["currentVersionId"], edited_version_id);
    let edited_snapshot = edited_state["sourceSnapshotUri"]
        .as_str()
        .expect("edited runtime state should include source snapshot")
        .to_string();
    assert_eq!(
        edited_state["latestBuild"]["sourceSnapshotUri"], edited_snapshot,
        "edit runtime-state latestBuild sourceSnapshotUri should match promoted sourceSnapshotUri"
    );
    let source_snapshot_changed = edited_snapshot != initial_snapshot;
    assert!(source_snapshot_changed);

    let current_preview =
        get_json(app.clone(), &format!("/preview/{project_id}/current"), 8192).await;
    assert_eq!(current_preview["versionId"], edited_version_id);
    assert_eq!(current_preview["status"], "promoted");

    let artifact = get_text(app, artifact_path, 256_000).await;
    let artifact_byte_length = artifact.len();
    let artifact_contains_expected = artifact.contains(expected_artifact_text);
    let edited_local_artifact_url =
        local_artifact_url(workspace_root, project_id, local_artifact_relative);
    emit_real_provider_evidence(
        project_id,
        "edit",
        &edit_run_id,
        json!({
            "runtimeState": edited_state,
            "currentPreview": current_preview,
            "initialVersionId": initial_version_id,
            "editedVersionId": edited_version_id,
            "sourceSnapshotUri": edited_snapshot,
            "initialSourceSnapshotUri": initial_snapshot,
            "editedSourceSnapshotUri": edited_snapshot,
            "sourceSnapshotChanged": source_snapshot_changed,
            "artifactPath": artifact_path,
            "localArtifactUrl": edited_local_artifact_url,
            "artifactServed": true,
            "artifactByteLength": artifact_byte_length,
            "artifactContainsExpectedText": artifact_contains_expected,
            "artifactContainsEditMarker": artifact_contains_expected,
            "expectedArtifactText": expected_artifact_text,
            "previewUpdatedBeforeCompleted": true
        }),
    );
    assert!(
        artifact_contains_expected,
        "artifact {artifact_path} should include edited text {expected_artifact_text:?}; body preview={}",
        artifact.chars().take(1000).collect::<String>()
    );
}

fn local_artifact_url(workspace_root: &Path, project_id: &str, relative: &str) -> String {
    let path = workspace_root.join(project_id).join(relative);
    assert!(
        path.exists(),
        "local artifact file should exist for provider evidence: {}",
        path.display()
    );
    format!("file://{}", path.display())
}

fn emit_real_provider_evidence(project_id: &str, stage: &str, run_id: &str, evidence: Value) {
    eprintln!(
        "REAL_PROVIDER_EVIDENCE {}",
        serde_json::to_string(&json!({
            "project": project_id,
            "stage": stage,
            "runId": run_id,
            "evidence": evidence
        }))
        .unwrap()
    );
}

async fn emit_real_provider_run_stream(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    stage: &str,
) {
    let run = store.get_run(run_id).await;
    eprintln!(
        "REAL_PROVIDER_STREAM_BEGIN project={} stage={} run={} status={:?} outputVersion={:?}",
        project_id,
        stage,
        run_id,
        run.as_ref().map(|run| &run.status),
        run.as_ref()
            .and_then(|run| run.output_version_id.as_deref())
    );
    for event in store.events(run_id).await {
        eprintln!(
            "REAL_PROVIDER_EVENT {}",
            serde_json::to_string(&event).unwrap()
        );
    }
    eprintln!(
        "REAL_PROVIDER_STREAM_END project={} stage={} run={}",
        project_id, stage, run_id
    );
}

async fn start_public_run(
    app: axum::Router,
    project_id: &str,
    phase: &str,
    input_context: Value,
) -> String {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": project_id,
                        "phase": phase,
                        "agentProfile": phase,
                        "inputContext": input_context
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
    payload["runId"].as_str().unwrap().to_string()
}

async fn post_continue(app: axum::Router, run_id: &str, user_message: &str) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": user_message }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn get_json(app: axum::Router, uri: &str, limit: usize) -> Value {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), limit).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn get_text(app: axum::Router, uri: &str, limit: usize) -> String {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), limit)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn assert_preview_updated_before_completed(store: &RuntimeStore, run_id: &str) {
    let event_types = store
        .events(run_id)
        .await
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    let updated_index = event_types
        .iter()
        .position(|event| event == "preview.updated")
        .expect("preview.updated should be emitted");
    let completed_index = event_types
        .iter()
        .position(|event| event == "run.completed")
        .expect("run.completed should be emitted");
    assert!(
        updated_index < completed_index,
        "preview.updated must be emitted before run.completed: {event_types:?}"
    );
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
    let app = http_api::router_with_state(AppState {
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-anydesign-preview-lease"], lease.id);
    let html =
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(html.contains(&format!("/previews/{}/assets/app.js", lease.id)));

    store.stop_preview_lease(&lease.id).await.unwrap();
    let stopped = app
        .oneshot(
            Request::builder()
                .uri(format!("/previews/{}/", lease.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    upstream.abort();
    assert_eq!(stopped.status(), StatusCode::NOT_FOUND);
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
async fn internal_template_build_route_is_disabled_by_default() {
    let app = http_api::router_with_state(AppState {
        config: RuntimeConfig::from_env(),
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
    let mut config = RuntimeConfig::from_env();
    config.enable_internal_template_build_api = true;
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
    config.workspace_root = workspace.clone();
    config.runtime_storage_dir = workspace.join("runtime-storage");
    let store = install_immutable_artifact(&config, "docs-project", &project_root).await;
    let app = http_api::router_with_state(AppState {
        config,
        store,
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
    config.workspace_root = workspace.clone();
    config.runtime_storage_dir = workspace.join("runtime-storage");
    let store = install_immutable_artifact(&config, "phase-a-project", &project_root).await;
    let app = http_api::router_with_state(AppState {
        config,
        store,
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

async fn wait_for_terminal(store: &RuntimeStore, run_id: &str) {
    for _ in 0..100 {
        if store
            .get_run(run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("run {run_id} did not reach terminal status");
}

async fn wait_for_terminal_with_timeout(store: &RuntimeStore, run_id: &str, seconds: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        if store
            .get_run(run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

async fn start_preview_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    (format!("http://{addr}/candidate"), handle)
}

async fn start_candidate_preview_upstream(manifest_hash: String) -> (String, u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let html = "<!doctype html><script src=\"/assets/app.js\"></script>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nX-AnyDesign-Candidate-Manifest-Hash: {manifest_hash}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    ("127.0.0.1".to_string(), addr.port(), handle)
}

async fn install_immutable_artifact(
    config: &RuntimeConfig,
    project_id: &str,
    source_root: &Path,
) -> RuntimeStore {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let candidate = store
        .create_project_version_candidate(
            project_id,
            &run.id,
            format!("http://preview.local/{project_id}"),
            None,
            None,
        )
        .await;
    store
        .promote_project_version(project_id, &run.id, &candidate.id)
        .await
        .unwrap();
    let target = anydesign_runtime::artifact_publisher::FileArtifactPublisher::version_root(
        &config.runtime_storage_dir,
        project_id,
        &candidate.id,
    );
    copy_test_directory(source_root, &target);
    store
}

fn copy_test_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_test_directory(&source_path, &target_path);
        } else {
            fs::copy(source_path, target_path).unwrap();
        }
    }
}

async fn start_runtime_state_workspace_channel_server(
    files: HashMap<String, Value>,
) -> (String, u16, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    let files = Arc::new(files);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let requests = server_requests.clone();
            let files = files.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = accept_async(stream).await else {
                    return;
                };
                let Some(Ok(message)) = socket.next().await else {
                    return;
                };
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => {
                        String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| "{}".to_string())
                    }
                    _ => "{}".to_string(),
                };
                let request: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
                requests.lock().unwrap().push(request.clone());
                let path = request["path"].as_str().unwrap_or("");
                let response = if request["op"].as_str() == Some("fs.read") {
                    files
                        .get(path)
                        .map(|value| json!({ "ok": true, "result": { "text": value.to_string() } }))
                        .unwrap_or_else(|| json!({ "ok": false, "error": "not found" }))
                } else {
                    json!({ "ok": false, "error": "unsupported op" })
                };
                let _ = socket
                    .send(Message::Text(response.to_string().into()))
                    .await;
            });
        }
    });
    ("127.0.0.1".to_string(), addr.port(), requests, handle)
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
