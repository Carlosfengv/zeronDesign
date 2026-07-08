use anydesign_runtime::{
    config::RuntimePolicyProfile,
    conversation::RuntimeStore,
    model_gateway::ToolCall,
    tools::{
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
    types::{AgentEvent, AgentPhase, AgentRunStatus, SandboxChannelProtocol},
};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
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

fn sandbox_executor(workspace: &Path) -> StreamingToolExecutor {
    StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        Default::default(),
        workspace,
    ))
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
            "fs.read" => json!({ "text": format!("remote:{}", request.path) }),
            "fs.write" => json!({ "bytes": request.payload["text"].as_str().unwrap().len() }),
            "fs.list" => json!({
                "entries": [
                    { "name": "child.md", "kind": "file" }
                ]
            }),
            "fs.stat" => {
                if request.path.ends_with("project") {
                    json!({ "kind": "dir" })
                } else {
                    json!({ "kind": "file" })
                }
            }
            "fs.removeFile" | "fs.removeDirAll" => json!({ "deleted": true }),
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
}

#[tokio::test]
async fn fs_read_write_list_and_search_are_workspace_bounded() {
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
    assert!(results.iter().all(|result| !result.result.is_error));
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
    assert!(results.iter().all(|result| !result.result.is_error));
    assert_eq!(results[0].result.content["text"], "remote channel text");
    assert!(backend.reads.lock().unwrap()[0].ends_with("project/existing.md"));
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert!(writes[0].0.ends_with("project/generated.md"));
    assert_eq!(writes[0].1, "written through backend");
    assert!(!workspace.join("project/generated.md").exists());
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
    assert!(!results[0].result.is_error);
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
    assert_eq!(appended[1].result.content["mode"], "append");
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
async fn project_write_page_renders_structured_sections_to_astro_route() {
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
        "/workspace/project/src/pages/pricing.astro"
    );
    let page = fs::read_to_string(workspace.join("project/src/pages/pricing.astro")).unwrap();
    assert!(page.contains("Runtime Plans"));
    assert!(page.contains("Launch sites with controlled runtime tools"));
    assert!(page.contains("Observable by default"));
    assert!(page.contains("class=\"saas\""));
}

#[tokio::test]
async fn project_write_page_root_route_overwrites_existing_index_page() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/src/pages")).unwrap();
    fs::write(
        workspace.join("project/src/pages/index.astro"),
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
            vec![ToolCall::new(
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
                            "body": "project.write_page must overwrite src/pages/index.astro for /.",
                            "visual": "Index page"
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
        "/workspace/project/src/pages/index.astro"
    );
    let page_path = workspace.join("project/src/pages/index.astro");
    assert!(page_path.is_file());
    let page = fs::read_to_string(page_path).unwrap();
    assert!(page.contains("Root Runtime Page"));
    assert!(!page.contains("old page"));
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
                    json!({ "mode": "restore", "packages": ["astro"] }),
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
    assert_eq!(ops.iter().filter(|op| **op == "fs.read").count(), 2);
    assert_eq!(ops.iter().filter(|op| **op == "fs.list").count(), 2);
    assert_eq!(ops.iter().filter(|op| **op == "fs.stat").count(), 3);
    assert_eq!(ops.iter().filter(|op| **op == "fs.write").count(), 1);
    assert_eq!(ops.iter().filter(|op| **op == "fs.removeFile").count(), 1);
    assert!(requests
        .iter()
        .all(|request| request.path.starts_with("/workspace/")));
    let write = requests
        .iter()
        .find(|request| request.op == "fs.write")
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
            "anydesign-astro-website-pool".to_string(),
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
    assert_eq!(seen_sandbox_ids.lock().unwrap().as_slice(), [binding.id]);
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
    let backend = JsonWorkspaceChannelBackend::new(
        WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(Duration::from_secs(2)),
        &workspace,
    );
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
    assert!(store
        .events(&run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "permission.denied"));
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
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello\nworld\n");
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
                ToolCall::new("tool-2", "fs.delete", json!({ "path": "project/old.md" })),
            ],
        )
        .await;

    assert!(results[0].result.is_error);
    assert!(!results[1].result.is_error);
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
async fn preview_start_spawns_static_server_from_dist() {
    let workspace = setup_workspace();
    write_successful_build_state(&workspace);
    fs::create_dir_all(workspace.join("project/dist")).unwrap();
    fs::write(
        workspace.join("project/dist/index.html"),
        "<!doctype html><title>Preview</title><h1>Ready</h1>",
    )
    .unwrap();
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
async fn preview_start_requires_dist_when_it_must_manage_server() {
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
    assert!(tool_result_error_text(&results[0].result).contains("missing dist"));
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
        ToolCall::new("tool-4", "browser.inspect", json!({})),
        ToolCall::new("tool-5", "preview.start", json!({})),
        ToolCall::new("tool-6", "browser.screenshot", json!({})),
    ]);

    assert!(tracked[0].is_concurrency_safe);
    assert!(tracked[1].is_concurrency_safe);
    assert!(tracked[2].is_concurrency_safe);
    assert!(tracked[3].is_concurrency_safe);
    assert!(!tracked[4].is_concurrency_safe);
    assert!(!tracked[5].is_concurrency_safe);
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
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    (format!("http://{}", addr), handle)
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
