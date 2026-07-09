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

fn sandbox_executor_local_e2e(workspace: &Path) -> StreamingToolExecutor {
    StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(sandbox_tools(), Default::default(), workspace)
            .with_policy_profile_and_registry(
                RuntimePolicyProfile::LocalE2e,
                "https://registry.npmjs.org/",
            ),
    )
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
            "fs.copyDir" => json!({ "copied": true }),
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
async fn fs_read_directory_failure_has_structured_metadata() {
    let workspace = setup_workspace();
    fs::create_dir_all(workspace.join("project/subdir")).unwrap();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor(&workspace);

    let results = executor
        .execute_calls(
            store,
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
async fn fs_list_missing_directory_failure_has_structured_metadata() {
    let workspace = setup_workspace();
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = sandbox_executor_local_e2e(&workspace);

    let results = executor
        .execute_calls(
            store,
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
    assert!(page.contains("class=\"runtime-page saas\""));
    assert!(page.contains("import '../styles/global.css';"));
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
    assert!(page.contains("import '../styles/global.css';"));
    assert!(page.contains("class=\"runtime-section\""));
    assert!(!page.contains("<style>"));
    assert!(!page.contains("old page"));
}

#[tokio::test]
async fn project_init_astro_website_writes_style_contract_and_tokens() {
    let workspace = setup_workspace();
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
                json!({ "template": "astro-website" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );
    assert_eq!(results[0].result.content["template"], "astro-website");
    assert_eq!(
        results[0].result.content["styleContractPath"],
        "/workspace/state/style-contract.json"
    );

    let package_json = fs::read_to_string(workspace.join("project/package.json")).unwrap();
    assert!(package_json.contains("tailwindcss"));
    let index = fs::read_to_string(workspace.join("project/src/pages/index.astro")).unwrap();
    assert!(index.contains("import '../styles/global.css';"));
    let button_component =
        fs::read_to_string(workspace.join("project/src/components/ui/Button.astro")).unwrap();
    assert!(button_component.contains("runtime-button"));
    assert!(button_component.contains("Astro.props"));

    let global_css = fs::read_to_string(workspace.join("project/src/styles/global.css")).unwrap();
    assert!(global_css.contains("@import \"tailwindcss\""));
    assert!(global_css.contains("@import \"./tokens.css\""));
    assert!(global_css.contains("var(--runtime-primary)"));
    assert!(global_css.contains("var(--runtime-radius-control)"));

    let tokens = fs::read_to_string(workspace.join("project/src/styles/tokens.css")).unwrap();
    assert!(tokens.contains("--runtime-primary: #2563eb"));
    assert!(tokens.contains("--runtime-radius-card: 8px"));

    let contract: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("state/style-contract.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(contract["template"], "astro-website");
    assert_eq!(
        contract["tokenFile"],
        "/workspace/project/src/styles/tokens.css"
    );
    assert_eq!(
        contract["globalCssFile"],
        "/workspace/project/src/styles/global.css"
    );
    assert_eq!(
        contract["componentRoot"],
        "/workspace/project/src/components/ui"
    );
    assert_eq!(contract["tailwind"]["version"], "4");
    assert_eq!(
        contract["tailwind"]["entryImport"],
        "@import \"tailwindcss\""
    );
    assert_eq!(contract["tailwind"]["themeSource"], "css-variables");
    assert_eq!(contract["tokens"]["color.primary"], "--runtime-primary");

    let state: Value =
        serde_json::from_str(&fs::read_to_string(workspace.join("state/project.json")).unwrap())
            .unwrap();
    assert_eq!(state["templateVersion"], "astro-website@runtime-p2");
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
                    json!({ "template": "astro-website" }),
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
        "/workspace/project/src/styles/tokens.css"
    );
    assert_eq!(
        results[1].result.content["changes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let tokens = fs::read_to_string(workspace.join("project/src/styles/tokens.css")).unwrap();
    assert!(tokens.contains("--runtime-primary: #f37a0a;"));
    assert!(tokens.contains("--runtime-primary-contrast: #111827;"));
    assert!(tokens.contains("--runtime-radius-card: 6px;"));
    assert!(!tokens.contains("--runtime-primary: #2563eb;"));
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
                    json!({ "template": "astro-website" }),
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
    let tokens = fs::read_to_string(workspace.join("project/src/styles/tokens.css")).unwrap();
    assert!(tokens.contains("--runtime-primary: #2563eb;"));
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
                json!({ "template": "astro-website" }),
            )],
        )
        .await;
    assert!(!init[0].result.is_error);
    let token_path = workspace.join("project/src/styles/tokens.css");
    let mut tokens = fs::read_to_string(&token_path).unwrap();
    tokens = tokens.replace("--runtime-primary:", "--runtime-primary-missing:");
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
        Some("--runtime-primary")
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
                    json!({ "template": "astro-website" }),
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
    assert_eq!(inspected["framework"], "astro");
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
        .any(
            |file| file["path"] == "/workspace/project/src/styles/tokens.css"
                && file["exists"] == true
        ));
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
    let global_css = fs::read_to_string(workspace.join("project/app/global.css")).unwrap();
    assert!(global_css.contains("@import 'tailwindcss'"));
    assert!(global_css.contains("@import './tokens.css'"));
    assert!(global_css.contains("var(--runtime-primary)"));
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
    assert_eq!(contract["tokens"]["color.primary"], "--runtime-primary");
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
        "---\ntitle: Runtime Flow\ndescription: Build and edit lifecycle\n---\n\n# Runtime Flow\n\nThe runtime creates, builds, previews, and promotes docs.\n",
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
        .starts_with("file:///workspace/outputs/build/source-snapshots/build-"));

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
                json!({ "template": "astro-website" }),
            )],
        )
        .await;
    assert_eq!(website_results.len(), 1);
    assert!(!website_results[0].result.is_error);
    assert!(workspace.join("project/src/pages/index.astro").exists());
    assert!(workspace.join("project/astro.config.mjs").exists());

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
    assert!(!workspace.join("project/src/pages/index.astro").exists());
    assert!(!workspace.join("project/astro.config.mjs").exists());
    assert!(workspace
        .join("project/app/docs/[[...slug]]/page.jsx")
        .exists());
    let docs_tsconfig = fs::read_to_string(workspace.join("project/tsconfig.json")).unwrap();
    assert!(docs_tsconfig.contains("\"plugins\": [{ \"name\": \"next\" }]"));
    assert!(!docs_tsconfig.contains("astro/tsconfigs/strict"));

    let website_again_results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-init-website-again",
                "project.init",
                json!({ "template": "astro-website" }),
            )],
        )
        .await;
    assert_eq!(website_again_results.len(), 1);
    assert!(!website_again_results[0].result.is_error);
    assert!(workspace.join("project/src/pages/index.astro").exists());
    assert!(workspace.join("project/astro.config.mjs").exists());
    assert!(!workspace
        .join("project/app/docs/[[...slug]]/page.jsx")
        .exists());
    assert!(!workspace.join("project/content/docs/index.mdx").exists());
    assert!(!workspace.join("project/next.config.mjs").exists());
    let website_tsconfig = fs::read_to_string(workspace.join("project/tsconfig.json")).unwrap();
    assert!(website_tsconfig.contains("astro/tsconfigs/strict"));
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
            ],
        )
        .await;

    assert_eq!(results.len(), 3);
    assert!(!results[0].result.is_error);
    assert!(results[1].result.is_error);
    assert_error_kind(&results[1].result, "docs.routing_root_forbidden");
    assert!(results[2].result.is_error);
    assert_error_kind(&results[2].result, "docs.routing_root_forbidden");
    assert!(!workspace.join("project/pages/index.jsx").exists());
    assert!(!workspace.join("project/src/pages/index.jsx").exists());
}

#[tokio::test]
async fn project_build_accepts_valid_fumadocs_docs_source_contract() {
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
        .contains("file:///workspace/outputs/build/source-snapshots/"));
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
async fn project_build_missing_command_failure_has_structured_metadata() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "scripts": { "build": "next build" }
        }))
        .unwrap(),
    )
    .unwrap();
    let transport = ExecBehaviorTransport::new(ExecBehavior::Output {
        status: 127,
        success: false,
        stdout: String::new(),
        stderr: "sh: next: command not found".to_string(),
    });
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
                "tool-build",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "build.missing_dependency");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(metadata["exitCode"], 127);
    assert!(metadata["stderr"]
        .as_str()
        .unwrap()
        .contains("next: command not found"));
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
async fn project_build_auto_restores_missing_dependencies_before_build() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "scripts": { "build": "astro build" },
            "dependencies": { "astro": "^5.16.4" }
        }))
        .unwrap(),
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
            store.clone(),
            &run_id,
            vec![ToolCall::new(
                "tool-build",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(
        results[0].result.content["dependencyRestoreAttempted"],
        true
    );
    assert_eq!(
        results[0].result.content["dependencyRestoreSucceeded"],
        true
    );
    assert_eq!(
        results[0].result.content["dependencyRestoreReason"],
        "node_modules_missing"
    );
    let requests = transport.requests.lock().unwrap().clone();
    let execs = requests
        .iter()
        .filter(|request| request.op == "process.exec")
        .collect::<Vec<_>>();
    assert_eq!(execs.len(), 2);
    assert!(execs[0].payload["argv"]
        .as_array()
        .is_some_and(|argv| argv[0] == "npm" && argv[1] == "install"));
    assert!(execs[1].payload["argv"]
        .as_array()
        .is_some_and(|argv| argv[0] == "npm" && argv[1] == "run" && argv[2] == "build"));
    let events = store.events(&run_id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolOutput { tool, text, .. }
            if tool == "package.install"
                && text.contains("runtime dependency restore before project.build")
    )));
}

#[tokio::test]
async fn project_build_dependency_restore_policy_denial_is_typed_recoverable() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "scripts": { "build": "astro build" },
            "dependencies": { "astro": "^5.16.4" }
        }))
        .unwrap(),
    )
    .unwrap();
    let transport = RecordingChannelTransport::default();
    let command_backend = JsonWorkspaceChannelCommandBackend::new(transport.clone(), &workspace);
    let store = RuntimeStore::new();
    let run_id = create_run(&store).await;
    let executor = StreamingToolExecutor::new(
        ToolExecutor::new_with_workspace_root(
            sandbox_tools_with_backends(Arc::new(LocalWorkspaceBackend), Arc::new(command_backend)),
            Default::default(),
            &workspace,
        )
        .with_policy_profile_and_registry(
            RuntimePolicyProfile::Production,
            "https://registry.npmjs.org",
        ),
    );

    let results = executor
        .execute_calls(
            store,
            &run_id,
            vec![ToolCall::new(
                "tool-build",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_error);
    assert_error_kind(&results[0].result, "build.missing_dependency");
    let metadata = results[0].result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("registry").and_then(Value::as_str),
        Some("https://registry.npmjs.org")
    );
    let requests = transport.requests.lock().unwrap().clone();
    assert!(!requests.iter().any(|request| request.op == "process.exec"));
}

#[tokio::test]
async fn project_build_uses_project_package_manager_for_build_command() {
    let workspace = setup_workspace();
    fs::write(
        workspace.join("project/package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "sandbox-project",
            "private": true,
            "scripts": { "build": "vite build" }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/project.json"),
        json!({ "appRoot": "project", "packageManager": "pnpm" }).to_string(),
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
                "tool-build",
                "project.build",
                json!({ "cwd": "project" }),
            )],
        )
        .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].result.is_error);
    assert_eq!(results[0].result.content["packageManager"], "pnpm");
    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.payload["argv"]
                .as_array()
                .is_some_and(|argv| argv[0] == "pnpm" && argv[1] == "run" && argv[2] == "build")
    }));
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
    assert_error_kind(&results[0].result, "patch.stale_read");
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello\nexternal edit\n");
}

#[tokio::test]
async fn fs_multi_patch_applies_multiple_edits_atomically() {
    let workspace = setup_workspace();
    let file = workspace.join("project").join("page.astro");
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
                    json!({ "path": "project/page.astro" }),
                ),
                ToolCall::new(
                    "tool-multi-patch",
                    "fs.multi_patch",
                    json!({
                        "path": "project/page.astro",
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
    let file = workspace.join("project").join("page.astro");
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
                    json!({ "path": "project/page.astro" }),
                ),
                ToolCall::new(
                    "tool-multi-patch",
                    "fs.multi_patch",
                    json!({
                        "path": "project/page.astro",
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
    assert_error_kind(&results[0].result, "patch.read_required");
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello\nworld\n");
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
async fn preview_start_spawns_static_server_from_fumadocs_out() {
    let workspace = setup_workspace();
    write_successful_build_state(&workspace);
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
    fs::create_dir_all(workspace.join("project/out")).unwrap();
    fs::write(
        workspace.join("project/out/index.html"),
        "<!doctype html><title>Docs</title><h1>Docs Ready</h1>",
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
    assert_eq!(
        results[0].result.content["staticOutputPath"],
        "/workspace/project/out"
    );
    assert_eq!(results[1].result.content["status"], "stopped");
}

#[tokio::test]
async fn preview_publish_builds_screenshots_and_promotes_candidate() {
    let workspace = setup_workspace();
    let (preview_url, _preview_server) = start_preview_server().await;
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
            store.clone(),
            &run_id,
            vec![
                ToolCall::new(
                    "tool-init",
                    "project.init",
                    json!({ "template": "astro-website" }),
                ),
                ToolCall::new(
                    "tool-publish",
                    "preview.publish",
                    json!({
                        "url": preview_url,
                        "screenshotId": "publish-shot"
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
    assert_eq!(results[1].result.content["published"], true);
    assert_eq!(results[1].result.content["build"]["success"], true);
    assert_eq!(
        results[1].result.content["screenshot"]["screenshotId"],
        "publish-shot"
    );
    assert_eq!(results[1].result.content["promotion"]["status"], "promoted");
    assert!(workspace
        .join("outputs/screenshots/publish-shot.json")
        .exists());

    let requests = transport.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| {
        request.op == "process.exec"
            && request.path == "/workspace/project"
            && request.payload["argv"]
                .as_array()
                .is_some_and(|argv| argv[0] == "npm" && argv[1] == "run" && argv[2] == "build")
    }));
    let event_types = store
        .events(&run_id)
        .await
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"preview.candidate".to_string()));
    assert!(event_types.contains(&"preview.updated".to_string()));
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
