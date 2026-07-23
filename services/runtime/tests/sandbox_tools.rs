use anydesign_runtime::{
    artifact_routes::{ArtifactRouteContract, ArtifactRouteFile, ArtifactRouteManifest},
    config::RuntimePolicyProfile,
    conversation::RuntimeStore,
    draft_preview::StartDraftPreview,
    model_gateway::ToolCall,
    project::{ProjectInitRecoveryOutcome, ProjectInitWorkspaceTransaction},
    project_asset::ProjectAssetStore,
    tools::{
        control_plane::control_plane_executor,
        runtime::ToolContext,
        runtime::ToolExecutor,
        sandbox::{
            sandbox_tools, sandbox_tools_with_backends, sandbox_tools_with_workspace_backend,
            JsonWorkspaceChannelBackend, JsonWorkspaceChannelCommandBackend, LocalWorkspaceBackend,
            SandboxChannelWorkspaceBackend, SandboxCommandBackend,
            WebSocketWorkspaceChannelTransport, WorkspaceBackend, WorkspaceChannelEndpointResolver,
            WorkspaceChannelRequest, WorkspaceChannelTransport, WorkspaceEntry, WorkspaceEntryKind,
            WorkspacePathKind,
        },
        streaming::{tool_result_error_text, StreamingToolExecutor},
    },
    types::{
        sha256_hex, AgentEvent, AgentPhase, AgentRunStatus, DesignProfile, DesignSourceIndex,
        DesignSourceIndexSection, ObservationOutcome, ObservationView, PreviewLeaseMode,
        PreviewLeaseStatus, SandboxBindingStatus, SandboxChannelProtocol,
    },
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::DraftPreviewSessionStatus,
    workspace_auth::{WorkspaceChannelClaims, WorkspaceChannelJwtIssuer},
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use ed25519_dalek::{pkcs8::EncodePublicKey, Signer, SigningKey};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    net::TcpListener as StdTcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::AsyncWriteExt, net::TcpListener, process::Command, task::JoinHandle, time::Duration,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn assert_error_kind(result: &anydesign_runtime::tools::runtime::ToolResult, expected: &str) {
    let metadata = result.metadata.as_ref().expect("error metadata");
    assert_eq!(
        metadata.get("errorKind").and_then(Value::as_str),
        Some(expected)
    );
    assert_eq!(
        metadata.get("recoverable").and_then(Value::as_bool),
        Some(true)
    );
}

static ASSET_PROVIDER_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn create_run(store: &RuntimeStore) -> String {
    store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await
        .id
}

fn imported_design_profile(source_artifact_id: &str, source_hash: &str) -> DesignProfile {
    let now = Utc::now();
    DesignProfile {
        id: "design-profile-source-gate".to_string(),
        schema_version: "design-profile@2".to_string(),
        name: "Source Gate".to_string(),
        status: "active".to_string(),
        version: 2,
        scope: json!({ "projectId": "project-1" }),
        source: json!({
            "kind": "imported",
            "sourceArtifactIds": [source_artifact_id],
            "primarySourceArtifactId": source_artifact_id,
            "sourceHash": source_hash,
            "converterVersion": "test@1",
            "integrity": "verified"
        }),
        product: json!({ "name": "Source Gate", "category": "test" }),
        brand: json!({}),
        visual: json!({ "direction": "test direction" }),
        tokens: json!({}),
        runtime_token_mapping: json!({
            "color.background": "#ffffff",
            "color.surface": "#f8fafc",
            "color.surfaceStrong": "#e2e8f0",
            "color.text": "#0f172a",
            "color.muted": "#475569",
            "color.primary": "#663af3",
            "color.primaryContrast": "#ffffff",
            "color.border": "#cbd5e1",
            "radius.card": "8px",
            "radius.control": "6px",
            "font.sans": "Inter, sans-serif",
            "shadow.soft": "none"
        }),
        extended_token_mapping: json!({}),
        components: json!({}),
        website_context: Value::Null,
        content: json!({}),
        accessibility: json!({}),
        technical: json!({ "allowedTemplates": ["next-app"] }),
        governance: json!({ "conflictBehavior": "ask" }),
        signature_rules: vec![json!({
            "id": "required-source",
            "statement": "Read the required source section.",
            "priority": "required",
            "appliesTo": ["website"],
            "verification": { "kind": "visual-review", "rubric": "Check it." }
        })],
        overrides: json!({}),
        created_at: now,
        updated_at: now,
    }
}

fn sandbox_executor(workspace: &Path) -> StreamingToolExecutor {
    StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        Default::default(),
        workspace,
    ))
}

fn sandbox_executor_local_e2e(workspace: &Path) -> StreamingToolExecutor {
    StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), workspace)
            .with_policy_profile_and_registry(
                RuntimePolicyProfile::LocalE2e,
                "https://registry.npmjs.org/",
            ),
    )
}

fn one_pixel_visual_asset() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[24, 80, 180, 255]).unwrap();
    drop(writer);
    bytes
}

#[derive(Debug, Clone)]
struct RecordingWorkspaceBackend {
    read_text: String,
    reads: Arc<Mutex<Vec<PathBuf>>>,
    writes: Arc<Mutex<Vec<(PathBuf, String)>>>,
}

#[derive(Debug, Default, Clone)]
struct RecordingChannelTransport {
    requests: Arc<Mutex<Vec<WorkspaceChannelRequest>>>,
    files: Arc<Mutex<HashMap<String, String>>>,
}

#[derive(Debug, Clone)]
enum ExecBehavior {
    Error(io::ErrorKind),
    Output {
        status: i32,
        success: bool,
        stdout: String,
        stderr: String,
    },
}

#[derive(Debug, Clone)]
struct ExecBehaviorTransport {
    requests: Arc<Mutex<Vec<WorkspaceChannelRequest>>>,
    behavior: ExecBehavior,
}

impl ExecBehaviorTransport {
    fn new(behavior: ExecBehavior) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            behavior,
        }
    }
}

#[derive(Debug, Clone)]
struct StaticEndpointResolver {
    endpoint: String,
    seen_sandbox_ids: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl WorkspaceChannelEndpointResolver for StaticEndpointResolver {
    async fn endpoint(&self, ctx: &ToolContext) -> io::Result<String> {
        self.seen_sandbox_ids
            .lock()
            .unwrap()
            .push(ctx.run.sandbox_id.clone().unwrap_or_default());
        Ok(self.endpoint.clone())
    }
}

#[async_trait]
impl WorkspaceChannelTransport for RecordingChannelTransport {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value> {
        let response = match request.op {
            "fs.read" => json!({
                "text": self.files.lock().unwrap().get(&request.path).cloned()
                    .unwrap_or_else(|| format!("remote:{}", request.path))
            }),
            "fs.write" => {
                let text = request.payload["text"].as_str().unwrap().to_string();
                self.files
                    .lock()
                    .unwrap()
                    .insert(request.path.clone(), text.clone());
                json!({ "bytes": text.len() })
            }
            "fs.list" => json!({
                "entries": [
                    { "name": "child.md", "kind": "file" }
                ]
            }),
            "fs.stat" => {
                if self.files.lock().unwrap().contains_key(&request.path) {
                    json!({ "kind": "file" })
                } else if request.path.ends_with("pnpm-lock.yaml")
                    || request.path.ends_with("package-lock.json")
                    || request.path.ends_with("new-channel.md")
                    || request.path.ends_with("state/read-tracking.json")
                {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "not found"));
                } else if request.path.ends_with("project") {
                    json!({ "kind": "dir" })
                } else {
                    json!({ "kind": "file" })
                }
            }
            "fs.copyDir" => json!({ "copied": true }),
            "fs.removeFile" | "fs.removeDirAll" => {
                self.files.lock().unwrap().remove(&request.path);
                json!({ "deleted": true })
            }
            "process.exec" => json!({
                "status": 0,
                "success": true,
                "stdout": format!(
                    "ran:{}@{}",
                    request.payload["argv"][0].as_str().unwrap_or(""),
                    request.path
                ),
                "stderr": ""
            }),
            "process.start" => json!({
                "leaseId": request.payload["leaseId"],
                "status": "running",
                "pid": 4321,
                "exitCode": Value::Null,
                "stdout": "",
                "stderr": ""
            }),
            "process.status" => json!({
                "leaseId": request.payload["leaseId"],
                "status": "running",
                "pid": 4321,
                "exitCode": Value::Null,
                "stdout": "",
                "stderr": ""
            }),
            "process.stop" => json!({
                "leaseId": request.payload["leaseId"],
                "status": "stopped",
                "pid": 4321,
                "exitCode": 0,
                "stdout": "",
                "stderr": ""
            }),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unexpected op: {other}"),
                ))
            }
        };
        self.requests.lock().unwrap().push(request);
        Ok(response)
    }
}

#[async_trait]
impl WorkspaceChannelTransport for ExecBehaviorTransport {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value> {
        self.requests.lock().unwrap().push(request.clone());
        if request.op != "process.exec" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected op: {}", request.op),
            ));
        }
        match &self.behavior {
            ExecBehavior::Error(kind) => Err(io::Error::new(*kind, "synthetic exec failure")),
            ExecBehavior::Output {
                status,
                success,
                stdout,
                stderr,
            } => Ok(json!({
                "status": status,
                "success": success,
                "stdout": stdout,
                "stderr": stderr
            })),
        }
    }
}

impl RecordingWorkspaceBackend {
    fn new(read_text: impl Into<String>) -> Self {
        Self {
            read_text: read_text.into(),
            reads: Arc::new(Mutex::new(Vec::new())),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl WorkspaceBackend for RecordingWorkspaceBackend {
    async fn read_to_string(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
    ) -> io::Result<String> {
        self.reads.lock().unwrap().push(path.to_path_buf());
        Ok(self.read_text.clone())
    }

    async fn write_string(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
        text: &str,
    ) -> io::Result<()> {
        self.writes
            .lock()
            .unwrap()
            .push((path.to_path_buf(), text.to_string()));
        Ok(())
    }

    async fn list_dir(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
    ) -> io::Result<Vec<WorkspaceEntry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            entries.push(WorkspaceEntry {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().to_string(),
                kind: if metadata.is_dir() {
                    WorkspaceEntryKind::Dir
                } else {
                    WorkspaceEntryKind::File
                },
            });
        }
        Ok(entries)
    }

    async fn path_kind(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
    ) -> io::Result<WorkspacePathKind> {
        let metadata = fs::metadata(path)?;
        Ok(if metadata.is_dir() {
            WorkspacePathKind::Dir
        } else {
            WorkspacePathKind::File
        })
    }

    async fn remove_file(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
    ) -> io::Result<()> {
        fs::remove_file(path)
    }

    async fn remove_dir_all(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        path: &Path,
    ) -> io::Result<()> {
        fs::remove_dir_all(path)
    }

    async fn copy_dir_all(
        &self,
        _ctx: &anydesign_runtime::tools::runtime::ToolContext,
        from: &Path,
        to: &Path,
        skip_dir_names: &[String],
    ) -> io::Result<()> {
        fn copy(from: &Path, to: &Path, skip_dir_names: &[String]) -> io::Result<()> {
            fs::create_dir_all(to)?;
            for entry in fs::read_dir(from)? {
                let entry = entry?;
                let name = entry.file_name();
                if entry.path().is_dir()
                    && skip_dir_names
                        .iter()
                        .any(|skip| name.to_string_lossy() == skip.as_str())
                {
                    continue;
                }
                let target = to.join(&name);
                if entry.path().is_dir() {
                    copy(&entry.path(), &target, skip_dir_names)?;
                } else {
                    fs::copy(entry.path(), target)?;
                }
            }
            Ok(())
        }
        copy(from, to, skip_dir_names)
    }
}

#[tokio::test]
async fn fs_read_write_list_and_search_are_workspace_bounded() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor_local_e2e(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "fs.write",
                    json!({ "path": "project/new.md", "text": "hello runtime" }),
                ),
                ToolCall::new("tool-2", "fs.read", json!({ "path": "project/new.md" })),
                ToolCall::new("tool-3", "fs.list", json!({ "path": "project" })),
                ToolCall::new(
                    "tool-4",
                    "fs.search",
                    json!({ "path": "project", "query": "runtime" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 4);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "workspace backend tool failures: {results:#?}"
    );
    assert_eq!(results[1].result.content["text"], "hello runtime");
    assert!(results[2].result.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "new.md"));
    assert_eq!(results[3].result.content["matches"][0]["line"], 1);
    assert_eq!(store.audit_records().await.len(), 4);
}

#[tokio::test]
async fn fs_search_skips_generated_dependency_trees_and_reports_bounds() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/node_modules/dependency")).unwrap();
    fs::create_dir_all(workspace.join("project/.next/server")).unwrap();
    fs::write(
        workspace.join("project/node_modules/dependency/index.js"),
        "needle should not be searched",
    )
    .unwrap();
    fs::write(
        workspace.join("project/.next/server/page.js"),
        "needle should not be searched",
    )
    .unwrap();
    fs::write(
        workspace.join("project/source.md"),
        "needle should be returned",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor_local_e2e(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-search-bounded",
                "fs.search",
                json!({ "path": "project", "query": "needle" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        results[0].result.content["matches"][0]["path"],
        "/workspace/project/source.md"
    );
    assert!(results[0].result.content["skippedPaths"].as_u64().unwrap() >= 2);
    assert_eq!(results[0].result.content["truncated"], false);
}

#[tokio::test]
async fn design_context_gate_requires_indexed_source_reads_before_mutation() {
    let workspace = setup_workspace();
    let inputs = workspace.join("inputs");
    fs::create_dir_all(&inputs).unwrap();
    for (name, content) in [
        ("brief.md", "# Brief\n"),
        ("design.md", "# Design Capsule\n"),
        ("design-profile.json", "{}\n"),
        ("design-source-index.json", "{}\n"),
    ] {
        fs::write(inputs.join(name), content).unwrap();
    }

    let mut source = b"# Required\nUse violet.\n## Optional\n".to_vec();
    source.extend(vec![b'x'; 40 * 1024]);
    let store = RuntimeStore::new();
    let artifact = store
        .create_design_source_artifact(
            json!({ "projectId": "project-1" }),
            "DESIGN.md".to_string(),
            "text/markdown".to_string(),
            source.clone(),
        )
        .await
        .unwrap();
    let profile = imported_design_profile(&artifact.id, &artifact.sha256);
    store.create_design_profile(profile.clone()).await.unwrap();
    let run_id = create_run(&store).await;
    store
        .attach_run_effective_design_profile(&run_id, &profile, Some("website"), Some("next-app"))
        .await
        .unwrap();
    store
        .configure_run_design_fidelity(&run_id, &profile, Some("source_fallback"))
        .await
        .unwrap();
    let optional_start = source
        .windows("## Optional".len())
        .position(|window| window == b"## Optional")
        .unwrap();
    let required = DesignSourceIndexSection {
        id: "section-1-required".to_string(),
        heading: "Required".to_string(),
        start_byte: 0,
        end_byte: optional_start,
        sha256: sha256_hex(&source[..optional_start]),
        purpose: vec!["token-evidence".to_string()],
        priority: "required".to_string(),
        recipe_ids: Vec::new(),
        required_by_rule_ids: vec!["required-source".to_string()],
    };
    let optional = DesignSourceIndexSection {
        id: "section-2-optional".to_string(),
        heading: "Optional".to_string(),
        start_byte: optional_start,
        end_byte: source.len(),
        sha256: sha256_hex(&source[optional_start..]),
        purpose: vec!["visual-reference".to_string()],
        priority: "optional".to_string(),
        recipe_ids: Vec::new(),
        required_by_rule_ids: Vec::new(),
    };
    store
        .set_run_design_source_index(
            &run_id,
            &DesignSourceIndex {
                source_artifact_id: artifact.id.clone(),
                source_hash: artifact.sha256.clone(),
                size_bytes: source.len() as u64,
                profile_hash: profile.stable_hash(),
                capsule_hash: "b".repeat(64),
                sections: vec![required.clone(), optional],
            },
            vec![required.id.clone()],
        )
        .await
        .unwrap();
    let executor = sandbox_executor_local_e2e(&workspace);

    let blocked = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "gate-blocked",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert_error_kind(&blocked[0].result, "design_context.read_required");

    for (index, path) in [
        "inputs/brief.md",
        "inputs/design.md",
        "inputs/design-profile.json",
        "inputs/design-source-index.json",
    ]
    .iter()
    .enumerate()
    {
        let read = executor
            .execute_calls(
                store.clone(),
                &run_id,
                vec![ToolCall::new(
                    format!("read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )],
            )
            .await;
        assert!(!read[0].result.is_error, "failed to read {path}");
    }

    let wrong_hash = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "source-wrong-hash",
                "design_source.read_sections",
                json!({
                    "sourceArtifactId": artifact.id,
                    "sectionIds": [required.id],
                    "expectedSourceHash": "0".repeat(64)
                }),
            )],
        )
        .await;
    assert_error_kind(&wrong_hash[0].result, "design_source.snapshot_mismatch");

    let source_read = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "source-read",
                "design_source.read_sections",
                json!({
                    "sourceArtifactId": artifact.id,
                    "sectionIds": [required.id],
                    "expectedSourceHash": artifact.sha256
                }),
            )],
        )
        .await;
    assert!(!source_read[0].result.is_error);
    assert_eq!(
        source_read[0].result.content["trustLabel"],
        "untrusted_design_reference"
    );
    assert_eq!(
        source_read[0].result.content["sections"][0]["text"],
        "# Required\nUse violet.\n"
    );

    let allowed = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "gate-allowed",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert_ne!(
        allowed[0]
            .result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("errorKind"))
            .and_then(Value::as_str),
        Some("design_context.read_required")
    );
    let run = store.get_run(&run_id).await.unwrap();
    assert!(run
        .design_source_read_section_hashes
        .contains(&required.sha256));

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn fs_read_directory_failure_has_structured_metadata() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/subdir")).unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-read-dir",
                "fs.read",
                json!({ "path": "project/subdir" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "fs.read_failed");
    assert!(tool_result_error_text(&results[0].result).contains("/workspace/project/subdir"));
}

#[tokio::test]
async fn fs_read_rejects_full_validation_report_but_allows_bounded_repair_context() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("state/validation-report.json"),
        json!({ "candidateManifest": { "files": vec!["large"; 100] } }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/repair-context.json"),
        json!({ "schemaVersion": "generation-repair-context@1", "targetFiles": ["project/app/page.tsx"] }).to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "read-repair-context",
                    "fs.read",
                    json!({ "path": "state/repair-context.json" }),
                ),
                ToolCall::new(
                    "read-full-validation",
                    "fs.read",
                    json!({ "path": "state/validation-report.json" }),
                ),
            ],
        )
        .await;

    assert!(!results[0].result.is_error);
    assert!(results[0].result.content["text"]
        .as_str()
        .unwrap()
        .contains("generation-repair-context@1"));
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "generation.repair_context_required");
}

#[tokio::test]
async fn fs_list_missing_directory_failure_has_structured_metadata() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor_local_e2e(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-list-missing-dir",
                "fs.list",
                json!({ "path": "project/missing-dir" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "fs.list_failed");
    assert!(tool_result_error_text(&results[0].result).contains("/workspace/project/missing-dir"));
}

#[tokio::test]
async fn fs_read_and_write_execute_through_workspace_backend() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/existing.md"), "local text").unwrap();
    let backend = RecordingWorkspaceBackend::new("remote channel text");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_workspace_backend(Arc::new(backend.clone())),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "fs.read",
                    json!({ "path": "project/existing.md" }),
                ),
                ToolCall::new(
                    "tool-2",
                    "fs.write",
                    json!({ "path": "project/generated.md", "text": "written through backend" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "workspace backend tool failures: {results:#?}"
    );
    assert_eq!(results[0].result.content["text"], "remote channel text");
    assert!(backend.reads.lock().unwrap()[0].ends_with("project/existing.md"));
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 3);
    assert!(writes
        .iter()
        .any(|(path, _)| path.ends_with("state/read-tracking.json")));
    let generated = writes
        .iter()
        .find(|(path, _)| path.ends_with("project/generated.md"))
        .unwrap();
    assert_eq!(generated.1, "written through backend");
    assert!(!workspace.join("project/generated.md").exists());
}

#[tokio::test]
async fn remote_fs_tools_do_not_require_the_host_workspace_to_exist() {
    let host_workspace = unique_temp_dir("remote-host-workspace-absent").join("missing-root");
    assert!(!host_workspace.exists());
    let backend = RecordingWorkspaceBackend::new("remote-only text");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(
            sandbox_tools_with_workspace_backend(Arc::new(backend.clone())),
            Default::default(),
            &host_workspace,
        )
        .with_remote_workspace(true),
    );

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "remote-read",
                    "fs.read",
                    json!({ "path": "project/remote-only.md" }),
                ),
                ToolCall::new(
                    "remote-write",
                    "fs.write",
                    json!({ "path": "project/generated.md", "text": "remote write" }),
                ),
            ],
        )
        .await;

    assert!(
        results.iter().all(|result| !result.result.is_error),
        "remote tools must be lexical and backend-driven: {results:#?}"
    );
    assert_eq!(results[0].result.content["text"], "remote-only text");
    assert!(backend
        .writes
        .lock()
        .unwrap()
        .iter()
        .any(|(path, text)| path.ends_with("project/generated.md") && text == "remote write"));
    assert!(!host_workspace.exists());
}

#[tokio::test]
async fn fs_write_accepts_direct_payloads_at_budget_limit() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let text_at_limit = "x".repeat(48_000);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-direct-write-limit",
                "fs.write",
                json!({ "path": "project/direct-limit.md", "text": text_at_limit }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert_eq!(
        fs::read_to_string(workspace.join("project/direct-limit.md"))
            .unwrap()
            .len(),
        48_000
    );
}

#[tokio::test]
async fn fs_write_rejects_direct_payloads_over_budget() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let oversized_text = "x".repeat(48_001);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-large-write",
                "fs.write",
                json!({ "path": "project/large.md", "text": oversized_text }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    let result = &results[0].result;
    assert!(result.is_error);
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("errorKind"))
            .and_then(Value::as_str),
        Some("tool.input_too_large")
    );
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("recoverable"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(!workspace.join("project/large.md").exists());
}

#[tokio::test]
async fn fs_write_rejects_serialized_arguments_over_budget_even_when_text_chars_fit() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let escaped_heavy_text = "\"".repeat(47_999);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-large-serialized-write",
                "fs.write",
                json!({ "path": "project/escaped.md", "text": escaped_heavy_text }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    let result = &results[0].result;
    assert!(result.is_error);
    let metadata = result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("errorKind").and_then(Value::as_str),
        Some("tool.input_too_large")
    );
    assert_eq!(
        metadata.get("path").and_then(Value::as_str),
        Some("project/escaped.md")
    );
    assert!(
        metadata
            .get("serializedBytes")
            .and_then(Value::as_u64)
            .unwrap()
            > 96_000
    );
    assert!(!workspace.join("project/escaped.md").exists());
}

#[tokio::test]
async fn fs_write_chunk_and_commit_chunks_create_large_file() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-chunk-0",
                    "fs.write_chunk",
                    json!({
                        "path": "project/chunked.md",
                        "sessionId": "chunked-test",
                        "index": 0,
                        "total": 2,
                        "text": "alpha\n",
                    }),
                ),
                ToolCall::new(
                    "tool-chunk-1",
                    "fs.write_chunk",
                    json!({
                        "path": "project/chunked.md",
                        "sessionId": "chunked-test",
                        "index": 1,
                        "total": 2,
                        "text": "beta\n",
                    }),
                ),
                ToolCall::new(
                    "tool-commit",
                    "fs.commit_chunks",
                    json!({
                        "path": "project/chunked.md",
                        "sessionId": "chunked-test",
                        "total": 2,
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(
        fs::read_to_string(workspace.join("project/chunked.md")).unwrap(),
        "alpha\nbeta\n"
    );
    let events = store.events(&run_id).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ChunkReceived { .. }))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ChunkCommitted {
            path,
            session_id,
            total: 2,
            ..
        } if path == "/workspace/project/chunked.md" && session_id == "chunked-test"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::MetricRecorded { name, .. }
                    if name == "tool_chunk_write_started"
            ))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "tool_chunk_write_committed"
            && metadata.get("path").and_then(Value::as_str)
                == Some("/workspace/project/chunked.md")
            && metadata.get("sessionId").and_then(Value::as_str) == Some("chunked-test")
    )));
    let health = serde_json::from_str::<Value>(
        &fs::read_to_string(workspace.join("state/run-health.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        health["chunkWrites"][0]["path"],
        "/workspace/project/chunked.md"
    );
    assert_eq!(health["chunkWrites"][0]["total"], 2);
    assert_eq!(health["chunkWrites"][0]["bytes"], "alpha\nbeta\n".len());
    assert_eq!(health["chunkWrites"][0]["status"], "committed");
    assert!(!workspace
        .join("outputs/staged-writes/chunked-test")
        .exists());
}

#[tokio::test]
async fn runtime_checkpoint_chunk_commit_does_not_require_a_draft_edit_base() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .draft_preview_store()
        .start(StartDraftPreview {
            project_id: "project-1".to_string(),
            sandbox_binding_id: "sandbox-binding-1".to_string(),
            template_id: "next-app".to_string(),
            base_snapshot_id: "draft-snapshot-1".to_string(),
            base_version_id: None,
            proxy_url: "https://runtime.test/previews/lease-1/".to_string(),
            writer_ttl_seconds: 120,
        })
        .unwrap();
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run.id,
            vec![
                ToolCall::new(
                    "bootstrap:state/context.md:chunk:0",
                    "fs.write_chunk",
                    json!({
                        "path": "state/context.md",
                        "sessionId": "checkpoint-test",
                        "index": 0,
                        "total": 2,
                        "text": "runtime ",
                    }),
                ),
                ToolCall::new(
                    "bootstrap:state/context.md:chunk:1",
                    "fs.write_chunk",
                    json!({
                        "path": "state/context.md",
                        "sessionId": "checkpoint-test",
                        "index": 1,
                        "total": 2,
                        "text": "checkpoint",
                    }),
                ),
                ToolCall::new(
                    "bootstrap:state/context.md:commit",
                    "fs.commit_chunks",
                    json!({
                        "path": "state/context.md",
                        "sessionId": "checkpoint-test",
                        "total": 2,
                    }),
                ),
            ],
        )
        .await;

    assert!(
        results.iter().all(|result| !result.result.is_error),
        "checkpoint results: {results:#?}"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("state/context.md")).unwrap(),
        "runtime checkpoint"
    );

    let source_mutation = executor
        .execute_calls(
            store,
            &run.id,
            vec![ToolCall::new(
                "model-source-write",
                "fs.write",
                json!({ "path": "project/page.tsx", "text": "export default function Page() {}" }),
            )],
        )
        .await;
    assert!(source_mutation[0].result.is_error);
    assert_error_kind(&source_mutation[0].result, "edit.base_stale");
}

#[tokio::test]
async fn fs_write_chunk_rejects_duplicate_chunk_index() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-chunk-0",
                    "fs.write_chunk",
                    json!({
                        "path": "project/chunked.md",
                        "sessionId": "duplicate-test",
                        "index": 0,
                        "total": 1,
                        "text": "alpha\n",
                    }),
                ),
                ToolCall::new(
                    "tool-chunk-0-again",
                    "fs.write_chunk",
                    json!({
                        "path": "project/chunked.md",
                        "sessionId": "duplicate-test",
                        "index": 0,
                        "total": 1,
                        "text": "alpha again\n",
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert!(results[1].result.is_error);
    assert!(tool_result_error_text(&results[1].result).contains("duplicate chunk"));
}

#[tokio::test]
async fn fs_commit_chunks_rejects_missing_chunk_without_writing_target() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-chunk-0",
                    "fs.write_chunk",
                    json!({
                        "path": "project/missing-chunk.md",
                        "sessionId": "missing-chunk-test",
                        "index": 0,
                        "total": 2,
                        "text": "alpha\n",
                    }),
                ),
                ToolCall::new(
                    "tool-commit-missing",
                    "fs.commit_chunks",
                    json!({
                        "path": "project/missing-chunk.md",
                        "sessionId": "missing-chunk-test",
                        "total": 2,
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert!(results[1].result.is_error);
    assert!(tool_result_error_text(&results[1].result).contains("missing chunk 1/2"));
    assert!(!workspace.join("project/missing-chunk.md").exists());
}

#[tokio::test]
async fn fs_commit_chunks_rejects_nested_package_root() {
    let workspace = setup_workspace();
    let session_dir = workspace.join("outputs/staged-writes/nested-package-test");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("chunk-00000.txt"),
        "{\"type\":\"module\"}\n",
    )
    .unwrap();
    fs::write(
        session_dir.join("manifest.json"),
        json!({
            "sessionId": "nested-package-test",
            "runId": "run-1",
            "path": "/workspace/project/src/package.json",
            "total": 1,
            "chunks": [0],
            "createdAt": "2026-07-07T00:00:00Z",
            "updatedAt": "2026-07-07T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-commit-nested-package",
                "fs.commit_chunks",
                json!({
                    "path": "project/src/package.json",
                    "sessionId": "nested-package-test",
                    "total": 1,
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("nested package root denied"));
    assert!(!workspace.join("project/src/package.json").exists());
}

#[tokio::test]
async fn fs_commit_chunks_supports_create_and_append_modes() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/existing.md"), "base\n").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let create_refused = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-create-chunk",
                    "fs.write_chunk",
                    json!({
                        "path": "project/existing.md",
                        "sessionId": "create-refused",
                        "index": 0,
                        "total": 1,
                        "text": "new\n",
                    }),
                ),
                ToolCall::new(
                    "tool-create-commit",
                    "fs.commit_chunks",
                    json!({
                        "path": "project/existing.md",
                        "sessionId": "create-refused",
                        "total": 1,
                        "mode": "create",
                    }),
                ),
            ],
        )
        .await;
    assert!(!create_refused[0].result.is_error);
    assert!(create_refused[1].result.is_error);
    assert_eq!(
        fs::read_to_string(workspace.join("project/existing.md")).unwrap(),
        "base\n"
    );

    let appended = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-append-read",
                    "fs.read",
                    json!({ "path": "project/existing.md" }),
                ),
                ToolCall::new(
                    "tool-append-chunk",
                    "fs.write_chunk",
                    json!({
                        "path": "project/existing.md",
                        "sessionId": "append-ok",
                        "index": 0,
                        "total": 1,
                        "text": "tail\n",
                    }),
                ),
                ToolCall::new(
                    "tool-append-commit",
                    "fs.commit_chunks",
                    json!({
                        "path": "project/existing.md",
                        "sessionId": "append-ok",
                        "total": 1,
                        "mode": "append",
                    }),
                ),
            ],
        )
        .await;
    assert!(appended.iter().all(|result| !result.result.is_error));
    assert_eq!(
        fs::read_to_string(workspace.join("project/existing.md")).unwrap(),
        "base\ntail\n"
    );
    assert_eq!(appended[2].result.content["mode"], "append");
}

#[tokio::test]
async fn fs_write_chunk_cleans_expired_staged_sessions() {
    let workspace = setup_workspace();
    let expired_dir = workspace.join("outputs/staged-writes/expired-session");
    fs::create_dir_all(&expired_dir).unwrap();
    fs::write(expired_dir.join("chunk-00000.txt"), "stale").unwrap();
    fs::write(
        expired_dir.join("manifest.json"),
        json!({
            "sessionId": "expired-session",
            "runId": "old-run",
            "path": "/workspace/project/stale.md",
            "total": 1,
            "chunks": [0],
            "createdAt": "2000-01-01T00:00:00Z",
            "updatedAt": "2000-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-chunk",
                "fs.write_chunk",
                json!({
                    "path": "project/fresh.md",
                    "sessionId": "fresh-session",
                    "index": 0,
                    "total": 1,
                    "text": "fresh\n",
                }),
            )],
        )
        .await;

    assert!(!results[0].result.is_error);
    assert!(!expired_dir.exists());
    assert!(workspace
        .join("outputs/staged-writes/fresh-session/manifest.json")
        .exists());
}

#[tokio::test]
async fn fs_write_chunk_rejects_chunk_payloads_over_budget() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let oversized_text = "x".repeat(24_001);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-large-chunk",
                "fs.write_chunk",
                json!({
                    "path": "project/chunked.md",
                    "sessionId": "chunked-test",
                    "index": 0,
                    "total": 1,
                    "text": oversized_text,
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    let result = &results[0].result;
    assert!(result.is_error);
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("errorKind"))
            .and_then(Value::as_str),
        Some("tool.input_too_large")
    );
    assert!(!workspace
        .join("outputs/staged-writes/chunked-test")
        .exists());
}

#[tokio::test]
async fn project_write_page_renders_next_app_route() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-page",
                "project.write_page",
                json!({
                    "route": "/pricing",
                    "title": "Runtime Plans",
                    "styleProfile": "saas",
                    "sections": [
                        {
                            "kind": "hero",
                            "heading": "Launch sites with controlled runtime tools",
                            "body": "Plan, generate, build, and promote previews without oversized tool-call payloads.",
                            "visual": "Build pipeline"
                        },
                        {
                            "kind": "proof",
                            "heading": "Observable by default",
                            "body": "Streams show chunk progress and input recovery guidance.",
                            "visual": "SSE events"
                        }
                    ]
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert_eq!(
        results[0].result.content["path"],
        "/workspace/project/app/pricing/page.tsx"
    );
    let page = fs::read_to_string(workspace.join("project/app/pricing/page.tsx")).unwrap();
    assert!(page.contains("Runtime Plans"));
    assert!(page.contains("export default function Page"));
    assert!(page.contains("className=\"mx-auto min-h-svh max-w-6xl px-6 py-20\""));
}

#[tokio::test]
async fn project_write_page_root_route_overwrites_existing_index_page() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/app")).unwrap();
    fs::write(
        workspace.join("project/app/page.tsx"),
        "<main>old page</main>\n",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-page-root-read",
                    "fs.read",
                    json!({ "path": "project/app/page.tsx" }),
                ),
                ToolCall::new(
                    "tool-page-root",
                    "project.write_page",
                    json!({
                        "route": "/",
                        "title": "Root Runtime Page",
                        "styleProfile": "saas",
                        "sections": [
                            {
                                "kind": "hero",
                                "heading": "Root route is writable",
                                "body": "project.write_page must overwrite app/page.tsx for /.",
                                "visual": "Index page"
                            }
                        ]
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        !results[1].result.is_error,
        "{}",
        tool_result_error_text(&results[1].result)
    );
    assert_eq!(
        results[1].result.content["path"],
        "/workspace/project/app/page.tsx"
    );
    let page_path = workspace.join("project/app/page.tsx");
    assert!(page_path.is_file());
    let page = fs::read_to_string(page_path).unwrap();
    assert!(page.contains("Root Runtime Page"));
    assert!(page.contains("export default function Page"));
    assert!(page.contains("className=\"text-5xl font-semibold tracking-tight\""));
    assert!(!page.contains("<style>"));
    assert!(!page.contains("old page"));
}

#[tokio::test]
async fn project_init_next_app_seeds_react_contract_and_protects_static_export_files() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-next-app",
                    "project.init",
                    json!({ "template": "next-app" }),
                ),
                ToolCall::new(
                    "tool-protected-next-config",
                    "fs.write",
                    json!({
                        "path": "project/next.config.mjs",
                        "text": "export default {}"
                    }),
                ),
                ToolCall::new(
                    "tool-forbidden-api-route",
                    "fs.write",
                    json!({
                        "path": "project/app/api/users/route.ts",
                        "text": "export function GET() { return new Response('no'); }"
                    }),
                ),
                ToolCall::new(
                    "tool-read-next-config",
                    "fs.read",
                    json!({ "path": "project/next.config.mjs" }),
                ),
                ToolCall::new(
                    "tool-patch-next-config",
                    "fs.patch",
                    json!({
                        "path": "project/next.config.mjs",
                        "oldStr": "output: \"export\"",
                        "newStr": "output: undefined"
                    }),
                ),
                ToolCall::new(
                    "tool-forbidden-server-action",
                    "fs.write",
                    json!({
                        "path": "project/app/admin/page.tsx",
                        "text": "\"use server\"; export default function Page() { return null; }"
                    }),
                ),
                ToolCall::new(
                    "tool-next-route",
                    "project.write_page",
                    json!({
                        "route": "/about/team",
                        "title": "Meet the team",
                        "styleProfile": "editorial",
                        "sections": [{
                            "kind": "content",
                            "heading": "Meet the team",
                            "body": "A small team building visual tools."
                        }]
                    }),
                ),
                ToolCall::new(
                    "tool-denied-next-dependency",
                    "project.ensure_dependencies",
                    json!({
                        "mode": "add",
                        "packages": ["express@5.1.0"]
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 8);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["template"], "next-app");
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "template.protected_contract_mutation");
    assert!(results[2].result.is_error);
    assert_error_kind(&results[2].result, "template.protected_contract_mutation");
    assert!(!results[3].result.is_error);
    assert!(results[4].result.is_error);
    assert_error_kind(&results[4].result, "template.protected_contract_mutation");
    assert!(results[5].result.is_error);
    assert_error_kind(&results[5].result, "template.static_export_forbidden");
    assert!(
        !results[6].result.is_error,
        "{}",
        tool_result_error_text(&results[6].result)
    );
    assert_eq!(
        results[6].result.content["path"],
        "/workspace/project/app/about/team/page.tsx"
    );
    assert!(results[7].result.is_error);
    assert_error_kind(&results[7].result, "dependency.not_in_catalog");

    let package: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("project/package.json")).unwrap())
            .unwrap();
    assert_eq!(package["dependencies"]["@base-ui/react"], "1.6.0");
    assert_eq!(package["dependencies"]["next"], "16.2.6");
    assert!(workspace.join("project/components/ui/dialog.tsx").is_file());
    assert!(workspace
        .join("project/components/ui/dropdown-menu.tsx")
        .is_file());
    assert!(workspace.join("project/components/ui/select.tsx").is_file());
    assert!(workspace.join("project/app/about/team/page.tsx").is_file());
    assert!(!workspace.join("project/app/api/users/route.ts").exists());
    assert!(!workspace.join("project/app/admin/page.tsx").exists());

    let config = fs::read_to_string(workspace.join("project/next.config.mjs")).unwrap();
    assert!(config.contains("output: \"export\""));
    let contract: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("state/style-contract.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(contract["template"], "next-app");
    assert_eq!(contract["tokenFile"], "/workspace/project/app/tokens.css");
    assert_eq!(contract["tokens"]["color.primary"], "--primary");
    assert_eq!(contract["tailwind"]["version"], "4");
}

#[tokio::test]
async fn generation_context_project_init_delivers_bounded_sources_and_authorizes_next_turn_write() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    store
        .set_run_generation_context_runtime_mode(&run_id, "enabled")
        .await
        .unwrap();
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), &workspace)
            .with_observation_receipts_enabled(true),
    );

    let init = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-with-source-context",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;

    assert_eq!(init.len(), 1);
    assert!(
        !init[0].result.is_error,
        "{}",
        tool_result_error_text(&init[0].result)
    );
    let observations = init[0].result.content["sourceObservations"]
        .as_array()
        .expect("source observations");
    assert!(!observations.is_empty());
    assert!(
        observations
            .iter()
            .map(|observation| observation["bytes"].as_u64().unwrap_or_default())
            .sum::<u64>()
            <= 24 * 1024
    );
    let page = observations
        .iter()
        .find(|observation| observation["path"] == "/workspace/project/app/page.tsx")
        .expect("primary route observation");
    assert_eq!(page["view"], "full");
    assert_eq!(page["purpose"], "source");
    assert_eq!(
        page["contentSha256"],
        sha256_hex(
            page["text"]
                .as_str()
                .expect("primary route source")
                .as_bytes()
        )
    );
    let receipts = store
        .events(&run_id)
        .await
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::ObservationReceipt { receipt, .. } => Some(receipt),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), observations.len());
    assert!(receipts.iter().all(|receipt| {
        receipt.view == ObservationView::Full
            && receipt.last_outcome == ObservationOutcome::ContentReturned
    }));
    assert!(receipts.iter().any(|receipt| {
        receipt.normalized_path == "project/app/page.tsx"
            && receipt.content_sha256 == page["contentSha256"]
    }));

    let replacement = format!(
        "{}\n// source observation lease verified\n",
        page["text"].as_str().unwrap()
    );
    let write = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-write-from-init-observation",
                "fs.write",
                json!({
                    "path": "project/app/page.tsx",
                    "text": replacement,
                }),
            )],
        )
        .await;

    assert_eq!(write.len(), 1);
    assert!(
        !write[0].result.is_error,
        "{}",
        tool_result_error_text(&write[0].result)
    );
    assert!(fs::read_to_string(workspace.join("project/app/page.tsx"))
        .unwrap()
        .contains("source observation lease verified"));
}

#[tokio::test]
async fn next_app_completes_with_draft_snapshot_without_creating_work_version() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let initialized = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-next-draft",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!initialized[0].result.is_error);

    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        serde_json::to_vec_pretty(&json!({
            "status": "success",
            "success": true,
            "sourceSnapshotUri": "runtime://source-snapshots/project-1/next-draft-1",
            "sourceFingerprint": "a".repeat(64)
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        serde_json::to_vec_pretty(&json!({
            "status": "running",
            "accessible": true,
            "url": "http://runtime.local/previews/next-draft/"
        }))
        .unwrap(),
    )
    .unwrap();

    let rejected_legacy_publish = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-next-legacy-publish",
                "preview.publish",
                json!({}),
            )],
        )
        .await;
    assert!(rejected_legacy_publish[0].result.is_error);
    assert_error_kind(
        &rejected_legacy_publish[0].result,
        "template.operation_unsupported",
    );

    let snapshot_result = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-next-draft-snapshot",
                "draft.snapshot_create",
                json!({}),
            )],
        )
        .await;
    assert!(
        !snapshot_result[0].result.is_error,
        "{}",
        tool_result_error_text(&snapshot_result[0].result)
    );
    assert_eq!(
        snapshot_result[0].result.content["visualReview"]["status"],
        "not_requested"
    );
    let snapshot_id = snapshot_result[0].result.content["draftSnapshot"]["snapshotId"]
        .as_str()
        .unwrap();
    assert!(store.get_draft_snapshot(snapshot_id).await.is_some());
    assert!(store.list_project_versions("project-1").await.is_empty());

    let completed = StreamingToolExecutor::new(
        control_plane_executor()
            .with_workspace_root(&workspace)
            .with_runtime_storage_dir(workspace.join(".runtime-storage")),
    )
    .execute_calls(
        store.clone(),
        &run_id,
        vec![ToolCall::new(
            "tool-complete-next-draft",
            "run.complete",
            json!({ "status": "completed", "summary": "Draft is ready." }),
        )],
    )
    .await;
    assert!(
        !completed[0].result.is_error,
        "{}",
        tool_result_error_text(&completed[0].result)
    );
    assert_eq!(completed[0].result.content["status"], "completed");
    assert!(store.current_project_version("project-1").await.is_none());
    assert!(store.list_project_versions("project-1").await.is_empty());
}

#[tokio::test]
async fn project_init_applies_design_profile_runtime_token_mapping_on_first_build() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("inputs")).unwrap();
    fs::write(
        workspace.join("inputs/design-profile.json"),
        serde_json::to_string_pretty(&json!({
            "id": "design-profile-1",
            "version": 1,
            "runtimeTokenMapping": {
                "color.background": "#101820",
                "color.surface": "#ffffff",
                "color.surfaceStrong": "#eef2ff",
                "color.text": "#f8fafc",
                "color.muted": "#94a3b8",
                "color.primary": "#f37a0a",
                "color.primaryContrast": "#111827",
                "color.border": "#334155",
                "radius.card": "6px",
                "radius.control": "4px",
                "font.sans": "Inter, sans-serif",
                "shadow.soft": "0 8px 24px rgba(15, 23, 42, 0.18)"
            },
            "extendedTokenMapping": {
                "font.display": "Space Grotesk, sans-serif",
                "spacing.pageGutter": "48px",
                "gradient.display": "linear-gradient(90deg, #d8ecf8, #98c0ef)"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-init-website",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    let tokens = fs::read_to_string(workspace.join("project/app/tokens.css")).unwrap();
    assert!(tokens.contains("--background: #101820"));
    assert!(tokens.contains("--primary: #f37a0a"));
    assert!(tokens.contains("--radius: 4px"));
    assert!(tokens.contains("--font-display: Space Grotesk, sans-serif"));
    assert!(tokens.contains("--spacing-page-gutter: 48px"));
    assert_eq!(
        results[0].result.content["designProfileTokenChanges"]
            .as_array()
            .unwrap()
            .len(),
        14
    );
}

#[tokio::test]
async fn style_update_tokens_updates_contract_backed_token_file() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-website",
                    "project.init",
                    json!({ "template": "next-app" }),
                ),
                ToolCall::new(
                    "tool-style-update",
                    "style.update_tokens",
                    json!({
                        "tokens": {
                            "color.primary": "#f37a0a",
                            "color.primaryContrast": "#111827",
                            "radius.card": "6px"
                        }
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        !results[1].result.is_error,
        "{}",
        tool_result_error_text(&results[1].result)
    );
    assert_eq!(
        results[1].result.content["tokenFile"],
        "/workspace/project/app/tokens.css"
    );
    assert_eq!(
        results[1].result.content["changes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let tokens = fs::read_to_string(workspace.join("project/app/tokens.css")).unwrap();
    assert!(tokens.contains("--primary: #f37a0a;"));
    assert!(tokens.contains("--primary-foreground: #111827;"));
    assert!(tokens.contains("--radius: 6px;"));
}

#[tokio::test]
async fn style_update_tokens_rejects_unknown_contract_tokens() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-website",
                    "project.init",
                    json!({ "template": "next-app" }),
                ),
                ToolCall::new(
                    "tool-style-update",
                    "style.update_tokens",
                    json!({
                        "tokens": {
                            "color.unknown": "#f37a0a"
                        }
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "style.token_unknown");
    let metadata = results[1].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("token").and_then(Value::as_str),
        Some("color.unknown")
    );
    assert!(tool_result_error_text(&results[1].result).contains("unknown token color.unknown"));
    let tokens = fs::read_to_string(workspace.join("project/app/tokens.css")).unwrap();
    assert!(tokens.contains("--primary: oklch(0.55 0.21 258);"));
}

#[tokio::test]
async fn style_update_tokens_requires_runtime_style_contract_metadata() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-style-update",
                "style.update_tokens",
                json!({
                    "tokens": {
                        "color.primary": "#f37a0a"
                    }
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "style.contract_missing");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("contractPath").and_then(Value::as_str),
        Some("/workspace/state/style-contract.json")
    );
}

#[tokio::test]
async fn style_update_tokens_rejects_missing_css_variable_with_metadata() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let init = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-website",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!init[0].result.is_error);
    let token_path = workspace.join("project/app/tokens.css");
    let mut tokens = fs::read_to_string(&token_path).unwrap();
    tokens = tokens.replace("--primary:", "--primary-missing:");
    fs::write(&token_path, tokens).unwrap();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-style-update",
                "style.update_tokens",
                json!({
                    "tokens": {
                        "color.primary": "#f37a0a"
                    }
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "style.token_variable_missing");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("cssVariable").and_then(Value::as_str),
        Some("--primary")
    );
}

#[tokio::test]
async fn project_inspect_returns_lifecycle_style_and_dependency_state() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "buildId": "build-1",
            "status": "success",
            "sourceSnapshotUri": "file:///workspace/outputs/build/source-snapshots/build-1"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/dependency-state.json"),
        json!({
            "needsRestore": false,
            "packageManager": "npm",
            "success": true
        })
        .to_string(),
    )
    .unwrap();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-website",
                    "project.init",
                    json!({ "template": "next-app" }),
                ),
                ToolCall::new("tool-inspect", "project.inspect", json!({})),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(
        !results[1].result.is_error,
        "{}",
        tool_result_error_text(&results[1].result)
    );
    let inspected = &results[1].result.content;
    assert_eq!(inspected["appRoot"], "/workspace/project");
    assert_eq!(inspected["packageManager"], "npm");
    assert_eq!(inspected["framework"], "nextjs");
    assert_eq!(
        inspected["styleContractPath"],
        "/workspace/state/style-contract.json"
    );
    assert_eq!(
        inspected["latestBuild"]["sourceSnapshotUri"],
        "file:///workspace/outputs/build/source-snapshots/build-1"
    );
    assert_eq!(inspected["dependencyState"]["needsRestore"], false);
    assert!(inspected["keySourceFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "/workspace/project/app/tokens.css" && file["exists"] == true));
}

#[tokio::test]
async fn runtime_project_state_is_authoritative_and_workspace_hint_is_protected() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let initialized = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "state-init",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!initialized[0].result.is_error);
    let authority = store
        .get_project_runtime_state("project-1")
        .await
        .expect("runtime project state");
    assert_eq!(authority.app_root, "project");
    assert_eq!(authority.template_key, "next-app");

    let generic_write = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "state-generic-write",
                "fs.write",
                json!({ "path": "state/project.json", "text": "{}" }),
            )],
        )
        .await;
    assert!(generic_write[0].result.is_error);
    assert_error_kind(&generic_write[0].result, "path.runtime_owned");

    fs::write(
        workspace.join("state/project.json"),
        json!({
            "appRoot": "attacker-controlled",
            "templateKey": "fumadocs-docs",
            "framework": "fumadocs",
            "packageManager": "pnpm"
        })
        .to_string(),
    )
    .unwrap();
    let conflict = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "state-inspect-conflict",
                "project.inspect",
                json!({}),
            )],
        )
        .await;
    assert!(conflict[0].result.is_error);
    assert_error_kind(&conflict[0].result, "project.state_conflict");
}

#[tokio::test]
async fn project_inspect_routes_a_fresh_workspace_with_authoritative_state_to_init() {
    let initialized_workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let initialized_executor = sandbox_executor(&initialized_workspace);
    let initialized = initialized_executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "state-init",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!initialized[0].result.is_error);

    let rebound_workspace = setup_workspace();
    let rebound_executor = sandbox_executor(&rebound_workspace);
    let inspected = rebound_executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "state-inspect-rebound-workspace",
                "project.inspect",
                json!({}),
            )],
        )
        .await;

    assert!(
        !inspected[0].result.is_error,
        "{}",
        tool_result_error_text(&inspected[0].result)
    );
    assert_eq!(inspected[0].result.content["projectHint"], Value::Null);
    assert_eq!(inspected[0].result.content["projectStateConflict"], false);
    assert_eq!(
        inspected[0].result.content["lifecycle"]["initialized"],
        false
    );
    assert_eq!(
        inspected[0].result.content["nextAction"]["tool"],
        "project.init"
    );

    let reinitialized = rebound_executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "state-reinitialize-rebound-workspace",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(
        !reinitialized[0].result.is_error,
        "{}",
        tool_result_error_text(&reinitialized[0].result)
    );
    assert!(rebound_workspace.join("state/project.json").exists());
    assert!(rebound_workspace.join("state/style-contract.json").exists());
    assert!(rebound_workspace.join("project/package.json").exists());
}

#[tokio::test]
async fn project_init_fumadocs_docs_writes_docs_source_contract() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-init-docs",
                "project.init",
                json!({ "template": "fumadocs-docs" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["template"], "fumadocs-docs");
    let package_json = fs::read_to_string(workspace.join("project/package.json")).unwrap();
    assert!(package_json.contains("fumadocs-ui"));
    assert!(package_json.contains(r#""typescript": "5.9.3""#));
    assert!(workspace.join("project/source.config.ts").exists());
    assert!(workspace.join("project/lib/source.js").exists());
    assert!(workspace
        .join("project/app/docs/[[...slug]]/page.jsx")
        .exists());
    let index_mdx = fs::read_to_string(workspace.join("project/content/docs/index.mdx")).unwrap();
    assert!(index_mdx.contains("title: Overview"));
    let meta: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("project/content/docs/meta.json")).unwrap(),
    )
    .unwrap();
    assert!(meta["pages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|page| page == "index"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/project.json")).unwrap())
            .unwrap();
    assert_eq!(state["templateKey"], "fumadocs-docs");
    assert_eq!(state["templateVersion"], "fumadocs-docs@runtime-p7");
    assert!(
        fs::read_to_string(workspace.join("project/next.config.mjs"))
            .unwrap()
            .contains("trailingSlash: true")
    );
    let global_css = fs::read_to_string(workspace.join("project/app/global.css")).unwrap();
    assert!(global_css.contains("@import 'tailwindcss'"));
    assert!(global_css.contains("@import './tokens.css'"));
    assert!(global_css.contains("var(--runtime-primary)"));
    assert!(global_css.contains("background: var(--color-fd-background)"));
    assert!(global_css.contains("color: var(--color-fd-foreground)"));
    assert!(global_css.contains(":root:not(.dark)"));
    assert!(global_css.contains("--color-fd-background: var(--runtime-bg)"));
    assert!(global_css.contains("--color-fd-foreground: var(--runtime-text)"));
    assert!(!global_css.contains("\n:root {\n  --fd-primary"));
    assert!(!global_css.contains("body {\n  background: var(--runtime-bg)"));
    assert!(!global_css.contains("\n  color: var(--runtime-text);\n  font-family"));
    assert!(workspace.join("project/app/tokens.css").exists());
    let button_component =
        fs::read_to_string(workspace.join("project/components/ui/button.jsx")).unwrap();
    assert!(button_component.contains("runtime-button"));
    assert!(button_component.contains("px-4 py-2"));
    let contract: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("state/style-contract.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(contract["template"], "fumadocs-docs");
    assert_eq!(contract["tokenFile"], "/workspace/project/app/tokens.css");
    assert_eq!(
        contract["globalCssFile"],
        "/workspace/project/app/global.css"
    );
    assert_eq!(
        contract["componentRoot"],
        "/workspace/project/components/ui"
    );
    assert_eq!(contract["tailwind"]["version"], "4");
    assert_eq!(
        contract["tailwind"]["entryImport"],
        "@import \"tailwindcss\""
    );
    assert_eq!(contract["tailwind"]["themeSource"], "css-variables");
    assert_eq!(contract["version"], "runtime-style-contract@p3");
    assert_eq!(contract["tokens"]["color.primary"], "--runtime-primary");
    assert_eq!(contract["tokens"]["color.action"], "--runtime-action");
    assert_eq!(
        contract["tokens"]["gradient.display"],
        "--runtime-gradient-display"
    );
    assert!(contract["tokens"].get("spacing.cardPadding").is_none());
}

#[tokio::test]
#[ignore = "installs npm dependencies and runs a real Next/Fumadocs production build"]
async fn fumadocs_docs_real_next_build_smoke() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-docs-smoke",
                "project.init",
                json!({ "template": "fumadocs-docs" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);

    let docs_dir = workspace.join("project/content/docs");
    fs::write(
        docs_dir.join("runtime-flow.mdx"),
        "---\ntitle: Runtime Flow\ndescription: Build and edit lifecycle\n---\n\n# Runtime Flow\n\n<Steps>\n  <Steps.Step>Initialize the project.</Steps.Step>\n  <Step>Build and publish the candidate.</Step>\n</Steps>\n\n<Tabs items={[\"CLI\", \"API\"]}>\n  <Tabs.Tab value=\"CLI\">Run the CLI.</Tabs.Tab>\n  <Tab value=\"API\">Call the API.</Tab>\n</Tabs>\n\n<Accordions type=\"single\" collapsible>\n  <Accordions.Accordion title=\"What is validated?\">Build, render, links, and accessibility.</Accordions.Accordion>\n</Accordions>\n",
    )
    .unwrap();
    fs::write(
        docs_dir.join("typed-errors.mdx"),
        "---\ntitle: Typed Errors\ndescription: Recoverable error model\n---\n\n# Typed Errors\n\nTyped metadata keeps provider recovery observable.\n",
    )
    .unwrap();
    fs::write(
        docs_dir.join("meta.json"),
        serde_json::to_string_pretty(&json!({
            "title": "AnyDesign Runtime Docs",
            "pages": ["index", "runtime-flow", "typed-errors"]
        }))
        .unwrap(),
    )
    .unwrap();

    let project_root = workspace.join("project");
    let install = tokio::time::timeout(
        Duration::from_secs(180),
        Command::new("npm")
            .args([
                "install",
                "--ignore-scripts",
                "--audit=false",
                "--fund=false",
                "--registry",
                "https://registry.npmjs.org/",
            ])
            .current_dir(&project_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .expect("npm install timed out")
    .expect("failed to run npm install");
    assert!(
        install.status.success(),
        "npm install failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );

    let build_results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-build-docs-smoke",
                "project.build",
                json!({
                    "cwd": "project",
                    "timeoutMs": 180000
                }),
            )],
        )
        .await;

    assert_eq!(build_results.len(), 1);
    assert!(
        !build_results[0].result.is_error,
        "{}",
        tool_result_error_text(&build_results[0].result)
    );
    assert_eq!(build_results[0].result.content["success"], true);

    let latest_build: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("outputs/build/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(latest_build["status"], "success");
    assert_eq!(latest_build["packageManager"], "npm");
    assert!(latest_build["sourceSnapshotUri"]
        .as_str()
        .unwrap()
        .starts_with("runtime://source-snapshots/project-1/build-"));

    assert!(project_root.join("out/docs.html").exists());
    assert!(project_root.join("out/docs/runtime-flow.html").exists());
    assert!(project_root.join("out/docs/typed-errors.html").exists());
}

#[tokio::test]
async fn project_init_cleans_conflicting_template_files_between_templates() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let website_results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-website",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert_eq!(website_results.len(), 1);
    assert!(!website_results[0].result.is_error);
    assert!(workspace.join("project/app/page.tsx").exists());
    assert!(workspace.join("project/next.config.mjs").exists());

    let docs_results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-docs",
                "project.init",
                json!({ "template": "fumadocs-docs" }),
            )],
        )
        .await;
    assert_eq!(docs_results.len(), 1);
    assert!(!docs_results[0].result.is_error);
    assert!(!workspace.join("project/app/page.tsx").exists());
    assert!(!workspace.join("project/components.json").exists());
    assert!(!workspace.join("project/components/ui/button.tsx").exists());
    assert!(workspace
        .join("project/app/docs/[[...slug]]/page.jsx")
        .exists());
    let docs_tsconfig = fs::read_to_string(workspace.join("project/tsconfig.json")).unwrap();
    assert!(docs_tsconfig.contains("\"plugins\": [{ \"name\": \"next\" }]"));
    assert!(docs_tsconfig.contains("\"allowJs\": true"));

    let website_again_results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-init-website-again",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert_eq!(website_again_results.len(), 1);
    assert!(!website_again_results[0].result.is_error);
    assert!(workspace.join("project/app/page.tsx").exists());
    assert!(workspace.join("project/next.config.mjs").exists());
    assert!(!workspace
        .join("project/app/docs/[[...slug]]/page.jsx")
        .exists());
    assert!(!workspace.join("project/content/docs/index.mdx").exists());
    assert!(workspace.join("project/next.config.mjs").exists());
    let website_tsconfig = fs::read_to_string(workspace.join("project/tsconfig.json")).unwrap();
    assert!(website_tsconfig.contains("\"allowJs\": false"));
    assert!(website_tsconfig.contains("\"strict\": true"));
}

#[tokio::test]
async fn fumadocs_docs_rejects_pages_router_writes_with_structured_metadata() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-docs",
                    "project.init",
                    json!({ "template": "fumadocs-docs" }),
                ),
                ToolCall::new(
                    "tool-write-pages-route",
                    "fs.write",
                    json!({
                        "path": "project/pages/index.jsx",
                        "text": "export default function Page() { return null; }"
                    }),
                ),
                ToolCall::new(
                    "tool-write-src-pages-route",
                    "fs.write",
                    json!({
                        "path": "project/src/pages/index.jsx",
                        "text": "export default function Page() { return null; }"
                    }),
                ),
                ToolCall::new(
                    "tool-write-shadow-mdx-components",
                    "fs.write",
                    json!({
                        "path": "project/src/mdx-components.tsx",
                        "text": "export function useMDXComponents() { return {}; }"
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 4);
    assert!(!results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "docs.routing_root_forbidden");
    assert!(results[2].result.is_error);
    assert_error_kind(&results[2].result, "docs.routing_root_forbidden");
    assert!(results[3].result.is_error);
    assert_error_kind(&results[3].result, "docs.routing_root_forbidden");
    assert!(!workspace.join("project/pages/index.jsx").exists());
    assert!(!workspace.join("project/src/pages/index.jsx").exists());
    assert!(!workspace.join("project/src/mdx-components.tsx").exists());
}

#[tokio::test]
async fn project_build_accepts_valid_fumadocs_docs_source_contract() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/out/docs")).unwrap();
    fs::write(
        workspace.join("project/out/docs/index.html"),
        "docs candidate",
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-docs",
                    "project.init",
                    json!({ "template": "fumadocs-docs" }),
                ),
                ToolCall::new(
                    "tool-build-docs",
                    "project.build",
                    json!({ "cwd": "project" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].result.is_error);
    assert!(
        !results[1].result.is_error,
        "{}",
        tool_result_error_text(&results[1].result)
    );
    assert_eq!(results[1].result.content["success"], true);
    assert!(results[1].result.content["sourceSnapshotUri"]
        .as_str()
        .unwrap()
        .starts_with("runtime://source-snapshots/project-1/"));
    assert!(results[1].result.content["candidateManifestHash"]
        .as_str()
        .is_some_and(|hash| hash.len() == 64));
    assert!(results[1].result.content["artifactRouteManifestHash"]
        .as_str()
        .is_some_and(|hash| hash.len() == 64));
    let route_manifest_path = results[1].result.content["artifactRouteManifestPath"]
        .as_str()
        .unwrap()
        .trim_start_matches("/workspace/");
    let route_manifest: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join(route_manifest_path)).unwrap())
            .unwrap();
    assert_eq!(route_manifest["schemaVersion"], "artifact-route-manifest@1");
    assert_eq!(route_manifest["entryRoute"], "/docs/");
    assert_eq!(route_manifest["canonicalPolicy"], "trailing_slash");
    assert_eq!(
        route_manifest["routes"]["/docs/"]["file"],
        "docs/index.html"
    );
    assert_eq!(route_manifest["aliases"]["/docs"], "/docs/");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.path == "/workspace/project"
            && request.payload["argv"]
                .as_array()
                .is_some_and(|argv| argv[0] == "npm" && argv[1] == "run" && argv[2] == "build")
    }));
}

#[tokio::test]
async fn project_build_rejects_ambiguous_fumadocs_artifact_routes() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/out/docs")).unwrap();
    fs::write(workspace.join("project/out/docs.html"), "legacy docs").unwrap();
    fs::write(
        workspace.join("project/out/docs/index.html"),
        "canonical docs",
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport, &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init-docs-ambiguous",
                    "project.init",
                    json!({ "template": "fumadocs-docs" }),
                ),
                ToolCall::new(
                    "tool-build-docs-ambiguous",
                    "project.build",
                    json!({ "cwd": "project" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "artifact.route_ambiguous");
    assert_eq!(
        fs::read_dir(workspace.join("outputs/candidates"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn project_build_rejects_fumadocs_docs_when_source_contract_is_missing() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "type": "module",
            "scripts": { "build": "next build" },
            "dependencies": { "fumadocs-ui": "^16.10.7" }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/project.json"),
        json!({ "appRoot": "project", "templateKey": "fumadocs-docs" }).to_string(),
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-build-docs",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    let error = tool_result_error_text(&results[0].result);
    assert!(error.contains("Docs source contract invalid"));
    assert!(error.contains("source.config.ts"));
    assert!(error.contains("content/docs/meta.json"));
    assert_error_kind(&results[0].result, "docs.source_contract_invalid");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(!requests.iter().any(|request| request.op == "process.exec"));
}

#[tokio::test]
async fn project_build_rejects_fumadocs_docs_with_pages_router_root() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let init_results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-init-docs",
                "project.init",
                json!({ "template": "fumadocs-docs" }),
            )],
        )
        .await;
    assert_eq!(init_results.len(), 1);
    assert!(!init_results[0].result.is_error);
    fs::create_dir_all(workspace.join("project/pages")).unwrap();
    fs::write(
        workspace.join("project/pages/index.jsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-build-docs",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "docs.routing_root_forbidden");
    assert!(tool_result_error_text(&results[0].result).contains("project/pages"));
    assert!(tool_result_error_text(&results[0].result).contains("forbidden"));
    let requests = transport.requests.lock().unwrap().clone();
    assert!(!requests.iter().any(|request| request.op == "process.exec"));
}

#[tokio::test]
async fn shell_run_can_execute_over_json_workspace_channel() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "shell.run",
                json!({ "argv": ["node", "-e", "process.stdout.write('remote')"], "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["stdout"],
        "ran:node@/workspace/project"
    );
    let requests = transport.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].op, "process.exec");
    assert_eq!(requests[0].path, "/workspace/project");
    assert_eq!(requests[0].payload["argv"][0], "node");
}

#[tokio::test]
async fn json_workspace_channel_backend_copy_dir_all_emits_copy_dir_request() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let run = store.get_run(&run_id).await.unwrap();
    let ctx = ToolContext::new(store, run, workspace.clone());

    workspace_backend
        .copy_dir_all(
            &ctx,
            &workspace.join("project"),
            &workspace.join("outputs/build/source-snapshots/build-test"),
            &["node_modules".to_string(), "dist".to_string()],
        )
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].op, "fs.copyDir");
    assert_eq!(requests[0].path, "/workspace/project");
    assert_eq!(
        requests[0].payload["to"],
        "/workspace/outputs/build/source-snapshots/build-test"
    );
    assert_eq!(requests[0].payload["skipDirNames"][0], "node_modules");
}

#[tokio::test]
async fn package_install_can_execute_over_json_workspace_channel() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "package.install",
                json!({ "mode": "add", "packages": ["@internal/ui"], "registry": "https://registry.internal.local" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["stdout"],
        "ran:npm@/workspace/project"
    );
    assert_eq!(results[0].result.content["command"][0], "npm");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(
        |request| request.op == "fs.stat" && request.path == "/workspace/project/package.json"
    ));
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.path == "/workspace/project"
            && request.payload["argv"][0] == "npm"
            && request.payload["argv"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == "@internal/ui")
    }));
    assert!(requests.iter().any(|request| {
        request.op == "fs.write"
            && request
                .path
                .starts_with("/workspace/outputs/build/package-install-tool-1")
            && request.payload["text"]
                .as_str()
                .unwrap()
                .contains("$ npm install")
    }));
    assert!(requests.iter().any(|request| {
        request.op == "fs.write"
            && request.path == "/workspace/outputs/build/package-install-latest.log"
    }));
}

#[tokio::test]
async fn project_ensure_dependencies_wraps_package_install_and_writes_dependency_state() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-ensure-deps",
                "project.ensure_dependencies",
                json!({ "mode": "restore", "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert_eq!(results[0].result.content["ensured"], true);
    assert_eq!(
        results[0].result.content["dependencyState"]["packageManager"],
        "npm"
    );
    assert_eq!(
        results[0].result.content["dependencyState"]["mode"],
        "restore"
    );
    assert_eq!(
        results[0].result.content["dependencyState"]["success"],
        true
    );

    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.path == "/workspace/project"
            && request.payload["argv"][0] == "npm"
            && request.payload["argv"][1] == "install"
    }));
    assert!(requests.iter().any(|request| {
        request.op == "fs.write"
            && request.path == "/workspace/state/dependency-state.json"
            && request.payload["text"]
                .as_str()
                .unwrap()
                .contains("\"needsRestore\": false")
    }));
}

#[tokio::test]
async fn project_ensure_dependencies_timeout_has_structured_metadata() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "dependencies": { "next": "^15.5.7" },
            "scripts": { "build": "next build" }
        }))
        .unwrap(),
    )
    .unwrap();
    let transport = ExecBehaviorTransport::new(ExecBehavior::Error(io::ErrorKind::TimedOut));
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport, &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-ensure-deps",
                "project.ensure_dependencies",
                json!({ "mode": "restore", "cwd": "project", "timeoutMs": 1 }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "dependency.install_timeout");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(metadata["timeoutMs"], 1);
    assert_eq!(metadata["packageManager"], "npm");
}

#[tokio::test]
async fn project_ensure_dependencies_registry_failure_is_typed_infrastructure_error() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "dependencies": { "next": "15.5.7" }
        }))
        .unwrap(),
    )
    .unwrap();
    let transport = ExecBehaviorTransport::new(ExecBehavior::Output {
        status: 1,
        success: false,
        stdout: String::new(),
        stderr: "npm error code EAI_AGAIN: internal registry unavailable".to_string(),
    });
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport, &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-ensure-deps",
                "project.ensure_dependencies",
                json!({ "mode": "restore", "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "infrastructure.registry_unavailable");
    assert_eq!(results[0].result.metadata.as_ref().unwrap()["status"], 1);
}

#[tokio::test]
async fn package_install_restore_uses_pnpm_install_and_emits_tool_output() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "dependencies": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-restore",
                "package.install",
                json!({
                    "mode": "restore",
                    "packageManager": "pnpm",
                    "registry": "https://registry.internal.local"
                }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["mode"], "restore");
    assert_eq!(results[0].result.content["packageManager"], "pnpm");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.payload["argv"]
                .as_array()
                .is_some_and(|argv| argv[0] == "pnpm" && argv[1] == "install")
    }));
    let events = store.events(&run_id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolOutput {
            tool,
            tool_use_id,
            stream,
            text,
            ..
        } if tool == "package.install"
            && tool_use_id == "tool-restore"
            && stream == "stdout"
            && text.contains("ran:pnpm@/workspace/project")
    )));
}

#[tokio::test]
async fn fs_tools_accept_virtual_workspace_prefix_paths() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/virtual-path.txt"),
        "from virtual path",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-read",
                "fs.read",
                json!({ "path": "/workspace/project/virtual-path.txt" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["text"], "from virtual path");
    assert_eq!(
        results[0].result.content["path"],
        "/workspace/project/virtual-path.txt"
    );
}

#[tokio::test]
async fn package_install_prefers_project_state_package_manager_over_lockfile() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "dependencies": {}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        workspace.join("project/pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\n",
    )
    .unwrap();
    fs::write(
        workspace.join("state/project.json"),
        json!({ "appRoot": "project", "packageManager": "npm" }).to_string(),
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    store
        .upsert_project_runtime_state(
            "project-1",
            "project".to_string(),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "next".to_string(),
            "npm".to_string(),
            "package-lock.json".to_string(),
            "https://registry.internal.example/npm/".to_string(),
        )
        .await
        .unwrap();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-state-manager",
                "package.install",
                json!({ "mode": "restore", "registry": "https://registry.internal.local" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["packageManager"], "npm");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.payload["argv"]
                .as_array()
                .is_some_and(|argv| argv[0] == "npm" && argv[1] == "install")
    }));
}

#[tokio::test]
async fn package_install_rejects_invalid_mode_package_combinations() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-add-empty",
                    "package.install",
                    json!({ "mode": "add", "packages": [] }),
                ),
                ToolCall::new(
                    "tool-restore-packages",
                    "package.install",
                    json!({ "mode": "restore", "packages": ["next"] }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("mode=add"));
    assert!(results[1].result.is_error);
    assert!(tool_result_error_text(&results[1].result).contains("mode=restore"));
}

#[tokio::test]
async fn json_workspace_channel_normalizes_paths_before_remote_requests() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
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
    let ctx = ToolContext::new(store, run, workspace.clone());

    workspace_backend
        .read_to_string(&ctx, &workspace.join("project/../project/index.md"))
        .await
        .unwrap();
    workspace_backend
        .write_string(
            &ctx,
            &workspace.join("project/../project/generated.md"),
            "remote",
        )
        .await
        .unwrap();
    command_backend
        .run(
            &ctx,
            &["node".to_string(), "--version".to_string()],
            &workspace.join("project/.."),
            1_000,
        )
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap().clone();
    assert_eq!(requests[0].path, "/workspace/project/index.md");
    assert_eq!(requests[1].path, "/workspace/project/generated.md");
    assert_eq!(requests[2].path, "/workspace");
}

#[tokio::test]
async fn json_workspace_channel_rejects_paths_outside_workspace() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport, &workspace);
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
    let ctx = ToolContext::new(store, run, workspace.clone());

    let error = workspace_backend
        .read_to_string(&ctx, &workspace.join("../outside.md"))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("path outside workspace"));
}

#[tokio::test]
async fn fs_tools_can_execute_over_json_workspace_channel() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/remote.md"), "local ignored").unwrap();
    let transport = RecordingChannelTransport::default();
    let backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_workspace_backend(Arc::new(backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "fs.read", json!({ "path": "project/remote.md" })),
                ToolCall::new("tool-2", "fs.list", json!({ "path": "project" })),
                ToolCall::new(
                    "tool-3",
                    "fs.search",
                    json!({ "path": "project", "query": "remote" }),
                ),
                ToolCall::new(
                    "tool-4",
                    "fs.write",
                    json!({ "path": "project/new-channel.md", "text": "hello channel" }),
                ),
                ToolCall::new(
                    "tool-5",
                    "fs.delete",
                    json!({ "path": "project/remote.md" }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 5);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "unexpected channel tool error: {:?}",
        results
            .iter()
            .map(|result| &result.result.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        results[0].result.content["text"],
        "remote:/workspace/project/remote.md"
    );
    assert_eq!(results[1].result.content["entries"][0]["name"], "child.md");
    assert_eq!(
        results[2].result.content["matches"][0]["text"],
        "remote:/workspace/project/child.md"
    );

    let requests = transport.requests.lock().unwrap().clone();
    let ops: Vec<_> = requests.iter().map(|request| request.op).collect();
    assert!(ops.iter().filter(|op| **op == "fs.read").count() >= 3);
    assert_eq!(ops.iter().filter(|op| **op == "fs.list").count(), 2);
    assert!(ops.iter().filter(|op| **op == "fs.stat").count() >= 3);
    assert!(ops.iter().filter(|op| **op == "fs.write").count() >= 2);
    assert_eq!(ops.iter().filter(|op| **op == "fs.removeFile").count(), 1);
    assert!(requests
        .iter()
        .all(|request| request.path.starts_with("/workspace/")));
    let write = requests
        .iter()
        .find(|request| {
            request.op == "fs.write" && request.path == "/workspace/project/new-channel.md"
        })
        .unwrap();
    assert_eq!(write.path, "/workspace/project/new-channel.md");
    assert_eq!(write.payload["text"], "hello channel");
    assert!(!workspace.join("project/new-channel.md").exists());
}

#[tokio::test]
async fn fs_tools_can_execute_over_websocket_workspace_channel() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/socket.md"), "local ignored").unwrap();
    let (endpoint, requests, handle) = start_workspace_channel_server().await;
    let workspace_backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_timeout(std::time::Duration::from_secs(2)),
        &workspace,
    );
    let command_backend = JsonWorkspaceChannelCommandBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint)
            .with_timeout(std::time::Duration::from_secs(2)),
        &workspace,
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "fs.read", json!({ "path": "project/socket.md" })),
                ToolCall::new(
                    "tool-2",
                    "fs.write",
                    json!({ "path": "project/socket-new.md", "text": "from runtime" }),
                ),
            ],
        )
        .await;

    handle.abort();
    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "unexpected websocket tool error: {:?}",
        results
            .iter()
            .map(|result| &result.result.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        results[0].result.content["text"],
        "websocket:/workspace/project/socket.md"
    );
    assert!(!workspace.join("project/socket-new.md").exists());
    let requests = requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request["op"] == "fs.read" && request["path"] == "/workspace/project/socket.md"
    }));
    assert!(requests.iter().any(|request| {
        request["op"] == "fs.write"
            && request["path"] == "/workspace/project/socket-new.md"
            && request["payload"]["text"] == "from runtime"
    }));
}

#[tokio::test]
async fn sandbox_channel_workspace_backend_resolves_endpoint_from_run_context() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/socket-bound.md"), "local ignored").unwrap();
    let (endpoint, requests, handle) = start_workspace_channel_server().await;
    let seen_sandbox_ids = Arc::new(Mutex::new(Vec::new()));
    let resolver = StaticEndpointResolver {
        endpoint,
        seen_sandbox_ids: seen_sandbox_ids.clone(),
    };
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "socket-bound-sandbox".to_string(),
            "socket-bound-sandbox".to_string(),
            "workspace-socket-bound-sandbox".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run_id, &binding.id)
        .await
        .unwrap();
    let backend = SandboxChannelWorkspaceBackend::new()
        .with_timeout(std::time::Duration::from_secs(2))
        .with_endpoint_resolver(Arc::new(resolver));
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_workspace_backend(Arc::new(backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "fs.read",
                json!({ "path": "project/socket-bound.md" }),
            )],
        )
        .await;

    handle.abort();
    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["text"],
        "websocket:/workspace/project/socket-bound.md"
    );
    let seen_sandbox_ids = seen_sandbox_ids.lock().unwrap();
    assert!(!seen_sandbox_ids.is_empty());
    assert!(seen_sandbox_ids
        .iter()
        .all(|sandbox_id| sandbox_id == &binding.id));
    assert_eq!(
        requests.lock().unwrap()[0]["path"],
        "/workspace/project/socket-bound.md"
    );
}

#[tokio::test]
async fn workspace_channel_server_script_serves_runtime_fs_protocol() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/channel-server.md"),
        "server text\nsecond line\n",
    )
    .unwrap();
    let local_package = workspace.join("local-package");
    fs::create_dir_all(&local_package).unwrap();
    fs::write(
        local_package.join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "@internal/channel-package",
            "version": "1.0.0",
            "main": "index.js"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(local_package.join("index.js"), "module.exports = 'ok';\n").unwrap();
    fs::write(
        workspace.join("project/binary.dat"),
        [0_u8, 1, 2, 127, 128, 254, 255],
    )
    .unwrap();
    fs::write(
        workspace.join("project/.anydesign-candidate-manifest.json"),
        "excluded",
    )
    .unwrap();
    fs::write(workspace.join("project/404.html"), "not found").unwrap();
    fs::create_dir_all(workspace.join("project/_next")).unwrap();
    fs::write(workspace.join("project/_next/runtime.js"), "runtime").unwrap();
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let mut child = Command::new("node")
        .arg(&script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .env("WORKSPACE_CHANNEL_ALLOW_TEST_PROCESS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start workspace-channel-server.js");

    wait_for_tcp_port(port).await;

    let endpoint = format!("ws://127.0.0.1:{port}/workspace");
    let backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let command_backend = JsonWorkspaceChannelCommandBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let run = store.get_run(&run_id).await.unwrap();
    let ctx = ToolContext::new(store.clone(), run, workspace.clone());
    let process = command_backend
        .start_process(
            &ctx,
            "process-lease-test-0001",
            &[
                "node".to_string(),
                "-e".to_string(),
                "setInterval(() => {}, 1000)".to_string(),
            ],
            &workspace.join("project"),
        )
        .await
        .expect("process.start lease");
    assert_eq!(process.status, "running");
    assert_eq!(
        command_backend
            .process_status(&ctx, "process-lease-test-0001")
            .await
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(
        command_backend
            .stop_process(&ctx, "process-lease-test-0001")
            .await
            .unwrap()
            .status,
        "stopped"
    );
    let restarted = command_backend
        .start_process(
            &ctx,
            "process-lease-test-0001",
            &[
                "node".to_string(),
                "-e".to_string(),
                "setInterval(() => {}, 1000)".to_string(),
            ],
            &workspace.join("project"),
        )
        .await
        .expect("terminal process lease must restart");
    assert_eq!(restarted.status, "running");
    assert_ne!(restarted.pid, process.pid);
    assert_eq!(
        command_backend
            .stop_process(&ctx, "process-lease-test-0001")
            .await
            .unwrap()
            .status,
        "stopped"
    );
    let heartbeat = workspace.join("project/process-timeout-heartbeat.txt");
    let heartbeat_script = r#"
const { spawn } = require('node:child_process');
const child = spawn(process.execPath, ['-e', `
  const fs = require('node:fs');
  process.on('SIGTERM', () => {});
  setInterval(() => fs.appendFileSync('process-timeout-heartbeat.txt', 'x'), 25);
`], { stdio: 'ignore' });
child.unref();
setInterval(() => {}, 1000);
"#;
    let timed_out = command_backend
        .run(
            &ctx,
            &[
                "node".to_string(),
                "-e".to_string(),
                heartbeat_script.to_string(),
            ],
            &workspace.join("project"),
            500,
        )
        .await
        .expect("timed process.exec response");
    assert!(!timed_out.success);
    assert!(timed_out.stderr.contains("process.exec timed out"));
    let heartbeat_after_timeout = fs::read(&heartbeat).expect("child must write heartbeat");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        fs::read(&heartbeat).expect("heartbeat remains readable"),
        heartbeat_after_timeout,
        "timed-out process.exec must terminate descendant processes before returning"
    );
    let missing = backend
        .path_kind(&ctx, &workspace.join("project/missing-template-path"))
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    backend
        .write_string(
            &ctx,
            &workspace.join("project/new/nested/generated.md"),
            "created with missing parents",
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(workspace.join("project/new/nested/generated.md")).unwrap(),
        "created with missing parents"
    );
    let export_target = workspace.join("runtime-export-target");
    let receipt = backend
        .export_tree(
            &ctx,
            &workspace.join("project"),
            &export_target,
            &[".anydesign-candidate-manifest.json".to_string()],
        )
        .await
        .expect("archive.export stream");
    assert!(receipt.file_count >= 3);
    assert_eq!(
        fs::read(export_target.join("binary.dat")).unwrap(),
        [0_u8, 1, 2, 127, 128, 254, 255]
    );
    assert!(!export_target
        .join(".anydesign-candidate-manifest.json")
        .exists());
    assert_eq!(
        fs::read_to_string(export_target.join("404.html")).unwrap(),
        "not found"
    );
    assert_eq!(
        fs::read_to_string(export_target.join("_next/runtime.js")).unwrap(),
        "runtime"
    );
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_workspace_backend(Arc::new(backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "fs.read",
                    json!({ "path": "project/channel-server.md" }),
                ),
                ToolCall::new(
                    "tool-2",
                    "fs.write",
                    json!({ "path": "project/generated-by-server.md", "text": "written by channel server" }),
                ),
                ToolCall::new(
                    "tool-3",
                    "shell.run",
                    json!({ "argv": ["node", "-e", "process.stdout.write('channel shell')"], "cwd": "project" }),
                ),
                ToolCall::new("tool-4", "fs.list", json!({ "path": "project" })),
                ToolCall::new(
                    "tool-5",
                    "fs.search",
                    json!({ "path": "project", "query": "server text" }),
                ),
                ToolCall::new(
                    "tool-6",
                    "package.install",
                    json!({ "packages": ["file:../local-package"], "registry": "https://registry.internal.local" }),
                ),
                ToolCall::new(
                    "tool-7",
                    "fs.delete",
                    json!({ "path": "project/generated-by-server.md" }),
                ),
            ],
        )
        .await;

    child.kill().await.ok();
    assert_eq!(results.len(), 7);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "unexpected workspace channel server tool error: {:?}",
        results
            .iter()
            .map(|result| &result.result.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        results[0].result.content["text"],
        "server text\nsecond line\n"
    );
    assert_eq!(results[2].result.content["stdout"], "channel shell");
    assert!(results[3].result.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "generated-by-server.md"));
    assert_eq!(
        results[4].result.content["matches"][0]["path"],
        "/workspace/project/channel-server.md"
    );
    assert_eq!(results[5].result.content["success"], true);
    assert!(workspace
        .join("project/node_modules/@internal/channel-package/package.json")
        .exists());
    assert!(
        fs::read_to_string(workspace.join("outputs/build/package-install-latest.log"))
            .unwrap()
            .contains("file:../local-package")
    );
    assert!(!workspace.join("project/generated-by-server.md").exists());
}

#[tokio::test]
async fn workspace_channel_server_requires_pod_bound_eddsa_token() {
    let workspace = setup_workspace();
    fs::write(workspace.join("project/authenticated.md"), "authorized").unwrap();
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let public_key_file = workspace.join("workspace-channel-public.der");
    fs::write(
        &public_key_file,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .expect("public key DER")
            .as_bytes(),
    )
    .unwrap();
    let previous_signing_key = SigningKey::from_bytes(&[12_u8; 32]);
    let previous_public_key_file = workspace.join("workspace-channel-public-previous.der");
    fs::write(
        &previous_public_key_file,
        previous_signing_key
            .verifying_key()
            .to_public_key_der()
            .expect("previous public key DER")
            .as_bytes(),
    )
    .unwrap();
    let issuer = WorkspaceChannelJwtIssuer::from_signing_key(signing_key.clone(), 60);
    let token = issuer
        .issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: "auth-test-valid-0001".to_string(),
            sandbox_binding_id: "binding-auth".to_string(),
            sandbox_name: "sandbox-auth".to_string(),
            pod_uid: "pod-uid-auth".to_string(),
            project_id: "project-1".to_string(),
            run_id: "run-auth".to_string(),
            operations: vec!["fs.read".to_string()],
        })
        .expect("issue token");
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let mut child = Command::new("node")
        .arg(&script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .env("WORKSPACE_CHANNEL_AUTH_MODE", "required")
        .env("WORKSPACE_CHANNEL_PUBLIC_KEY_FILE", &public_key_file)
        .env(
            "WORKSPACE_CHANNEL_PUBLIC_KEY_FILES",
            format!(
                "{},{}",
                public_key_file.display(),
                previous_public_key_file.display()
            ),
        )
        .env("POD_NAME", "sandbox-auth")
        .env("POD_UID", "pod-uid-auth")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start workspace-channel-server.js");
    wait_for_tcp_port(port).await;

    let endpoint = format!("ws://127.0.0.1:{port}/workspace");
    let request = WorkspaceChannelRequest {
        op: "fs.read",
        path: "/workspace/project/authenticated.md".to_string(),
        payload: json!({}),
    };
    let unauthorized = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await;
    assert!(
        unauthorized.is_err(),
        "missing token must fail the handshake"
    );

    let wrong_pod_token = issuer
        .issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: "auth-test-wrong-pod-0001".to_string(),
            sandbox_binding_id: "binding-auth".to_string(),
            sandbox_name: "sandbox-auth".to_string(),
            pod_uid: "wrong-pod-uid".to_string(),
            project_id: "project-1".to_string(),
            run_id: "run-auth".to_string(),
            operations: vec!["fs.read".to_string()],
        })
        .expect("wrong pod token");
    assert!(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_authorization(format!("Bearer {wrong_pod_token}"))
            .request(request.clone())
            .await
            .is_err(),
        "wrong Pod UID must fail the handshake"
    );

    let process_only_token = issuer
        .issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: "auth-test-process-only-0001".to_string(),
            sandbox_binding_id: "binding-auth".to_string(),
            sandbox_name: "sandbox-auth".to_string(),
            pod_uid: "pod-uid-auth".to_string(),
            project_id: "project-1".to_string(),
            run_id: "run-auth".to_string(),
            operations: vec!["process.exec".to_string()],
        })
        .expect("process-only token");
    assert!(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_authorization(format!("Bearer {process_only_token}"))
            .request(request.clone())
            .await
            .is_err(),
        "operation scope mismatch must fail the handshake"
    );

    for (label, claims) in [
        (
            "wrong audience",
            json!({
                "iss": "anydesign-runtime",
                "aud": "wrong-audience",
                "iat": Utc::now().timestamp(),
                "exp": Utc::now().timestamp() + 60,
                "jti": "wrong-audience-0001",
                "sandboxBindingId": "binding-auth",
                "sandboxName": "sandbox-auth",
                "podUid": "pod-uid-auth",
                "projectId": "project-1",
                "runId": "run-auth",
                "operations": ["fs.read"]
            }),
        ),
        (
            "expired",
            json!({
                "iss": "anydesign-runtime",
                "aud": "workspace-channel",
                "iat": Utc::now().timestamp() - 120,
                "exp": Utc::now().timestamp() - 60,
                "jti": "expired-token-0001",
                "sandboxBindingId": "binding-auth",
                "sandboxName": "sandbox-auth",
                "podUid": "pod-uid-auth",
                "projectId": "project-1",
                "runId": "run-auth",
                "operations": ["fs.read"]
            }),
        ),
    ] {
        let invalid_token = sign_workspace_claims(&signing_key, &claims);
        assert!(
            WebSocketWorkspaceChannelTransport::new(endpoint.clone())
                .with_authorization(format!("Bearer {invalid_token}"))
                .request(request.clone())
                .await
                .is_err(),
            "{label} token must fail the handshake"
        );
    }

    let response = WebSocketWorkspaceChannelTransport::new(endpoint.clone())
        .with_authorization(format!("Bearer {token}"))
        .with_timeout(Duration::from_secs(2))
        .request(request.clone())
        .await
        .expect("pod-bound token should authorize fs.read");
    let replay = WebSocketWorkspaceChannelTransport::new(endpoint)
        .with_authorization(format!("Bearer {token}"))
        .with_timeout(Duration::from_secs(2))
        .request(request)
        .await;
    child.kill().await.ok();
    child.wait().await.ok();
    let mut restarted_child = Command::new("node")
        .arg(&script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .env("WORKSPACE_CHANNEL_AUTH_MODE", "required")
        .env(
            "WORKSPACE_CHANNEL_PUBLIC_KEY_FILES",
            format!(
                "{},{}",
                public_key_file.display(),
                previous_public_key_file.display()
            ),
        )
        .env("POD_NAME", "sandbox-auth")
        .env("POD_UID", "pod-uid-auth")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must restart workspace-channel-server.js");
    wait_for_tcp_port(port).await;
    let replay_after_restart =
        WebSocketWorkspaceChannelTransport::new(format!("ws://127.0.0.1:{port}/workspace"))
            .with_authorization(format!("Bearer {token}"))
            .with_timeout(Duration::from_secs(2))
            .request(WorkspaceChannelRequest {
                op: "fs.read",
                path: "/workspace/project/authenticated.md".to_string(),
                payload: json!({}),
            })
            .await;

    let previous_token = WorkspaceChannelJwtIssuer::from_signing_key(previous_signing_key, 60)
        .issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: "auth-test-previous-key-0001".to_string(),
            sandbox_binding_id: "binding-auth".to_string(),
            sandbox_name: "sandbox-auth".to_string(),
            pod_uid: "pod-uid-auth".to_string(),
            project_id: "project-1".to_string(),
            run_id: "run-auth".to_string(),
            operations: vec!["fs.read".to_string()],
        })
        .expect("previous key token");
    let previous_key_response =
        WebSocketWorkspaceChannelTransport::new(format!("ws://127.0.0.1:{port}/workspace"))
            .with_authorization(format!("Bearer {previous_token}"))
            .with_timeout(Duration::from_secs(2))
            .request(WorkspaceChannelRequest {
                op: "fs.read",
                path: "/workspace/project/authenticated.md".to_string(),
                payload: json!({}),
            })
            .await
            .expect("previous rotation key should remain valid");
    restarted_child.kill().await.ok();
    assert_eq!(response["text"], "authorized");
    assert_eq!(previous_key_response["text"], "authorized");
    assert!(
        replay.is_err(),
        "the same jti must not authorize a second upgrade"
    );
    assert!(
        replay_after_restart.is_err(),
        "the same jti must remain consumed after a channel server restart"
    );
}

fn sign_workspace_claims(signing_key: &SigningKey, claims: &Value) -> String {
    let public_key = signing_key
        .verifying_key()
        .to_public_key_der()
        .expect("public key DER");
    let key_hash = sha256_hex(public_key.as_bytes());
    let header = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": format!("ed25519-{}", &key_hash[..16]),
        }))
        .unwrap(),
    );
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    let signing_input = format!("{header}.{payload}");
    let signature = signing_key.sign(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

#[tokio::test]
async fn static_preview_server_serves_only_frozen_candidate_snapshots() {
    let workspace = setup_workspace();
    let candidate = workspace.join("outputs/candidates/build-preview-test");
    fs::create_dir_all(candidate.join("docs")).unwrap();
    let root_bytes = b"<!doctype html><h1>Frozen candidate</h1>";
    let docs_bytes = b"<h1>Docs route</h1>";
    fs::write(candidate.join("index.html"), root_bytes).unwrap();
    fs::write(candidate.join("docs/index.html"), docs_bytes).unwrap();
    let route_manifest = ArtifactRouteManifest::build(
        "build-preview-test",
        &ArtifactRouteContract {
            entry_route: "/".to_string(),
            canonical_policy: anydesign_runtime::artifact_routes::RoutePolicy::TrailingSlash,
        },
        [
            ArtifactRouteFile {
                path: "index.html".to_string(),
                sha256: sha256_hex(root_bytes),
            },
            ArtifactRouteFile {
                path: "docs/index.html".to_string(),
                sha256: sha256_hex(docs_bytes),
            },
        ],
    )
    .unwrap();
    let route_manifest_text = serde_json::to_string_pretty(&route_manifest).unwrap();
    let route_manifest_hash = sha256_hex(route_manifest_text.as_bytes());
    fs::write(
        candidate.join(".anydesign-artifact-routes.json"),
        &route_manifest_text,
    )
    .unwrap();
    let manifest = serde_json::to_string_pretty(&json!({
        "schemaVersion": "candidate-manifest@1",
        "buildId": "build-preview-test",
        "artifactRouteManifestPath": ".anydesign-artifact-routes.json",
        "artifactRouteManifestHash": route_manifest_hash,
        "files": [
            { "path": ".anydesign-artifact-routes.json", "bytes": route_manifest_text.len(), "sha256": route_manifest_hash },
            { "path": "docs/index.html", "bytes": docs_bytes.len(), "sha256": sha256_hex(docs_bytes) },
            { "path": "index.html", "bytes": root_bytes.len(), "sha256": sha256_hex(root_bytes) }
        ]
    }))
    .unwrap();
    fs::write(
        candidate.join(".anydesign-candidate-manifest.json"),
        &manifest,
    )
    .unwrap();
    let manifest_hash = sha256_hex(manifest.as_bytes());
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/static-preview-server.js");
    let mut child = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("CANDIDATE_PREVIEW_HOST", "127.0.0.1")
        .env("CANDIDATE_PREVIEW_PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start static-preview-server.js");
    wait_for_tcp_port(port).await;

    let client = reqwest::Client::new();
    let root = client
        .get(format!(
            "http://127.0.0.1:{port}/candidates/build-preview-test/"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(root.status(), reqwest::StatusCode::OK);
    assert_eq!(
        root.headers()["x-anydesign-candidate-manifest-hash"],
        manifest_hash
    );
    assert!(root.text().await.unwrap().contains("Frozen candidate"));
    let docs = client
        .get(format!(
            "http://127.0.0.1:{port}/candidates/build-preview-test/docs"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(docs.status(), reqwest::StatusCode::OK);
    assert_eq!(
        docs.headers()["x-anydesign-artifact-path"],
        "docs/index.html"
    );
    assert!(docs.text().await.unwrap().contains("Docs route"));
    let docs_slash = client
        .head(format!(
            "http://127.0.0.1:{port}/candidates/build-preview-test/docs/"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(docs_slash.status(), reqwest::StatusCode::OK);
    assert_eq!(
        docs_slash.headers()["x-anydesign-artifact-sha256"],
        sha256_hex(docs_bytes)
    );
    let mutable_project = client
        .get(format!("http://127.0.0.1:{port}/project/index.html"))
        .send()
        .await
        .unwrap();
    child.kill().await.ok();
    assert_eq!(mutable_project.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspace_channel_server_script_copies_directory_snapshots() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/src")).unwrap();
    fs::create_dir_all(workspace.join("project/node_modules/ignored-package")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build/source-snapshots")).unwrap();
    fs::write(workspace.join("project/src/index.md"), "copy me").unwrap();
    fs::write(
        workspace.join("project/node_modules/ignored-package/index.js"),
        "ignored",
    )
    .unwrap();
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let mut child = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start workspace-channel-server.js");

    wait_for_tcp_port(port).await;

    let backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(format!("ws://127.0.0.1:{port}/workspace"))
            .with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let run = store.get_run(&run_id).await.unwrap();
    let ctx = ToolContext::new(store, run, workspace.clone());
    let target = workspace.join("outputs/build/source-snapshots/channel-build");

    backend
        .copy_dir_all(
            &ctx,
            &workspace.join("project"),
            &target,
            &["node_modules".to_string()],
        )
        .await
        .unwrap();

    child.kill().await.ok();
    assert_eq!(
        fs::read_to_string(target.join("src/index.md")).unwrap(),
        "copy me"
    );
    assert!(!target
        .join("node_modules/ignored-package/index.js")
        .exists());
}

#[tokio::test]
async fn websocket_workspace_channel_recovers_committed_project_init_transaction() {
    let workspace = setup_workspace();
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let mut child = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start workspace-channel-server.js");
    wait_for_tcp_port(port).await;

    let backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(format!("ws://127.0.0.1:{port}/workspace"))
            .with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let run = store.get_run(&run_id).await.unwrap();
    let ctx = ToolContext::new(store.clone(), run, workspace.clone());
    let mut transaction = ProjectInitWorkspaceTransaction::begin(
        &backend,
        &ctx,
        &workspace.join("project"),
        "next-app",
    )
    .await
    .unwrap();
    transaction
        .mark_workspace_committed(json!({
            "projectId": ctx.project_id,
            "runId": ctx.run.id,
            "appRoot": "project",
            "templateKey": "next-app",
            "templateVersion": "next-app@1",
            "templateManifestSha256": "919771231a9745aee050a3280518189d4b8d9f106d6ba334a896f41eac253067",
            "framework": "nextjs",
            "sandboxExecutionProfileId": "next-app",
            "sandboxExecutionProfileVersion": "0.1.0",
            "packageManager": "npm",
            "lockfile": "package-lock.json",
            "registry": ctx.npm_registry,
        }))
        .await
        .unwrap();
    assert_eq!(
        ProjectInitWorkspaceTransaction::recover_pending(&backend, &ctx)
            .await
            .unwrap(),
        ProjectInitRecoveryOutcome::CompletedCommitted
    );
    assert_eq!(
        store
            .get_project_runtime_state(&ctx.project_id)
            .await
            .unwrap()
            .template_key,
        "next-app"
    );
    assert!(!ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());

    backend
        .write_string(&ctx, &workspace.join("project/original.txt"), "original")
        .await
        .unwrap();
    let _prepared = ProjectInitWorkspaceTransaction::begin(
        &backend,
        &ctx,
        &workspace.join("project"),
        "next-app",
    )
    .await
    .unwrap();
    backend
        .remove_dir_all(&ctx, &workspace.join("project"))
        .await
        .unwrap();
    backend
        .write_string(&ctx, &workspace.join("project/partial.txt"), "partial")
        .await
        .unwrap();
    assert_eq!(
        ProjectInitWorkspaceTransaction::recover_pending(&backend, &ctx)
            .await
            .unwrap(),
        ProjectInitRecoveryOutcome::RolledBackPrepared
    );
    assert_eq!(
        backend
            .read_to_string(&ctx, &workspace.join("project/original.txt"))
            .await
            .unwrap(),
        "original"
    );
    assert!(backend
        .path_kind(&ctx, &workspace.join("project/partial.txt"))
        .await
        .is_err());
    child.kill().await.ok();
}

#[tokio::test]
async fn workspace_channel_server_script_serves_state_and_diagnostics_tools() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("outputs/build/build.log"),
        "Build failed\nError: missing import",
    )
    .unwrap();
    write_successful_build_state(&workspace);
    fs::write(
        workspace.join("outputs/reports/typescript.json"),
        json!({ "ok": false, "diagnostics": [{ "message": "Type mismatch" }] }).to_string(),
    )
    .unwrap();
    let (preview_url, preview_handle) = start_preview_server().await;
    let port = free_tcp_port();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
    let mut child = Command::new("node")
        .arg(script)
        .env("WORKSPACE_ROOT", &workspace)
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node must start workspace-channel-server.js");

    wait_for_tcp_port(port).await;

    let endpoint = format!("ws://127.0.0.1:{port}/workspace");
    let workspace_backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint.clone())
            .with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let command_backend = JsonWorkspaceChannelCommandBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(Duration::from_secs(2)),
        &workspace,
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
        Default::default(),
        &workspace,
    ));

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "diagnostics.build_log", json!({})),
                ToolCall::new("tool-2", "diagnostics.typescript", json!({})),
                ToolCall::new(
                    "tool-3",
                    "preview.start",
                    json!({ "url": preview_url, "port": 4321 }),
                ),
                ToolCall::new("tool-4", "preview.status", json!({})),
                ToolCall::new(
                    "tool-5",
                    "browser.open",
                    json!({ "url": "http://127.0.0.1:4321" }),
                ),
                ToolCall::new(
                    "tool-6",
                    "browser.screenshot",
                    json!({ "screenshotId": "channel-shot", "blank": false }),
                ),
                ToolCall::new("tool-7", "browser.inspect", json!({})),
            ],
        )
        .await;

    child.kill().await.ok();
    preview_handle.abort();
    assert_eq!(results.len(), 7);
    assert!(
        results.iter().all(|result| !result.result.is_error),
        "unexpected state channel tool error: {:?}",
        results
            .iter()
            .map(|result| &result.result.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(results[0].result.content["hasTerminalError"], true);
    assert_eq!(
        results[1].result.content["diagnostics"][0]["message"],
        "Type mismatch"
    );
    assert_eq!(results[3].result.content["status"], "running");
    assert_eq!(results[6].result.content["opened"], true);
    assert!(workspace.join("state/preview.json").exists());
    assert!(workspace.join("state/browser.json").exists());
    assert!(workspace
        .join("outputs/screenshots/channel-shot.json")
        .exists());
}

#[tokio::test]
async fn fs_rejects_secret_and_external_paths() {
    let workspace = setup_workspace();
    fs::write(workspace.join(".env"), "SECRET=1").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new("tool-1", "fs.read", json!({ "path": ".env" })),
                ToolCall::new("tool-2", "fs.read", json!({ "path": "/etc/passwd" })),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.result.is_error));
    assert_error_kind(&results[0].result, "path.secret");
    assert_error_kind(&results[1].result, "path.external_directory");
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "permission.denied"));
}

#[tokio::test]
async fn fs_external_path_returns_structured_recoverable_error() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-root",
                "fs.read",
                json!({ "path": "/" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("errorKind").and_then(Value::as_str),
        Some("path.external_directory")
    );
    assert_eq!(
        metadata.get("receivedPath").and_then(Value::as_str),
        Some("/")
    );
    assert_eq!(
        metadata.get("suggestedPath").and_then(Value::as_str),
        Some("project")
    );
    assert_eq!(
        metadata.get("recoverable").and_then(Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
async fn fs_invalid_path_component_returns_structured_recoverable_error() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-invalid",
                "fs.write",
                json!({ "path": "project/../escape.md", "text": "nope" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "path.invalid_component");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("receivedPath").and_then(Value::as_str),
        Some("project/../escape.md")
    );
}

#[tokio::test]
async fn fs_patch_tools_reject_nested_package_root_with_structured_error() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/src")).unwrap();
    fs::write(
        workspace.join("project/src/package.json"),
        "{\"name\":\"nested\"}\n",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-patch-nested-package",
                    "fs.patch",
                    json!({
                        "path": "project/src/package.json",
                        "oldStr": "nested",
                        "newStr": "changed"
                    }),
                ),
                ToolCall::new(
                    "tool-multi-patch-nested-package",
                    "fs.multi_patch",
                    json!({
                        "path": "project/src/package.json",
                        "edits": [
                            { "oldStr": "nested", "newStr": "changed" }
                        ]
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert_error_kind(&results[0].result, "path.nested_package_root");
    assert_error_kind(&results[1].result, "path.nested_package_root");
    assert_eq!(
        fs::read_to_string(workspace.join("project/src/package.json")).unwrap(),
        "{\"name\":\"nested\"}\n"
    );
    let audits = store.audit_records().await;
    assert_eq!(audits.len(), 2);
    assert_eq!(
        audits
            .iter()
            .filter(|record| record.decision == "deny"
                && record.reason.contains("nested package root denied"))
            .count(),
        2
    );
}

#[tokio::test]
async fn fs_patch_is_atomic_when_old_string_is_ambiguous() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("copy.md");
    fs::write(&file, "same\nsame\n").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let read = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-read",
                "fs.read",
                json!({ "path": "project/copy.md" }),
            )],
        )
        .await;
    assert!(!read[0].result.is_error);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "fs.patch",
                json!({ "path": "project/copy.md", "oldStr": "same", "newStr": "changed" }),
            )],
        )
        .await;

    assert!(results[0].result.is_error);
    assert_eq!(fs::read_to_string(&file).unwrap(), "same\nsame\n");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("errorKind").and_then(Value::as_str),
        Some("patch.old_str_ambiguous")
    );
    assert_eq!(metadata.get("matchCount").and_then(Value::as_u64), Some(2));
}

#[tokio::test]
async fn fs_patch_replace_all_updates_repeated_matches() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("tokens.css");
    fs::write(
        &file,
        ":root { --color-primary: #16a34a; }\n.button { color: #16a34a; }\n",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-read",
                    "fs.read",
                    json!({ "path": "project/tokens.css" }),
                ),
                ToolCall::new(
                    "tool-patch",
                    "fs.patch",
                    json!({
                        "path": "project/tokens.css",
                        "oldStr": "#16a34a",
                        "newStr": "#7c3aed",
                        "replaceAll": true
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].result.is_error);
    assert!(!results[1].result.is_error);
    assert_eq!(results[1].result.content["replaceAll"], true);
    assert_eq!(results[1].result.content["replacements"], 2);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        ":root { --color-primary: #7c3aed; }\n.button { color: #7c3aed; }\n"
    );
}

#[tokio::test]
async fn fs_patch_rejects_stale_read_after_file_changes() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("copy.md");
    fs::write(&file, "hello\nworld\n").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let read = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-read",
                "fs.read",
                json!({ "path": "project/copy.md" }),
            )],
        )
        .await;
    assert!(!read[0].result.is_error);

    fs::write(&file, "hello\nexternal edit\n").unwrap();

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-patch",
                "fs.patch",
                json!({ "path": "project/copy.md", "oldStr": "hello", "newStr": "hi" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("modified since fs.read"));
    assert_error_kind(&results[0].result, "mutation.stale_lease");
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello\nexternal edit\n");
}

#[tokio::test]
async fn fs_multi_patch_applies_multiple_edits_atomically() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("page.tsx");
    fs::write(
        &file,
        "<h1>Old title</h1>\n<p>Old body</p>\n<span>#16a34a</span>\n",
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-read",
                    "fs.read",
                    json!({ "path": "project/page.tsx" }),
                ),
                ToolCall::new(
                    "tool-multi-patch",
                    "fs.multi_patch",
                    json!({
                        "path": "project/page.tsx",
                        "edits": [
                            { "oldStr": "Old title", "newStr": "New title" },
                            { "oldStr": "Old body", "newStr": "New body" },
                            { "oldStr": "#16a34a", "newStr": "#7c3aed" }
                        ]
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].result.is_error);
    assert!(!results[1].result.is_error);
    assert_eq!(results[1].result.content["replacements"], 3);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "<h1>New title</h1>\n<p>New body</p>\n<span>#7c3aed</span>\n"
    );
}

#[tokio::test]
async fn fs_multi_patch_does_not_write_when_later_edit_fails() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("page.tsx");
    let original = "<h1>Old title</h1>\n<p>Old body</p>\n";
    fs::write(&file, original).unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-read",
                    "fs.read",
                    json!({ "path": "project/page.tsx" }),
                ),
                ToolCall::new(
                    "tool-multi-patch",
                    "fs.multi_patch",
                    json!({
                        "path": "project/page.tsx",
                        "edits": [
                            { "oldStr": "Old title", "newStr": "New title" },
                            { "oldStr": "Missing text", "newStr": "New body" }
                        ]
                    }),
                ),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert!(tool_result_error_text(&results[1].result).contains("edit 1 oldStr not found"));
    assert_error_kind(&results[1].result, "patch.old_str_missing");
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
}

#[tokio::test]
async fn fs_patch_requires_read_before_patch() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("copy.md");
    fs::write(&file, "hello\nworld\n").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "fs.patch",
                json!({ "path": "project/copy.md", "oldStr": "hello", "newStr": "hi" }),
            )],
        )
        .await;

    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("requires reading"));
    assert_error_kind(&results[0].result, "mutation.read_required");
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello\nworld\n");
}

#[tokio::test]
async fn fs_write_requires_lease_for_existing_file_and_advances_self_authored_hash() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("copy.md");
    fs::write(&file, "first\n").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let blocked = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "write-before-read",
                "fs.write",
                json!({ "path": "project/copy.md", "text": "second\n" }),
            )],
        )
        .await;
    assert_error_kind(&blocked[0].result, "mutation.read_required");

    let allowed = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "read-before-write",
                    "fs.read",
                    json!({ "path": "project/copy.md" }),
                ),
                ToolCall::new(
                    "write-observed",
                    "fs.write",
                    json!({ "path": "project/copy.md", "text": "second\n" }),
                ),
                ToolCall::new(
                    "write-self-authored",
                    "fs.write",
                    json!({ "path": "project/copy.md", "text": "third\n" }),
                ),
            ],
        )
        .await;
    assert!(allowed.iter().all(|result| !result.result.is_error));
    assert_eq!(fs::read_to_string(&file).unwrap(), "third\n");

    fs::write(&file, "external\n").unwrap();
    let stale = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "write-after-external-change",
                "fs.write",
                json!({ "path": "project/copy.md", "text": "fourth\n" }),
            )],
        )
        .await;
    assert_error_kind(&stale[0].result, "mutation.stale_lease");
}

#[tokio::test]
async fn batched_fs_reads_preserve_the_target_patch_lease() {
    let workspace = setup_workspace();
    for index in 0..6 {
        fs::write(
            workspace.join("project").join(format!("page-{index}.md")),
            format!("old-{index}\n"),
        )
        .unwrap();
    }
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);
    let mut calls = (0..6)
        .map(|index| {
            ToolCall::new(
                format!("tool-read-{index}"),
                "fs.read",
                json!({ "path": format!("project/page-{index}.md") }),
            )
        })
        .collect::<Vec<_>>();
    calls.push(ToolCall::new(
        "tool-patch",
        "fs.patch",
        json!({ "path": "project/page-0.md", "oldStr": "old-0", "newStr": "new-0" }),
    ));

    let tracked = executor.track_calls(calls.clone());
    assert!(
        tracked[..6].iter().all(|call| !call.is_concurrency_safe),
        "fs.read must serialize its read-tracking side effect"
    );
    let results = executor.execute_calls(store, &run_id, calls).await;

    assert_eq!(results.len(), 7);
    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(
        fs::read_to_string(workspace.join("project/page-0.md")).unwrap(),
        "new-0\n"
    );
}

#[tokio::test]
async fn shell_run_denied_command_returns_structured_recoverable_error() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-shell",
                "shell.run",
                json!({ "argv": ["sh", "-c", "echo nope"] }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "shell.command_denied");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(metadata["argv"], json!(["sh", "-c", "echo nope"]));
}

#[tokio::test]
async fn fs_delete_is_limited_to_project_children() {
    let workspace = setup_workspace();
    let deletable = workspace.join("project").join("old.md");
    fs::write(&deletable, "old").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "fs.delete", json!({ "path": "project" })),
                ToolCall::new(
                    "tool-read-old",
                    "fs.read",
                    json!({ "path": "project/old.md" }),
                ),
                ToolCall::new("tool-2", "fs.delete", json!({ "path": "project/old.md" })),
            ],
        )
        .await;

    assert!(results[0].result.is_error);
    assert!(!results[1].result.is_error);
    assert!(!results[2].result.is_error);
    assert!(!deletable.exists());
}

#[tokio::test]
async fn shell_run_uses_argv_policy_without_shell_strings() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "shell.run",
                    json!({ "argv": ["node", "-e", "process.stdout.write('ok')"], "cwd": "project" }),
                ),
                ToolCall::new(
                    "tool-2",
                    "shell.run",
                    json!({ "argv": ["sh", "-c", "echo nope"], "cwd": "project" }),
                ),
            ],
        )
        .await;

    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["stdout"], "ok");
    assert!(results[1].result.is_error);
    assert!(tool_result_error_text(&results[1].result).contains("not allowed"));
}

#[tokio::test]
async fn shell_run_local_backend_maps_virtual_workspace_argv_paths() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/dist")).unwrap();
    fs::write(workspace.join("project/dist/index.html"), "<h1>ok</h1>").unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-shell",
                "shell.run",
                json!({ "argv": ["ls", "/workspace/project/dist"], "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert!(results[0].result.content["stdout"]
        .as_str()
        .unwrap()
        .contains("index.html"));
}

#[tokio::test]
async fn shell_run_non_zero_exit_is_error_and_cancels_later_sibling_tools() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "shell.run",
                    json!({ "argv": ["node", "-e", "process.stderr.write('boom'); process.exit(7)"], "cwd": "project" }),
                ),
                ToolCall::new("tool-2", "fs.read", json!({ "path": "project/index.md" })),
            ],
        )
        .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("status Some(7)"));
    assert!(tool_result_error_text(&results[0].result).contains("boom"));
    assert_error_kind(&results[0].result, "shell.non_zero_exit");
    assert!(results[1].synthetic);
    assert!(tool_result_error_text(&results[1].result).contains("shell.run failed"));
}

#[tokio::test]
async fn package_install_public_registry_is_denied_in_production_profile() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "package.install",
                json!({ "packages": ["left-pad"], "registry": "https://registry.npmjs.org" }),
            )],
        )
        .await;

    assert!(results[0].result.is_error);
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );
    assert!(tool_result_error_text(&results[0].result).contains("public npm registry"));
    assert!(!store
        .events(&run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "permission.requested"));
}

#[tokio::test]
async fn package_install_omitted_public_registry_is_denied_in_production_profile() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), &workspace)
            .with_policy_profile_and_registry(
                RuntimePolicyProfile::Production,
                "https://registry.npmjs.org",
            ),
    );

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "package.install",
                json!({ "mode": "restore" }),
            )],
        )
        .await;

    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("public npm registry"));
}

#[tokio::test]
async fn package_install_public_registry_is_allowed_only_for_local_e2e_profile() {
    let workspace = setup_workspace();
    let transport = RecordingChannelTransport::default();
    let workspace_backend = JsonWorkspaceChannelBackend::new(transport.clone(), &workspace);
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(
            sandbox_tools_with_backends(Arc::new(workspace_backend), Arc::new(command_backend)),
            Default::default(),
            &workspace,
        )
        .with_policy_profile_and_registry(
            RuntimePolicyProfile::LocalE2e,
            "https://registry.npmjs.org",
        ),
    );

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "package.install",
                json!({ "packages": ["left-pad"] }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["registry"],
        "https://registry.npmjs.org"
    );
}

#[tokio::test]
async fn package_install_internal_registry_installs_workspace_local_package_and_logs() {
    let workspace = setup_workspace();
    let local_package = workspace.join("local-package");
    fs::create_dir_all(&local_package).unwrap();
    fs::write(
        local_package.join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "@internal/ui",
            "version": "1.0.0",
            "main": "index.js"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(local_package.join("index.js"), "export const ok = true;\n").unwrap();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "dependencies": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "package.install",
                json!({ "packages": ["file:../local-package"], "registry": "https://registry.internal.local" }),
            )],
        )
        .await;

    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["success"], true);
    assert_eq!(
        results[0].result.content["logPath"],
        "/workspace/outputs/build/package-install-tool-1.log"
    );
    assert!(workspace
        .join("project/node_modules/@internal/ui/package.json")
        .exists());
    assert!(fs::read_to_string(workspace.join("project/package.json"))
        .unwrap()
        .contains("@internal/ui"));
    assert!(
        fs::read_to_string(workspace.join("outputs/build/package-install-tool-1.log"))
            .unwrap()
            .contains("file:../local-package")
    );
    assert!(
        fs::read_to_string(workspace.join("outputs/build/package-install-latest.log"))
            .unwrap()
            .contains("file:../local-package")
    );
    assert_eq!(store.audit_records().await[0].decision, "allow");
}

#[tokio::test]
async fn preview_start_status_and_stop_use_workspace_state() {
    let workspace = setup_workspace();
    write_successful_build_state(&workspace);
    let (preview_url, _preview_server) = start_preview_server().await;
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "preview.start",
                    json!({ "url": preview_url, "port": 4321 }),
                ),
                ToolCall::new("tool-2", "preview.status", json!({})),
                ToolCall::new("tool-3", "preview.stop", json!({})),
                ToolCall::new("tool-4", "preview.status", json!({})),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[1].result.content["status"], "running");
    assert_eq!(results[1].result.content["accessible"], true);
    assert_eq!(results[3].result.content["status"], "stopped");
    assert_eq!(results[3].result.content["accessible"], false);
}

#[tokio::test]
async fn kubernetes_preview_start_creates_pod_bound_candidate_lease() {
    let workspace = setup_workspace();
    write_successful_build_state(&workspace);
    let mut build: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("outputs/build/latest.json")).unwrap(),
    )
    .unwrap();
    build["buildId"] = json!("build-test");
    build["candidateOutputPath"] = json!("/workspace/outputs/candidates/build-test");
    build["candidateManifestHash"] = json!("a".repeat(64));
    fs::write(
        workspace.join("outputs/build/latest.json"),
        serde_json::to_string_pretty(&build).unwrap(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-preview".to_string(),
            "claim-preview".to_string(),
            "workspace-preview".to_string(),
            "pool-preview".to_string(),
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
            "sandbox-preview".to_string(),
            Some("sandbox-preview".to_string()),
            Some("sandbox-uid-preview".to_string()),
            Some("pod-uid-preview".to_string()),
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run_id, &binding.id)
        .await
        .unwrap();
    let process_transport = RecordingChannelTransport::default();
    let command_backend =
        JsonWorkspaceChannelCommandBackend::new(process_transport.clone(), &workspace);
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(
            sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
            Default::default(),
            &workspace,
        )
        .with_runtime_public_base_url("http://runtime.test")
        .with_remote_workspace(true),
    );

    let started = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "preview-lease-start",
                "preview.start",
                json!({}),
            )],
        )
        .await;
    assert!(!started[0].result.is_error);
    assert_eq!(started[0].result.content["port"], 4321);
    assert_eq!(started[0].result.content["managed"], true);
    let lease_id = started[0].result.content["leaseId"].as_str().unwrap();
    assert!(started[0].result.content["url"]
        .as_str()
        .unwrap()
        .contains(lease_id));
    let lease = store.get_preview_lease(lease_id).await.unwrap();
    assert_eq!(lease.pod_uid, "pod-uid-preview");
    assert_eq!(lease.candidate_manifest_hash, "a".repeat(64));
    assert!(process_transport
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| {
            request.op == "process.start"
                && request.payload["leaseId"] == lease_id
                && request.payload["argv"][1] == "/opt/anydesign/bootstrap/static-preview-server.js"
        }));
    assert!(process_transport
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| {
            request.op == "process.exec"
                && request.payload["argv"][2]
                    .as_str()
                    .is_some_and(|script| script.contains("http://127.0.0.1:4321/healthz"))
        }));

    let stopped = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "preview-lease-stop",
                "preview.stop",
                json!({}),
            )],
        )
        .await;
    assert!(!stopped[0].result.is_error);
    assert_eq!(
        store.get_preview_lease(lease_id).await.unwrap().status,
        PreviewLeaseStatus::Stopped
    );
}

#[tokio::test]
async fn next_app_dev_preview_uses_hmr_lease_and_durable_revisions() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let initializer = sandbox_executor(&workspace);
    let initialized = initializer
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-init",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!initialized[0].result.is_error);

    let historical_snapshot = store
        .create_draft_snapshot(
            "project-1",
            "runtime://source-snapshots/project-1/historical-next-dev".to_string(),
            "a".repeat(64),
            "next-app".to_string(),
            "1".to_string(),
            "runtime-dependency-policy-v1".to_string(),
            "b".repeat(64),
            "historical-next-dev-run",
            None,
            None,
        )
        .await
        .unwrap();

    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-next-dev".to_string(),
            "claim-next-dev".to_string(),
            "workspace-next-dev".to_string(),
            "pool-next-dev".to_string(),
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
            "sandbox-next-dev".to_string(),
            Some("sandbox-next-dev".to_string()),
            Some("sandbox-next-dev-uid".to_string()),
            Some("pod-next-dev-uid".to_string()),
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run_id, &binding.id)
        .await
        .unwrap();

    let transport = RecordingChannelTransport::default();
    let command = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(
            sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command)),
            Default::default(),
            &workspace,
        )
        .with_runtime_public_base_url("http://runtime.test")
        .with_remote_workspace(true),
    );
    let started = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-start",
                "preview.dev_start",
                json!({}),
            )],
        )
        .await;
    assert!(
        !started[0].result.is_error,
        "{}",
        tool_result_error_text(&started[0].result)
    );
    let lease_id = started[0].result.content["leaseId"].as_str().unwrap();
    let session_id = started[0].result.content["sessionId"].as_str().unwrap();
    let session = store.draft_preview_store().get(session_id).unwrap();
    assert_ne!(session.base_snapshot_id, historical_snapshot.snapshot_id);
    let base_snapshot = store
        .get_draft_snapshot(&session.base_snapshot_id)
        .await
        .unwrap();
    assert_eq!(base_snapshot.created_by_run_id, run_id);
    assert_eq!(
        base_snapshot.based_on_snapshot_id.as_deref(),
        Some(historical_snapshot.snapshot_id.as_str())
    );
    let lease = store.get_preview_lease(lease_id).await.unwrap();
    assert_eq!(lease.mode, PreviewLeaseMode::Dev);
    assert_eq!(lease.target_port, 3000);
    assert!(transport.requests.lock().unwrap().iter().any(|request| {
        request.op == "process.start"
            && request.payload["argv"][1]
                == format!("ANYDESIGN_PREVIEW_BASE_PATH=/previews/{lease_id}")
            && request.payload["argv"][2] == "npm"
    }));

    let mutated = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-write",
                "fs.write",
                json!({
                    "path": "project/app/dev-preview-test.tsx",
                    "text": "export default function DevPreviewTest() { return <div>updated</div> }\n"
                }),
            )],
        )
        .await;
    assert!(
        !mutated[0].result.is_error,
        "{}",
        tool_result_error_text(&mutated[0].result)
    );
    assert_eq!(
        mutated[0].result.content["draftPreview"]["status"],
        "durable"
    );
    let session = store.draft_preview_store().get(session_id).unwrap();
    assert_eq!(session.workspace_revision, 1);
    assert_eq!(session.durable_revision, 1);
    assert_ne!(session.durable_snapshot_id, session.base_snapshot_id);
    let first_snapshot_id = session.durable_snapshot_id.clone();

    let pending_snapshot = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-pending-durable-snapshot",
                "draft.snapshot_create",
                json!({}),
            )],
        )
        .await;
    assert!(pending_snapshot[0].result.is_error);
    assert_error_kind(
        &pending_snapshot[0].result,
        "draft.preview_revision_pending",
    );

    store
        .draft_preview_store()
        .mark_ready(
            &session.session_id,
            session.session_epoch,
            session.workspace_revision,
        )
        .unwrap();

    let reused_snapshot = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-reuse-durable-snapshot",
                "draft.snapshot_create",
                json!({}),
            )],
        )
        .await;
    assert!(
        !reused_snapshot[0].result.is_error,
        "{}",
        tool_result_error_text(&reused_snapshot[0].result)
    );
    assert_eq!(
        reused_snapshot[0].result.content["status"],
        "snapshot_reused"
    );
    assert_eq!(
        reused_snapshot[0].result.content["draftSnapshot"]["snapshotId"],
        first_snapshot_id
    );

    let mutated_again = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-write-again",
                "fs.write",
                json!({
                    "path": "project/app/dev-preview-test.tsx",
                    "text": "export default function DevPreviewTest() { return <div>changed again</div> }\n"
                }),
            )],
        )
        .await;
    assert!(
        !mutated_again[0].result.is_error,
        "{}",
        tool_result_error_text(&mutated_again[0].result)
    );
    assert_eq!(
        store
            .draft_preview_store()
            .get(session_id)
            .unwrap()
            .workspace_revision,
        2
    );
    let versions_before_restore = store.list_project_versions("project-1").await;

    let restored = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-restore",
                "draft.restore",
                json!({
                    "kind": "draft_snapshot",
                    "itemId": first_snapshot_id,
                }),
            )],
        )
        .await;
    assert!(
        !restored[0].result.is_error,
        "{}",
        tool_result_error_text(&restored[0].result)
    );
    assert_eq!(
        fs::read_to_string(workspace.join("project/app/dev-preview-test.tsx")).unwrap(),
        "export default function DevPreviewTest() { return <div>updated</div> }\n"
    );
    let restored_snapshot = &restored[0].result.content["draftSnapshot"];
    assert_ne!(
        restored_snapshot["snapshotId"].as_str().unwrap(),
        first_snapshot_id
    );
    assert_eq!(
        restored_snapshot["basedOnSnapshotId"].as_str().unwrap(),
        first_snapshot_id
    );
    assert_eq!(restored[0].result.content["productionBuildCreated"], false);
    assert_eq!(restored[0].result.content["publicationChanged"], false);
    let versions_after_restore = store.list_project_versions("project-1").await;
    assert_eq!(versions_after_restore.len(), versions_before_restore.len());
    assert_eq!(
        versions_after_restore
            .iter()
            .map(|version| version.id.as_str())
            .collect::<Vec<_>>(),
        versions_before_restore
            .iter()
            .map(|version| version.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        store.draft_preview_store().get(session_id).unwrap().status,
        DraftPreviewSessionStatus::Stopped
    );
    let restored_session_id = restored[0].result.content["preview"]["sessionId"]
        .as_str()
        .unwrap();
    assert_ne!(restored_session_id, session_id);
    assert!(matches!(
        store
            .draft_preview_store()
            .get(restored_session_id)
            .unwrap()
            .status,
        DraftPreviewSessionStatus::Starting | DraftPreviewSessionStatus::Ready
    ));

    let dependency_added = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-add-dependency",
                "package.install",
                json!({
                    "mode": "add",
                    "packages": ["lucide-react"],
                }),
            )],
        )
        .await;
    assert!(
        !dependency_added[0].result.is_error,
        "{}",
        tool_result_error_text(&dependency_added[0].result)
    );
    assert_eq!(
        dependency_added[0].result.content["draftPreview"]["status"], "durable",
        "draft preview result: {:#?}",
        dependency_added[0].result.content["draftPreview"]
    );
    assert_eq!(
        store
            .draft_preview_store()
            .get(restored_session_id)
            .unwrap()
            .workspace_revision,
        1
    );

    let stopped = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "next-dev-stop",
                "preview.dev_stop",
                json!({}),
            )],
        )
        .await;
    assert!(!stopped[0].result.is_error);
    assert_eq!(
        store.get_preview_lease(lease_id).await.unwrap().status,
        PreviewLeaseStatus::Stopped
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn next_app_p1_registry_and_project_assets_are_hash_pinned_and_local() {
    let _asset_provider_guard = ASSET_PROVIDER_ENV_LOCK.lock().await;
    unsafe {
        std::env::remove_var("ASSET_GENERATION_PROVIDER_ENDPOINT");
        std::env::remove_var("ASSET_GENERATION_PROVIDER_AUTH_TOKEN");
    }
    let workspace = setup_workspace();
    let runtime_storage = workspace.join(".runtime-storage");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), &workspace)
            .with_runtime_storage_dir(runtime_storage.clone()),
    );
    let initialized = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "p1-next-init",
                "project.init",
                json!({ "template": "next-app" }),
            )],
        )
        .await;
    assert!(!initialized[0].result.is_error);

    let inspected = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "p1-component-inspect",
                "component.inspect",
                json!({ "name": "badge" }),
            )],
        )
        .await;
    assert!(!inspected[0].result.is_error);
    let component_hash = inspected[0].result.content["item"]["contentHash"]
        .as_str()
        .unwrap();
    let installed = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "p1-component-install",
                "component.install",
                json!({
                    "name": "badge",
                    "expectedContentHash": component_hash,
                }),
            )],
        )
        .await;
    assert!(
        !installed[0].result.is_error,
        "{}",
        tool_result_error_text(&installed[0].result)
    );
    assert_eq!(installed[0].result.content["sourceContract"], "pass");
    assert_eq!(installed[0].result.content["dependencyPolicy"], "pass");
    assert!(workspace.join("project/components/ui/badge.tsx").is_file());

    let visual_store = VisualArtifactStore::open(runtime_storage.join("visual-artifacts")).unwrap();
    let visual = visual_store
        .create_upload("project-1", &one_pixel_visual_asset(), BTreeMap::new())
        .unwrap();
    let imported = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "p1-asset-import",
                "asset.import",
                json!({
                    "artifactId": visual.id,
                    "name": "hero-art",
                    "altText": "Abstract blue square",
                    "license": "user-owned",
                }),
            )],
        )
        .await;
    assert!(
        !imported[0].result.is_error,
        "{}",
        tool_result_error_text(&imported[0].result)
    );
    let target_path = imported[0].result.content["targetPath"].as_str().unwrap();
    assert!(target_path.starts_with("public/assets/"));
    assert!(workspace.join("project").join(target_path).is_file());
    let project_assets = ProjectAssetStore::open(&runtime_storage).unwrap();
    let assets = project_assets.list_project("project-1");
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].content_hash, visual.sha256);
    assert_eq!(assets[0].target_path, target_path);

    let unavailable = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "p1-asset-generate",
                "asset.generate",
                json!({
                    "prompt": "Editorial blue texture",
                    "width": 1200,
                    "height": 800,
                    "crop": "cover",
                    "altText": "Blue editorial texture",
                }),
            )],
        )
        .await;
    assert!(unavailable[0].result.is_error);
    assert_error_kind(&unavailable[0].result, "asset.provider_unavailable");
    assert_eq!(
        unavailable[0].result.metadata.as_ref().unwrap()["blocking"],
        false
    );

    let (provider_url, provider) = start_asset_generation_provider().await;
    unsafe {
        std::env::set_var("ASSET_GENERATION_PROVIDER_ENDPOINT", provider_url);
        std::env::set_var("ASSET_GENERATION_PROVIDER_AUTH_TOKEN", "connector-secret");
    }
    let generated = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "p1-asset-generate-success",
                "asset.generate",
                json!({
                    "prompt": "Editorial blue texture",
                    "width": 1200,
                    "height": 800,
                    "crop": "cover",
                    "altText": "Blue editorial texture",
                }),
            )],
        )
        .await;
    unsafe {
        std::env::remove_var("ASSET_GENERATION_PROVIDER_ENDPOINT");
        std::env::remove_var("ASSET_GENERATION_PROVIDER_AUTH_TOKEN");
    }
    provider.abort();
    assert!(
        !generated[0].result.is_error,
        "{}",
        tool_result_error_text(&generated[0].result)
    );
    assert_eq!(generated[0].result.content["asset"]["source"], "generated");
    assert_eq!(generated[0].result.content["partial"], false);
    assert!(workspace
        .join("project")
        .join(
            generated[0].result.content["asset"]["targetPath"]
                .as_str()
                .unwrap()
        )
        .is_file());
    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn preview_start_spawns_static_server_from_dist() {
    let workspace = setup_workspace();
    write_frozen_candidate_build_state(
        &workspace,
        "build-preview-dist",
        ArtifactRouteContract::website(),
        &[(
            "index.html",
            "<!doctype html><title>Preview</title><h1>Ready</h1>",
        )],
    );
    let port = free_tcp_port();
    let preview_url = format!("http://127.0.0.1:{port}");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "preview.start",
                    json!({ "url": preview_url, "port": port }),
                ),
                ToolCall::new("tool-2", "preview.stop", json!({})),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[0].result.content["status"], "running");
    assert_eq!(results[0].result.content["accessible"], true);
    assert!(results[0].result.content["pid"].as_u64().is_some());
    assert_eq!(results[1].result.content["status"], "stopped");
    assert_eq!(results[1].result.content["pid"], Value::Null);
}

#[tokio::test]
async fn managed_preview_restart_uses_fresh_port_and_latest_static_output() {
    let workspace = setup_workspace();
    write_frozen_candidate_build_state(
        &workspace,
        "build-preview-first",
        ArtifactRouteContract::website(),
        &[("index.html", "first build")],
    );
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let first = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new("preview-first", "preview.start", json!({}))],
        )
        .await;
    assert!(!first[0].result.is_error);
    let first_url = first[0].result.content["url"].as_str().unwrap().to_string();
    assert_eq!(
        reqwest::get(&first_url)
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "first build"
    );

    write_frozen_candidate_build_state(
        &workspace,
        "build-preview-second",
        ArtifactRouteContract::website(),
        &[("index.html", "second build")],
    );
    let second = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new("preview-second", "preview.start", json!({}))],
        )
        .await;
    assert!(!second[0].result.is_error);
    let second_url = second[0].result.content["url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first_url, second_url);
    assert_eq!(
        reqwest::get(&second_url)
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "second build"
    );

    let stopped = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new("preview-stop", "preview.stop", json!({}))],
        )
        .await;
    assert!(!stopped[0].result.is_error);
}

#[tokio::test]
async fn preview_start_spawns_static_server_from_fumadocs_out() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("state/project.json"),
        json!({
            "appRoot": "project",
            "templateKey": "fumadocs-docs",
            "template": "fumadocs-docs"
        })
        .to_string(),
    )
    .unwrap();
    write_frozen_candidate_build_state(
        &workspace,
        "build-preview-docs",
        ArtifactRouteContract::docs(),
        &[(
            "docs/index.html",
            "<!doctype html><title>Docs</title><h1>Docs Ready</h1>",
        )],
    );
    let port = free_tcp_port();
    let preview_url = format!("http://127.0.0.1:{port}");
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "preview.start",
                    json!({ "url": preview_url, "port": port }),
                ),
                ToolCall::new("tool-2", "preview.stop", json!({})),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[0].result.content["status"], "running");
    assert_eq!(results[0].result.content["accessible"], true);
    assert_eq!(
        results[0].result.content["staticOutputPath"],
        "/workspace/outputs/candidates/build-preview-docs"
    );
    assert_eq!(results[1].result.content["status"], "stopped");
}

#[tokio::test]
async fn preview_start_requires_frozen_candidate_when_it_must_manage_server() {
    let workspace = setup_workspace();
    write_successful_build_state(&workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-1",
                "preview.start",
                json!({ "url": "http://127.0.0.1:9", "port": 9 }),
            )],
        )
        .await;

    assert!(results[0].result.is_error);
    assert!(tool_result_error_text(&results[0].result).contains("buildId evidence"));
    assert!(!workspace.join("state/preview.json").exists());
}

#[tokio::test]
async fn diagnostics_read_build_log_and_typescript_report() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("outputs/build/build.log"),
        "Build failed\nError: missing import",
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/reports/typescript.json"),
        json!({ "ok": false, "diagnostics": [{ "message": "Type mismatch" }] }).to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new("tool-1", "diagnostics.build_log", json!({})),
                ToolCall::new("tool-2", "diagnostics.typescript", json!({})),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[0].result.content["hasTerminalError"], true);
    assert_eq!(results[1].result.content["ok"], false);
    assert_eq!(
        results[1].result.content["diagnostics"][0]["message"],
        "Type mismatch"
    );

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn design_context_fidelity_diagnostics_fail_closed_and_filter_findings() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let forged = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "diagnostics-forged-report",
                "fs.write",
                json!({
                    "path": "state/design-profile-fidelity.json",
                    "text": r#"{"status":"passed"}"#
                }),
            )],
        )
        .await;
    assert!(forged[0].result.is_error);
    assert_error_kind(&forged[0].result, "path.runtime_owned");

    let missing = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "diagnostics-missing-report",
                "diagnostics.accessibility",
                json!({}),
            )],
        )
        .await;
    assert!(missing[0].result.is_error);
    assert_error_kind(&missing[0].result, "design_context.fidelity_report_missing");

    fs::write(
        workspace.join("state/design-profile-fidelity.json"),
        serde_json::to_vec_pretty(&json!({
            "version": "design-profile-fidelity@2",
            "status": "failed",
            "internalOnly": "must-not-leak",
            "assertions": [
                {
                    "ruleId": "a11y-button-name",
                    "kind": "a11y",
                    "passed": false,
                    "reason": "button has no accessible name"
                },
                {
                    "ruleId": "viewport-375",
                    "kind": "viewport",
                    "passed": false,
                    "reason": "horizontal overflow"
                },
                {
                    "ruleId": "token-primary",
                    "kind": "token",
                    "passed": true,
                    "privateValue": "must-not-leak"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let results = executor
        .execute_calls(
            store.clone(),
            &run_id,
            vec![
                ToolCall::new("diagnostics-a11y", "diagnostics.accessibility", json!({})),
                ToolCall::new(
                    "diagnostics-responsive",
                    "preview.audit_responsive",
                    json!({}),
                ),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(
        results[0].result.content,
        json!({
            "reportVersion": "design-profile-fidelity@2",
            "status": "failed",
            "kind": "a11y",
            "findings": [{
                "ruleId": "a11y-button-name",
                "kind": "a11y",
                "passed": false,
                "reason": "button has no accessible name"
            }]
        })
    );
    assert_eq!(
        results[1].result.content,
        json!({
            "reportVersion": "design-profile-fidelity@2",
            "status": "failed",
            "kind": "viewport",
            "findings": [{
                "ruleId": "viewport-375",
                "kind": "viewport",
                "passed": false,
                "reason": "horizontal overflow"
            }]
        })
    );
    let serialized = serde_json::to_string(
        &results
            .iter()
            .map(|result| &result.result.content)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!serialized.contains("internalOnly"));
    assert!(!serialized.contains("privateValue"));
    assert!(!serialized.contains("must-not-leak"));

    let mutation_attempts = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "diagnostics-patch-report",
                    "fs.patch",
                    json!({
                        "path": "state/design-profile-fidelity.json",
                        "oldStr": "failed",
                        "newStr": "passed"
                    }),
                ),
                ToolCall::new(
                    "diagnostics-delete-report",
                    "fs.delete",
                    json!({ "path": "state/design-profile-fidelity.json" }),
                ),
            ],
        )
        .await;
    assert!(mutation_attempts[0].result.is_error);
    assert!(
        tool_result_error_text(&mutation_attempts[0].result)
            .contains("runtime-owned path cannot be mutated"),
        "{}",
        tool_result_error_text(&mutation_attempts[0].result)
    );
    assert!(mutation_attempts[1].result.is_error);
    assert!(
        tool_result_error_text(&mutation_attempts[1].result)
            .contains("fs.delete is limited to non-root paths under /workspace/project"),
        "{}",
        tool_result_error_text(&mutation_attempts[1].result)
    );

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn browser_open_screenshot_and_inspect_use_workspace_state() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![
                ToolCall::new(
                    "tool-1",
                    "browser.open",
                    json!({ "url": "http://127.0.0.1:4321" }),
                ),
                ToolCall::new(
                    "tool-2",
                    "browser.screenshot",
                    json!({ "screenshotId": "shot-1", "blank": false }),
                ),
                ToolCall::new("tool-3", "browser.inspect", json!({})),
            ],
        )
        .await;

    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[1].result.content["screenshotId"], "shot-1");
    assert_eq!(results[1].result.content["blank"], false);
    assert_eq!(results[2].result.content["url"], "http://127.0.0.1:4321");
    assert_eq!(results[2].result.content["opened"], true);
    assert!(workspace.join("outputs/screenshots/shot-1.json").exists());
}

#[test]
fn execution_tools_are_registered_with_expected_concurrency_flags() {
    let workspace = setup_workspace();
    let executor = sandbox_executor(&workspace);
    let tracked = executor.track_calls(vec![
        ToolCall::new("tool-1", "preview.status", json!({})),
        ToolCall::new("tool-2", "diagnostics.build_log", json!({})),
        ToolCall::new("tool-3", "diagnostics.typescript", json!({})),
        ToolCall::new("tool-4", "diagnostics.accessibility", json!({})),
        ToolCall::new("tool-5", "preview.audit_responsive", json!({})),
        ToolCall::new("tool-6", "design_context.status", json!({})),
        ToolCall::new("tool-7", "browser.inspect", json!({})),
        ToolCall::new("tool-8", "preview.start", json!({})),
        ToolCall::new("tool-9", "browser.screenshot", json!({})),
    ]);

    for call in &tracked[..7] {
        assert!(call.is_concurrency_safe, "{}", call.name);
    }
    assert!(!tracked[7].is_concurrency_safe);
    assert!(!tracked[8].is_concurrency_safe);
}

fn setup_workspace() -> PathBuf {
    let workspace = unique_temp_dir("sandbox-tools");
    fs::create_dir_all(workspace.join("project")).unwrap();
    fs::create_dir_all(workspace.join("inputs")).unwrap();
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::create_dir_all(workspace.join("outputs")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::create_dir_all(workspace.join("outputs/reports")).unwrap();
    fs::create_dir_all(workspace.join("outputs/screenshots")).unwrap();
    fs::write(workspace.join("project").join("index.md"), "hello").unwrap();
    workspace
}

fn write_successful_build_state(workspace: &Path) {
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "success": true,
            "cwd": "/workspace/project",
            "argv": ["npm", "run", "build"],
            "logPath": "/workspace/outputs/build/build.log"
        })
        .to_string(),
    )
    .unwrap();
}

fn write_frozen_candidate_build_state(
    workspace: &Path,
    build_id: &str,
    contract: ArtifactRouteContract,
    files: &[(&str, &str)],
) {
    let candidate = workspace.join("outputs/candidates").join(build_id);
    fs::create_dir_all(&candidate).unwrap();
    let mut route_files = Vec::new();
    let mut manifest_files = Vec::new();
    for (path, content) in files {
        let output = candidate.join(path);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, content).unwrap();
        let hash = sha256_hex(content.as_bytes());
        route_files.push(ArtifactRouteFile {
            path: (*path).to_string(),
            sha256: hash.clone(),
        });
        manifest_files.push(json!({
            "path": path,
            "bytes": content.len(),
            "sha256": hash,
        }));
    }
    let route_manifest = ArtifactRouteManifest::build(build_id, &contract, route_files).unwrap();
    let route_manifest_text = serde_json::to_string_pretty(&route_manifest).unwrap();
    let route_manifest_hash = sha256_hex(route_manifest_text.as_bytes());
    fs::write(
        candidate.join(".anydesign-artifact-routes.json"),
        &route_manifest_text,
    )
    .unwrap();
    manifest_files.push(json!({
        "path": ".anydesign-artifact-routes.json",
        "bytes": route_manifest_text.len(),
        "sha256": route_manifest_hash,
    }));
    manifest_files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let candidate_manifest = serde_json::to_string_pretty(&json!({
        "schemaVersion": "candidate-manifest@1",
        "buildId": build_id,
        "artifactRouteManifestPath": ".anydesign-artifact-routes.json",
        "artifactRouteManifestHash": route_manifest_hash,
        "files": manifest_files,
    }))
    .unwrap();
    let candidate_manifest_hash = sha256_hex(candidate_manifest.as_bytes());
    fs::write(
        candidate.join(".anydesign-candidate-manifest.json"),
        candidate_manifest,
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        serde_json::to_string_pretty(&json!({
            "buildId": build_id,
            "status": "success",
            "success": true,
            "cwd": "/workspace/project",
            "candidateOutputPath": format!("/workspace/outputs/candidates/{build_id}"),
            "candidateManifestHash": candidate_manifest_hash,
            "artifactRouteManifestPath": format!("/workspace/outputs/candidates/{build_id}/.anydesign-artifact-routes.json"),
            "artifactRouteManifestHash": route_manifest_hash,
        }))
        .unwrap(),
    )
    .unwrap();
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

fn free_tcp_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn wait_for_tcp_port(port: u16) {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("workspace channel server did not listen on port {port}");
}

async fn start_preview_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        const BODY: &[u8] = br#"<!doctype html>
<html lang="en">
  <head>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Runtime preview fixture</title>
  </head>
  <body>
    <nav aria-label="Primary"><a href="/">Home</a></nav>
    <button type="button" aria-label="Search">Search</button>
    <main><h1>Runtime preview fixture</h1><p>Validated preview content.</p></main>
  </body>
</html>"#;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                BODY.len()
            );
            let _ = stream.write_all(&[header.as_bytes(), BODY].concat()).await;
        }
    });
    (format!("http://{}", addr), handle)
}

async fn start_asset_generation_provider() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let content = base64::engine::general_purpose::STANDARD.encode(one_pixel_visual_asset());
    let body = json!({
        "contentBase64": content,
        "providerIdentity": "asset-provider-test",
        "modelVersion": "image-model@1",
        "license": "provider-test-license",
    })
    .to_string();
    let handle = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream
                .write_all(&[header.as_bytes(), body.as_bytes()].concat())
                .await;
        }
    });
    (format!("http://{addr}"), handle)
}

async fn start_workspace_channel_server() -> (String, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let requests = server_requests.clone();
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
                let op = request["op"].as_str().unwrap_or("");
                let path = request["path"].as_str().unwrap_or("");
                let result = match op {
                    "fs.read" => json!({ "text": format!("websocket:{path}") }),
                    "fs.write" => {
                        json!({ "bytes": request["payload"]["text"].as_str().unwrap_or("").len() })
                    }
                    _ => json!({}),
                };
                let response = json!({ "ok": true, "result": result }).to_string();
                let _ = socket.send(Message::Text(response.into())).await;
            });
        }
    });
    (format!("ws://{}", addr), requests, handle)
}
