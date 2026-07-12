use anydesign_runtime::{
    config::{PublicPrincipalAuthMode, RuntimePolicyProfile, SandboxBackendMode},
    http_api::{self, AppState},
    model_gateway::{
        MockModelClient, ModelResponse, OpenAiCompatibleModelClient, ToolCall,
        ToolInputParseFailure,
    },
    preview::{promote_preview, PromotionGateReport},
    public_principal::{PublicPrincipalClaims, PublicPrincipalJwtIssuer, PREVIEW_READ_OPERATION},
    types::{
        sha256_hex, AgentEvent, AgentPhase, AgentRunStatus, Brief, BriefStatus, ContentSource,
        PreviewLeaseStatus, ReviewFindingCategory, ReviewFindingSeverity, ReviewFindingStatus,
        SandboxBindingStatus, SandboxChannelProtocol,
    },
    RuntimeConfig, RuntimeStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
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
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
    time::{timeout, Duration},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tower::ServiceExt;

static SANDBOX_CHANNEL_ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
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

pub(super) fn phase_a_contract_config() -> RuntimeConfig {
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
    config.runtime_storage_dir = unique_temp_dir("http-runtime-storage");
    config
}

fn public_principal_token(
    issuer: &PublicPrincipalJwtIssuer,
    principal_id: &str,
    project_id: &str,
) -> String {
    issuer
        .issue(PublicPrincipalClaims {
            iss: String::new(),
            aud: String::new(),
            sub: principal_id.to_string(),
            jti: format!("public-jti-{principal_id}-0001"),
            exp: 0,
            iat: 0,
            project_id: project_id.to_string(),
            operations: vec![PREVIEW_READ_OPERATION.to_string()],
        })
        .unwrap()
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

#[path = "../cases/artifacts.rs"]
mod artifacts;
#[path = "../cases/design_profiles.rs"]
mod design_profiles;
#[path = "../cases/design_sources.rs"]
mod design_sources;
#[path = "../cases/edit_lifecycle.rs"]
mod edit_lifecycle;
#[path = "../cases/internal.rs"]
mod internal;
#[path = "../cases/lifecycle_docs_real.rs"]
mod lifecycle_docs_real;
#[path = "../cases/lifecycle_website.rs"]
mod lifecycle_website;
#[path = "../cases/previews.rs"]
mod previews;
#[path = "../cases/profile_run_integration.rs"]
mod profile_run_integration;
#[path = "../cases/project_runtime.rs"]
mod project_runtime;
#[path = "../cases/run_events.rs"]
mod run_events;
#[path = "../cases/run_mutations.rs"]
mod run_mutations;
#[path = "../cases/runs_bindings.rs"]
mod runs_bindings;
#[path = "../cases/runs_repair.rs"]
mod runs_repair;
#[path = "../cases/runs_start.rs"]
mod runs_start;
#[path = "../cases/system.rs"]
mod system;
