use crate::{
    config::RuntimePolicyProfile,
    permission::{
        check_command_policy, check_create_path, check_existing_path, check_workspace_path,
        PermissionError, PermissionReason, PermissionResult, RuleSource,
    },
    preview::{promote_preview, PromotionGateReport},
    sandbox_adapter::sandbox_channel_from_binding,
    tools::{
        runtime::{ProgressSink, Tool, ToolContext, ToolError, ToolResult, ValidationError},
        schema::{object_schema, string_schema},
    },
    types::{AgentEvent, AgentPhase, AgentRunStatus},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    net::TcpStream,
    process::{Child, Command as TokioCommand},
    sync::Mutex,
    time,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const MAX_DIRECT_WRITE_ARGUMENT_BYTES: usize = 96_000;
const MAX_DIRECT_WRITE_TEXT_CHARS: usize = 48_000;
const MAX_CHUNK_ARGUMENT_BYTES: usize = 48_000;
const MAX_CHUNK_TEXT_CHARS: usize = 24_000;
const MAX_CHUNKS_PER_WRITE: u64 = 512;
const STAGED_WRITE_TTL_SECS: i64 = 24 * 60 * 60;
const LARGE_WRITE_GUIDANCE: &str = "fs.write input is too large for direct tool-call JSON. Use fs.patch for existing files or fs.write_chunk/fs.commit_chunks for large new files. Do not retry the same full fs.write payload.";

pub fn sandbox_tools() -> Vec<Arc<dyn Tool>> {
    sandbox_tools_with_workspace_backend(Arc::new(LocalWorkspaceBackend))
}

pub fn sandbox_tools_with_workspace_backend(
    workspace_backend: Arc<dyn WorkspaceBackend>,
) -> Vec<Arc<dyn Tool>> {
    sandbox_tools_with_backends(workspace_backend, Arc::new(LocalCommandBackend))
}

pub fn sandbox_tools_with_backends(
    workspace_backend: Arc<dyn WorkspaceBackend>,
    command_backend: Arc<dyn SandboxCommandBackend>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(FsReadTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsListTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsSearchTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsWriteTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsWriteChunkTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsCommitChunksTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsPatchTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsMultiPatchTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(StyleUpdateTokensTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsDeleteTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(ShellRunTool {
            command: command_backend.clone(),
        }),
        Arc::new(ProjectInitTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(ProjectWritePageTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(ProjectInspectTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(ProjectBuildTool {
            workspace: workspace_backend.clone(),
            command: command_backend.clone(),
        }),
        Arc::new(ProjectEnsureDependenciesTool {
            workspace: workspace_backend.clone(),
            command: command_backend.clone(),
        }),
        Arc::new(PackageInstallTool {
            workspace: workspace_backend.clone(),
            command: command_backend.clone(),
        }),
        Arc::new(PreviewRebuildingTool),
        Arc::new(PreviewReportCandidateTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(PreviewPublishTool {
            workspace: workspace_backend.clone(),
            command: command_backend.clone(),
        }),
        Arc::new(PreviewStartTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(PreviewStatusTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(PreviewStopTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(DiagnosticsBuildLogTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(DiagnosticsTypescriptTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(BrowserOpenTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(BrowserScreenshotTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(BrowserInspectTool {
            workspace: workspace_backend,
        }),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: WorkspaceEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEntryKind {
    File,
    Dir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePathKind {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCommandOutput {
    pub status: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait SandboxCommandBackend: Send + Sync {
    async fn run(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput>;

    async fn run_with_output_events(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
        progress: Option<ProgressSink>,
        tool_name: &str,
    ) -> io::Result<SandboxCommandOutput> {
        let output = self.run(ctx, argv, cwd, timeout_ms).await?;
        if let Some(progress) = progress {
            progress
                .emit_tool_output(tool_name, "stdout", output.stdout.clone())
                .await;
            progress
                .emit_tool_output(tool_name, "stderr", output.stderr.clone())
                .await;
        }
        Ok(output)
    }
}

#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    async fn read_to_string(&self, ctx: &ToolContext, path: &Path) -> io::Result<String>;
    async fn write_string(&self, ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()>;
    async fn rename(&self, ctx: &ToolContext, from: &Path, to: &Path) -> io::Result<()> {
        let text = self.read_to_string(ctx, from).await?;
        self.write_string(ctx, to, &text).await?;
        self.remove_file(ctx, from).await
    }
    async fn list_dir(&self, ctx: &ToolContext, path: &Path) -> io::Result<Vec<WorkspaceEntry>>;
    async fn path_kind(&self, ctx: &ToolContext, path: &Path) -> io::Result<WorkspacePathKind>;
    async fn remove_file(&self, ctx: &ToolContext, path: &Path) -> io::Result<()>;
    async fn remove_dir_all(&self, ctx: &ToolContext, path: &Path) -> io::Result<()>;
    async fn copy_dir_all(
        &self,
        ctx: &ToolContext,
        from: &Path,
        to: &Path,
        skip_dir_names: &[String],
    ) -> io::Result<()>;
}

#[async_trait]
pub trait WorkspaceChannelTransport: Send + Sync {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value>;
}

#[async_trait]
pub trait WorkspaceChannelEndpointResolver: Send + Sync {
    async fn endpoint(&self, ctx: &ToolContext) -> io::Result<String>;
}

#[derive(Debug, Clone, Default)]
pub struct SandboxBindingEndpointResolver;

#[async_trait]
impl WorkspaceChannelEndpointResolver for SandboxBindingEndpointResolver {
    async fn endpoint(&self, ctx: &ToolContext) -> io::Result<String> {
        let sandbox_id = ctx.run.sandbox_id.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "run is not bound to a sandbox channel",
            )
        })?;
        let binding = ctx
            .store
            .get_sandbox_binding(sandbox_id)
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "sandbox binding not found"))?;
        let channel = sandbox_channel_from_binding(&binding)
            .map_err(|error| io::Error::new(io::ErrorKind::NotConnected, error))?;
        Ok(channel.endpoint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceChannelRequest {
    pub op: &'static str,
    pub path: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct WebSocketWorkspaceChannelTransport {
    endpoint: String,
    timeout: Duration,
}

impl WebSocketWorkspaceChannelTransport {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl WorkspaceChannelTransport for WebSocketWorkspaceChannelTransport {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value> {
        let endpoint = self.endpoint.clone();
        let timeout = self.timeout;
        time::timeout(timeout, async move {
            let mut last_error = None;
            for attempt in 1..=3 {
                match websocket_channel_request_once(&endpoint, request.clone()).await {
                    Ok(value) => return Ok(value),
                    Err(error) if is_transient_workspace_channel_error(&error) && attempt < 3 => {
                        last_error = Some(error);
                        time::sleep(Duration::from_millis(25 * attempt)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(last_error.unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "workspace channel request aborted before execution",
                )
            }))
        })
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "workspace channel request timed out",
            )
        })?
    }
}

async fn websocket_channel_request_once(
    endpoint: &str,
    request: WorkspaceChannelRequest,
) -> io::Result<Value> {
    let (mut socket, _) = connect_async(endpoint)
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))?;
    let payload = serde_json::to_string(&json!({
        "op": request.op,
        "path": request.path,
        "payload": request.payload,
    }))
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
    let message = socket
        .next()
        .await
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "workspace channel closed"))?
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
    let text = match message {
        Message::Text(text) => text.to_string(),
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        Message::Close(_) => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "workspace channel closed",
            ))
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace channel returned non-data message",
            ))
        }
    };
    let response: Value = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if response.get("ok").and_then(Value::as_bool) == Some(false) {
        let message = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("workspace channel request failed");
        return Err(io::Error::other(message.to_string()));
    }
    if let Some(result) = response.get("result") {
        return Ok(result.clone());
    }
    Ok(response)
}

fn is_transient_workspace_channel_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

#[derive(Clone)]
pub struct SandboxChannelWorkspaceBackend {
    timeout: Duration,
    endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
}

impl Default for SandboxChannelWorkspaceBackend {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            endpoint_resolver: Arc::new(SandboxBindingEndpointResolver),
        }
    }
}

impl SandboxChannelWorkspaceBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_endpoint_resolver(
        mut self,
        endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
    ) -> Self {
        self.endpoint_resolver = endpoint_resolver;
        self
    }

    async fn channel_backend(
        &self,
        ctx: &ToolContext,
    ) -> io::Result<JsonWorkspaceChannelBackend<WebSocketWorkspaceChannelTransport>> {
        let endpoint = self.endpoint_resolver.endpoint(ctx).await?;
        if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported workspace channel endpoint: {endpoint}"),
            ));
        }
        Ok(JsonWorkspaceChannelBackend::new(
            WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(self.timeout),
            ctx.workspace_root.clone(),
        ))
    }
}

#[derive(Clone)]
pub struct SandboxChannelCommandBackend {
    timeout: Duration,
    endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
}

impl Default for SandboxChannelCommandBackend {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            endpoint_resolver: Arc::new(SandboxBindingEndpointResolver),
        }
    }
}

impl SandboxChannelCommandBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_endpoint_resolver(
        mut self,
        endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
    ) -> Self {
        self.endpoint_resolver = endpoint_resolver;
        self
    }

    async fn channel_backend(
        &self,
        ctx: &ToolContext,
    ) -> io::Result<JsonWorkspaceChannelCommandBackend<WebSocketWorkspaceChannelTransport>> {
        let endpoint = self.endpoint_resolver.endpoint(ctx).await?;
        if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported workspace channel endpoint: {endpoint}"),
            ));
        }
        Ok(JsonWorkspaceChannelCommandBackend::new(
            WebSocketWorkspaceChannelTransport::new(endpoint).with_timeout(self.timeout),
            ctx.workspace_root.clone(),
        ))
    }
}

#[async_trait]
impl SandboxCommandBackend for SandboxChannelCommandBackend {
    async fn run(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput> {
        self.channel_backend(ctx)
            .await?
            .run(ctx, argv, cwd, timeout_ms)
            .await
    }
}

#[async_trait]
impl WorkspaceBackend for SandboxChannelWorkspaceBackend {
    async fn read_to_string(&self, ctx: &ToolContext, path: &Path) -> io::Result<String> {
        self.channel_backend(ctx)
            .await?
            .read_to_string(ctx, path)
            .await
    }

    async fn write_string(&self, ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        self.channel_backend(ctx)
            .await?
            .write_string(ctx, path, text)
            .await
    }

    async fn rename(&self, ctx: &ToolContext, from: &Path, to: &Path) -> io::Result<()> {
        self.channel_backend(ctx).await?.rename(ctx, from, to).await
    }

    async fn list_dir(&self, ctx: &ToolContext, path: &Path) -> io::Result<Vec<WorkspaceEntry>> {
        self.channel_backend(ctx).await?.list_dir(ctx, path).await
    }

    async fn path_kind(&self, ctx: &ToolContext, path: &Path) -> io::Result<WorkspacePathKind> {
        self.channel_backend(ctx).await?.path_kind(ctx, path).await
    }

    async fn remove_file(&self, ctx: &ToolContext, path: &Path) -> io::Result<()> {
        self.channel_backend(ctx)
            .await?
            .remove_file(ctx, path)
            .await
    }

    async fn remove_dir_all(&self, ctx: &ToolContext, path: &Path) -> io::Result<()> {
        self.channel_backend(ctx)
            .await?
            .remove_dir_all(ctx, path)
            .await
    }

    async fn copy_dir_all(
        &self,
        ctx: &ToolContext,
        from: &Path,
        to: &Path,
        skip_dir_names: &[String],
    ) -> io::Result<()> {
        self.channel_backend(ctx)
            .await?
            .copy_dir_all(ctx, from, to, skip_dir_names)
            .await
    }
}

#[derive(Clone)]
pub struct JsonWorkspaceChannelCommandBackend<T> {
    transport: Arc<T>,
    workspace_root: PathBuf,
}

impl<T> JsonWorkspaceChannelCommandBackend<T>
where
    T: WorkspaceChannelTransport + 'static,
{
    pub fn new(transport: T, workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            transport: Arc::new(transport),
            workspace_root: fs::canonicalize(&workspace_root).unwrap_or(workspace_root),
        }
    }

    fn workspace_path(&self, path: &Path) -> io::Result<String> {
        workspace_channel_path(path, &self.workspace_root)
    }
}

#[async_trait]
impl<T> SandboxCommandBackend for JsonWorkspaceChannelCommandBackend<T>
where
    T: WorkspaceChannelTransport + 'static,
{
    async fn run(
        &self,
        _ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput> {
        let value = self
            .transport
            .request(WorkspaceChannelRequest {
                op: "process.exec",
                path: self.workspace_path(cwd)?,
                payload: json!({
                    "argv": argv,
                    "timeoutMs": timeout_ms,
                }),
            })
            .await?;
        Ok(SandboxCommandOutput {
            status: value
                .get("status")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok()),
            success: value
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            stdout: value
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stderr: value
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

#[derive(Clone)]
pub struct JsonWorkspaceChannelBackend<T> {
    transport: Arc<T>,
    workspace_root: PathBuf,
}

impl<T> JsonWorkspaceChannelBackend<T>
where
    T: WorkspaceChannelTransport + 'static,
{
    pub fn new(transport: T, workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            transport: Arc::new(transport),
            workspace_root: fs::canonicalize(&workspace_root).unwrap_or(workspace_root),
        }
    }

    fn workspace_path(&self, path: &Path) -> io::Result<String> {
        workspace_channel_path(path, &self.workspace_root)
    }

    async fn request(&self, op: &'static str, path: &Path, payload: Value) -> io::Result<Value> {
        self.transport
            .request(WorkspaceChannelRequest {
                op,
                path: self.workspace_path(path)?,
                payload,
            })
            .await
    }
}

#[async_trait]
impl<T> WorkspaceBackend for JsonWorkspaceChannelBackend<T>
where
    T: WorkspaceChannelTransport + 'static,
{
    async fn read_to_string(&self, _ctx: &ToolContext, path: &Path) -> io::Result<String> {
        let value = self.request("fs.read", path, json!({})).await?;
        value
            .get("text")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fs.read missing text"))
    }

    async fn write_string(&self, _ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        self.request("fs.write", path, json!({ "text": text }))
            .await?;
        Ok(())
    }

    async fn list_dir(&self, _ctx: &ToolContext, path: &Path) -> io::Result<Vec<WorkspaceEntry>> {
        let value = self.request("fs.list", path, json!({})).await?;
        let entries = value
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fs.list missing entries"))?;
        entries
            .iter()
            .map(|entry| {
                let name = entry.get("name").and_then(Value::as_str).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "fs.list entry missing name")
                })?;
                let kind = match entry.get("kind").and_then(Value::as_str) {
                    Some("dir") => WorkspaceEntryKind::Dir,
                    Some("file") => WorkspaceEntryKind::File,
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "fs.list entry has invalid kind",
                        ))
                    }
                };
                Ok(WorkspaceEntry {
                    path: path.join(name),
                    name: name.to_string(),
                    kind,
                })
            })
            .collect()
    }

    async fn path_kind(&self, _ctx: &ToolContext, path: &Path) -> io::Result<WorkspacePathKind> {
        let value = self.request("fs.stat", path, json!({})).await?;
        match value.get("kind").and_then(Value::as_str) {
            Some("dir") => Ok(WorkspacePathKind::Dir),
            Some("file") => Ok(WorkspacePathKind::File),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fs.stat missing valid kind",
            )),
        }
    }

    async fn remove_file(&self, _ctx: &ToolContext, path: &Path) -> io::Result<()> {
        self.request("fs.removeFile", path, json!({})).await?;
        Ok(())
    }

    async fn remove_dir_all(&self, _ctx: &ToolContext, path: &Path) -> io::Result<()> {
        self.request("fs.removeDirAll", path, json!({})).await?;
        Ok(())
    }

    async fn copy_dir_all(
        &self,
        _ctx: &ToolContext,
        from: &Path,
        to: &Path,
        skip_dir_names: &[String],
    ) -> io::Result<()> {
        self.request(
            "fs.copyDir",
            from,
            json!({
                "to": self.workspace_path(to)?,
                "skipDirNames": skip_dir_names,
            }),
        )
        .await?;
        Ok(())
    }
}

fn workspace_channel_path(path: &Path, workspace_root: &Path) -> io::Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let path = canonicalize_existing_prefix(&path)?;
    let relative = path
        .strip_prefix(workspace_root)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "path outside workspace"))?;
    if relative.as_os_str().is_empty() {
        return Ok("/workspace".to_string());
    }
    Ok(format!("/workspace/{}", relative.display()))
}

fn canonicalize_existing_prefix(path: &Path) -> io::Result<PathBuf> {
    if let Ok(real) = fs::canonicalize(path) {
        return Ok(real);
    }

    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::<OsString>::new();
    loop {
        let Some(file_name) = ancestor.file_name() else {
            return Ok(normalize_path(path));
        };
        suffix.push(file_name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return Ok(normalize_path(path));
        };
        ancestor = parent.to_path_buf();
        if let Ok(mut real_parent) = fs::canonicalize(&ancestor) {
            for part in suffix.iter().rev() {
                real_parent.push(part);
            }
            return Ok(normalize_path(&real_parent));
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn copy_dir_all_local(from: &Path, to: &Path, skip_dir_names: &[String]) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let source = entry.path();
        let target = to.join(&name);
        if source.is_dir() {
            if skip_dir_names
                .iter()
                .any(|skip| name.to_string_lossy() == skip.as_str())
            {
                continue;
            }
            copy_dir_all_local(&source, &target, skip_dir_names)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct LocalWorkspaceBackend;

#[async_trait]
impl WorkspaceBackend for LocalWorkspaceBackend {
    async fn read_to_string(&self, _ctx: &ToolContext, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    async fn write_string(&self, _ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)
    }

    async fn rename(&self, _ctx: &ToolContext, from: &Path, to: &Path) -> io::Result<()> {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, to)
    }

    async fn list_dir(&self, _ctx: &ToolContext, path: &Path) -> io::Result<Vec<WorkspaceEntry>> {
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

    async fn path_kind(&self, _ctx: &ToolContext, path: &Path) -> io::Result<WorkspacePathKind> {
        let metadata = fs::metadata(path)?;
        Ok(if metadata.is_dir() {
            WorkspacePathKind::Dir
        } else {
            WorkspacePathKind::File
        })
    }

    async fn remove_file(&self, _ctx: &ToolContext, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    async fn remove_dir_all(&self, _ctx: &ToolContext, path: &Path) -> io::Result<()> {
        fs::remove_dir_all(path)
    }

    async fn copy_dir_all(
        &self,
        _ctx: &ToolContext,
        from: &Path,
        to: &Path,
        skip_dir_names: &[String],
    ) -> io::Result<()> {
        copy_dir_all_local(from, to, skip_dir_names)
    }
}

struct FsReadTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsReadTool {
    fn name(&self) -> &'static str {
        "fs.read"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        let text = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| {
                let workspace_path = display_workspace_path(&path, &ctx);
                ToolError::RecoverableWithMetadata {
                    message: format!("failed to read {workspace_path}: {error}"),
                    error_kind: "fs.read_failed".to_string(),
                    metadata: json!({
                        "path": workspace_path,
                        "suggestedAction": "If the path is a directory, call fs.list. Otherwise verify the file exists and retry fs.read with a workspace-relative file path."
                    }),
                }
            })?;
        record_read_path(&ctx, &path, &text)?;
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "text": text }),
        ))
    }
}

struct FsListTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsListTool {
    fn name(&self) -> &'static str {
        "fs.list"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        let entries = self
            .workspace
            .list_dir(&ctx, &path)
            .await
            .map_err(|error| {
                let display_path = display_workspace_path(&path, &ctx);
                ToolError::typed_recoverable(
                    format!("failed to list {display_path}: {error}"),
                    "fs.list_failed",
                    json!({
                        "path": display_path,
                        "suggestedAction": "Verify the directory exists, or call fs.read if the path is a file."
                    }),
                )
            })?;
        let entries = entries
            .into_iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "kind": match entry.kind {
                        WorkspaceEntryKind::Dir => "dir",
                        WorkspaceEntryKind::File => "file",
                    },
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "entries": entries }),
        ))
    }
}

struct FsSearchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsSearchTool {
    fn name(&self) -> &'static str {
        "fs.search"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "query": string_schema("Text query")
            }),
            &["path", "query"],
        )
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "query", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        let query = required_str(&input, "query")?;
        let mut matches = Vec::new();
        collect_search_matches(&*self.workspace, &path, &ctx, query, &mut matches).await?;
        Ok(ToolResult::ok(json!({ "matches": matches })))
    }
}

struct FsWriteTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsWriteTool {
    fn name(&self) -> &'static str {
        "fs.write"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "text": string_schema("File contents. Max 48000 chars and max 96000 serialized argument bytes. For larger files use fs.write_chunk then fs.commit_chunks.")
            }),
            &["path", "text"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "text", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        validate_fumadocs_app_router_write_path(&input, ctx, self.name())?;
        validate_write_payload_budget(&input, "fs.write")?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_write_path(&input, &ctx)?;
        let text = required_str(&input, "text")?;
        self.workspace
            .write_string(&ctx, &path, text)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!("failed to write {}: {error}", path.display()))
            })?;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "bytes": text.len(),
        })))
    }
}

struct FsWriteChunkTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsWriteChunkTool {
    fn name(&self) -> &'static str {
        "fs.write_chunk"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Final workspace path"),
                "sessionId": string_schema("Chunked write session id"),
                "index": { "type": "integer", "minimum": 0, "description": "Zero-based chunk index" },
                "total": { "type": "integer", "minimum": 1, "maximum": MAX_CHUNKS_PER_WRITE, "description": "Total chunk count" },
                "text": string_schema("Chunk contents. Max 24000 chars and max 48000 serialized argument bytes.")
            }),
            &["path", "sessionId", "index", "total", "text"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "sessionId", self.name())?;
        require_string(&input, "text", self.name())?;
        validate_fumadocs_app_router_write_path(&input, ctx, self.name())?;
        let index = required_u64_validation(&input, "index", self.name())?;
        let total = required_u64_validation(&input, "total", self.name())?;
        validate_chunk_bounds(index, total)?;
        validate_chunk_payload_budget(&input)?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let final_path = checked_write_path(&input, &ctx)?;
        let session_id = safe_session_id(required_str(&input, "sessionId")?)?;
        let index = required_u64(&input, "index")?;
        let total = required_u64(&input, "total")?;
        validate_chunk_bounds_tool(index, total)?;
        let text = required_str(&input, "text")?;
        cleanup_expired_staged_writes(&*self.workspace, &ctx).await?;
        update_chunk_manifest(
            &*self.workspace,
            &ctx,
            &session_id,
            &final_path,
            index,
            total,
        )
        .await?;
        let chunk_path = staged_chunk_path(&ctx, &session_id, index);
        self.workspace
            .write_string(&ctx, &chunk_path, text)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!(
                    "failed to stage chunk {index} for {}: {error}",
                    final_path.display()
                ))
            })?;
        let display_path = display_workspace_path(&final_path, &ctx);
        ctx.store
            .append_event(AgentEvent::ChunkReceived {
                run_id: ctx.run.id.clone(),
                path: display_path.clone(),
                session_id: session_id.clone(),
                index,
                total,
                bytes: text.len(),
                chars: text.chars().count(),
                timestamp: Utc::now(),
            })
            .await;
        ctx.store
            .append_event(AgentEvent::MetricRecorded {
                run_id: ctx.run.id.clone(),
                name: "tool_chunk_write_started".to_string(),
                value: 1,
                metadata: Some(json!({
                    "tool": self.name(),
                    "path": display_path,
                    "sessionId": session_id.clone(),
                    "index": index,
                    "total": total,
                    "bytes": text.len(),
                    "chars": text.chars().count(),
                })),
                timestamp: Utc::now(),
            })
            .await;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&final_path, &ctx),
            "sessionId": session_id,
            "index": index,
            "total": total,
            "chunkPath": display_workspace_path(&chunk_path, &ctx),
            "chars": text.chars().count(),
        })))
    }
}

struct FsCommitChunksTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsCommitChunksTool {
    fn name(&self) -> &'static str {
        "fs.commit_chunks"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Final workspace path"),
                "sessionId": string_schema("Chunked write session id"),
                "total": { "type": "integer", "minimum": 1, "maximum": MAX_CHUNKS_PER_WRITE, "description": "Total chunk count" },
                "mode": string_schema("Commit mode: create, overwrite, or append. Defaults to overwrite."),
                "sha256": string_schema("Optional expected final sha256")
            }),
            &["path", "sessionId", "total"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "sessionId", self.name())?;
        validate_fumadocs_app_router_write_path(&input, ctx, self.name())?;
        if input.get("sha256").is_some() {
            require_string(&input, "sha256", self.name())?;
        }
        if let Some(mode) = input.get("mode") {
            let Some(mode) = mode.as_str() else {
                return Err(ValidationError::with_kind(
                    "fs.commit_chunks requires string mode",
                    "tool.input_schema_invalid",
                ));
            };
            if !matches!(mode, "create" | "overwrite" | "append") {
                return Err(ValidationError::with_kind(
                    "fs.commit_chunks mode must be create, overwrite, or append",
                    "tool.input_schema_invalid",
                ));
            }
        }
        let total = required_u64_validation(&input, "total", self.name())?;
        validate_chunk_bounds(0, total)?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let final_path = checked_write_path(&input, &ctx)?;
        let session_id = safe_session_id(required_str(&input, "sessionId")?)?;
        let total = required_u64(&input, "total")?;
        validate_chunk_bounds_tool(0, total)?;
        let mode = input
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("overwrite");
        let manifest = read_chunk_manifest(&*self.workspace, &ctx, &session_id)
            .await
            .ok_or_else(|| {
                ToolError::Recoverable(format!(
                    "missing chunk session manifest for session {session_id}"
                ))
            })?;
        validate_chunk_manifest_for_commit(&manifest, &ctx, &final_path, total)?;
        let mut content = String::new();
        for index in 0..total {
            let chunk_path = staged_chunk_path(&ctx, &session_id, index);
            let chunk = self
                .workspace
                .read_to_string(&ctx, &chunk_path)
                .await
                .map_err(|error| {
                    ToolError::Recoverable(format!(
                        "missing or unreadable chunk {index}/{total} for session {session_id}: {error}"
                    ))
                })?;
            content.push_str(&chunk);
        }
        let existing_content = match mode {
            "create" => match self.workspace.read_to_string(&ctx, &final_path).await {
                Ok(_) => {
                    return Err(ToolError::Recoverable(format!(
                        "fs.commit_chunks mode=create refused to overwrite existing {}",
                        display_workspace_path(&final_path, &ctx)
                    )));
                }
                Err(_) => String::new(),
            },
            "append" => self
                .workspace
                .read_to_string(&ctx, &final_path)
                .await
                .unwrap_or_default(),
            _ => String::new(),
        };
        let final_content = if mode == "append" {
            format!("{existing_content}{content}")
        } else {
            content
        };
        let actual_sha256 = sha256_hex(final_content.as_bytes());
        if let Some(expected) = input.get("sha256").and_then(Value::as_str) {
            if expected != actual_sha256 {
                return Err(ToolError::Recoverable(format!(
                    "chunk commit sha256 mismatch: expected {expected}, got {actual_sha256}"
                )));
            }
        }
        let tmp_path = final_path.with_extension("tmp-anydesign-chunks");
        self.workspace
            .write_string(&ctx, &tmp_path, &final_content)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!("failed to write temp file: {error}"))
            })?;
        self.workspace
            .rename(&ctx, &tmp_path, &final_path)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!(
                    "failed to commit chunks to {}: {error}",
                    final_path.display()
                ))
            })?;
        let _ = self
            .workspace
            .remove_dir_all(&ctx, &staged_session_dir(&ctx, &session_id))
            .await;
        let display_path = display_workspace_path(&final_path, &ctx);
        ctx.store
            .append_event(AgentEvent::ChunkCommitted {
                run_id: ctx.run.id.clone(),
                path: display_path.clone(),
                session_id: session_id.clone(),
                total,
                bytes: final_content.len(),
                chars: final_content.chars().count(),
                sha256: actual_sha256.clone(),
                timestamp: Utc::now(),
            })
            .await;
        ctx.store
            .append_event(AgentEvent::MetricRecorded {
                run_id: ctx.run.id.clone(),
                name: "tool_chunk_write_committed".to_string(),
                value: 1,
                metadata: Some(json!({
                    "tool": self.name(),
                    "path": display_path.clone(),
                    "sessionId": session_id.clone(),
                    "total": total,
                    "mode": mode,
                    "bytes": final_content.len(),
                    "chars": final_content.chars().count(),
                    "sha256": actual_sha256.clone(),
                })),
                timestamp: Utc::now(),
            })
            .await;
        record_chunk_write_health(
            &*self.workspace,
            &ctx,
            json!({
                "status": "committed",
                "path": display_workspace_path(&final_path, &ctx),
                "sessionId": session_id.clone(),
                "total": total,
                "mode": mode,
                "bytes": final_content.len(),
                "chars": final_content.chars().count(),
                "sha256": actual_sha256.clone(),
            }),
        )
        .await?;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&final_path, &ctx),
            "sessionId": session_id,
            "total": total,
            "mode": mode,
            "bytes": final_content.len(),
            "chars": final_content.chars().count(),
            "sha256": actual_sha256,
        })))
    }
}

struct FsPatchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsPatchTool {
    fn name(&self) -> &'static str {
        "fs.patch"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "oldStr": string_schema("Existing exact text"),
                "newStr": string_schema("Replacement text"),
                "replaceAll": { "type": "boolean" }
            }),
            &["path", "oldStr", "newStr"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "oldStr", self.name())?;
        require_string(&input, "newStr", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        if input.get("replaceAll").is_some()
            && !input.get("replaceAll").is_some_and(Value::is_boolean)
        {
            return Err(ValidationError::new("fs.patch replaceAll must be boolean"));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        ensure_not_nested_package_root(&path, &ctx)?;
        let old_str = required_str(&input, "oldStr")?;
        let new_str = required_str(&input, "newStr")?;
        let replace_all = input
            .get("replaceAll")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let read_entry = read_tracking_entry(&ctx, &path);
        if read_entry.is_none() {
            return Err(typed_recoverable(
                "fs.patch requires reading the target file first. Call fs.read on the path, then retry with a small unique oldStr of roughly 2-6 lines; do not paste the whole file.".to_string(),
                "patch.read_required",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "suggestedAction": "Call fs.read on this path before fs.patch."
                }),
            ));
        }
        let content = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let current_hash = sha256_hex(content.as_bytes());
        let read_hash = read_entry
            .as_ref()
            .and_then(|entry| entry.get("contentHash").and_then(Value::as_str));
        if read_hash != Some(current_hash.as_str()) {
            return Err(typed_recoverable(
                "file has been modified since fs.read; read it again before attempting fs.patch"
                    .to_string(),
                "patch.stale_read",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "currentHash": current_hash,
                    "readHash": read_hash,
                    "suggestedAction": "Call fs.read again and patch against current contents."
                }),
            ));
        }
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err(typed_recoverable(
                "oldStr not found in file".to_string(),
                "patch.old_str_missing",
                patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content)),
            ));
        }
        if count > 1 && !replace_all {
            return Err(typed_recoverable(
                "oldStr found multiple times, provide more context or set replaceAll=true to replace every occurrence".to_string(),
                "patch.old_str_ambiguous",
                patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content)),
            ));
        }
        let new_content = if replace_all {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };
        self.workspace
            .write_string(&ctx, &path, &new_content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&ctx, &path, &new_content)?;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "patched": true,
            "replaceAll": replace_all,
            "replacements": if replace_all { count } else { 1 },
        })))
    }
}

struct FsMultiPatchTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsMultiPatchTool {
    fn name(&self) -> &'static str {
        "fs.multi_patch"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "path": string_schema("Workspace path"),
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldStr": string_schema("Existing exact text"),
                            "newStr": string_schema("Replacement text"),
                            "replaceAll": { "type": "boolean" }
                        },
                        "required": ["oldStr", "newStr"]
                    }
                }
            }),
            &["path", "edits"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        validate_workspace_path_input(&input, ctx, self.name())?;
        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError::new("fs.multi_patch requires edits array"))?;
        if edits.is_empty() {
            return Err(ValidationError::new(
                "fs.multi_patch requires at least one edit",
            ));
        }
        for edit in edits {
            require_string(edit, "oldStr", self.name())?;
            require_string(edit, "newStr", self.name())?;
            if edit.get("replaceAll").is_some()
                && !edit.get("replaceAll").is_some_and(Value::is_boolean)
            {
                return Err(ValidationError::new(
                    "fs.multi_patch edit replaceAll must be boolean",
                ));
            }
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        check_existing_write_path_permission(input, ctx, self.name())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_existing_path(&input, &ctx)?;
        ensure_not_nested_package_root(&path, &ctx)?;
        let read_entry = read_tracking_entry(&ctx, &path);
        if read_entry.is_none() {
            return Err(typed_recoverable(
                "fs.multi_patch requires reading the target file first. Call fs.read on the path, then retry with small unique oldStr snippets.".to_string(),
                "patch.read_required",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "suggestedAction": "Call fs.read on this path before fs.multi_patch."
                }),
            ));
        }
        let original_content = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let current_hash = sha256_hex(original_content.as_bytes());
        let read_hash = read_entry
            .as_ref()
            .and_then(|entry| entry.get("contentHash").and_then(Value::as_str));
        if read_hash != Some(current_hash.as_str()) {
            return Err(typed_recoverable(
                "file has been modified since fs.read; read it again before attempting fs.multi_patch"
                    .to_string(),
                "patch.stale_read",
                json!({
                    "path": display_workspace_path(&path, &ctx),
                    "currentHash": current_hash,
                    "readHash": read_hash,
                    "suggestedAction": "Call fs.read again and patch against current contents."
                }),
            ));
        }

        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::Recoverable("fs.multi_patch requires edits array".to_string())
            })?;
        let mut content = original_content;
        let mut applied = Vec::new();
        let mut total_replacements = 0usize;
        for (index, edit) in edits.iter().enumerate() {
            let old_str = required_str(edit, "oldStr")?;
            let new_str = required_str(edit, "newStr")?;
            let replace_all = edit
                .get("replaceAll")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let count = content.matches(old_str).count();
            if count == 0 {
                let mut metadata =
                    patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content));
                metadata["editIndex"] = json!(index);
                return Err(typed_recoverable(
                    format!("fs.multi_patch edit {index} oldStr not found in file"),
                    "patch.old_str_missing",
                    metadata,
                ));
            }
            if count > 1 && !replace_all {
                let mut metadata =
                    patch_recovery_metadata(&ctx, &path, old_str, count, Some(&content));
                metadata["editIndex"] = json!(index);
                return Err(typed_recoverable(
                    format!("fs.multi_patch edit {index} oldStr found multiple times, provide more context or set replaceAll=true"),
                    "patch.old_str_ambiguous",
                    metadata,
                ));
            }
            content = if replace_all {
                content.replace(old_str, new_str)
            } else {
                content.replacen(old_str, new_str, 1)
            };
            let replacements = if replace_all { count } else { 1 };
            total_replacements += replacements;
            applied.push(json!({
                "index": index,
                "replaceAll": replace_all,
                "replacements": replacements,
            }));
        }
        self.workspace
            .write_string(&ctx, &path, &content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&ctx, &path, &content)?;
        Ok(ToolResult::ok(json!({
            "path": display_workspace_path(&path, &ctx),
            "patched": true,
            "edits": applied,
            "replacements": total_replacements,
        })))
    }
}

struct StyleUpdateTokensTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for StyleUpdateTokensTool {
    fn name(&self) -> &'static str {
        "style.update_tokens"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "tokens": {
                    "type": "object",
                    "description": "Patch map of style contract token names to CSS values, for example color.primary -> #f37a0a.",
                    "additionalProperties": { "type": "string" }
                }
            }),
            &["tokens"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let tokens = input
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| style_validation_error("style.update_tokens requires tokens object"))?;
        if tokens.is_empty() {
            return Err(style_validation_error(
                "style.update_tokens requires at least one token",
            ));
        }
        for (name, value) in tokens {
            if name.trim().is_empty() {
                return Err(style_validation_error(
                    "style.update_tokens token names must be non-empty",
                ));
            }
            let Some(value) = value.as_str() else {
                return Err(style_validation_error(
                    "style.update_tokens token values must be strings",
                ));
            };
            validate_style_token_value(value).map_err(|message| {
                style_validation_error(format!("style.update_tokens {message}"))
            })?;
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "style token update allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let contract = read_workspace_json(&*self.workspace, &ctx, "state/style-contract.json")
            .await
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens requires state/style-contract.json; initialize the project first",
                    "style.contract_missing",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "suggestedAction": "Run project.init or project.inspect before style.update_tokens so the runtime style contract exists."
                    }),
                )
            })?;
        let token_file = contract
            .get("tokenFile")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens style contract is missing tokenFile",
                    "style.contract_invalid",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "missingField": "tokenFile",
                        "suggestedAction": "Regenerate the project style contract with project.init or repair state/style-contract.json."
                    }),
                )
            })?;
        let token_path = check_existing_path(
            &resolve_path(token_file, &ctx.workspace_root),
            &ctx.workspace_root,
        )
        .map_err(|error| {
            style_typed_recoverable(
                format!("style.update_tokens tokenFile is not readable: {error:?}"),
                "style.token_file_unavailable",
                json!({
                    "tokenFile": token_file,
                    "suggestedAction": "Ensure the contract tokenFile points to an existing workspace token CSS file."
                }),
            )
        })?;
        let contract_tokens = contract
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens style contract is missing tokens map",
                    "style.contract_invalid",
                    json!({
                        "contractPath": "/workspace/state/style-contract.json",
                        "missingField": "tokens",
                        "suggestedAction": "Regenerate the project style contract with project.init or repair state/style-contract.json."
                    }),
                )
            })?;
        let requested = input
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens requires tokens object",
                    "style.input_invalid",
                    json!({
                        "suggestedAction": "Pass a tokens object such as {\"color.primary\":\"#f37a0a\"}."
                    }),
                )
            })?;

        let mut content = self
            .workspace
            .read_to_string(&ctx, &token_path)
            .await
            .map_err(|error| {
                style_typed_recoverable(
                    format!("style.update_tokens failed to read token file: {error}"),
                    "style.token_file_unavailable",
                    json!({
                        "tokenFile": display_workspace_path(&token_path, &ctx),
                        "suggestedAction": "Ensure the token file exists and is readable before updating style tokens."
                    }),
                )
            })?;
        let mut changes = Vec::new();
        for (token_name, value) in requested {
            let Some(css_variable) = contract_tokens.get(token_name).and_then(Value::as_str) else {
                return Err(style_typed_recoverable(
                    format!(
                        "style.update_tokens unknown token {token_name}; use one of the tokens declared in state/style-contract.json"
                    ),
                    "style.token_unknown",
                    json!({
                        "token": token_name,
                        "contractPath": "/workspace/state/style-contract.json",
                        "availableTokens": contract_tokens.keys().cloned().collect::<Vec<_>>(),
                        "suggestedAction": "Call project.inspect and update only tokens declared in state/style-contract.json."
                    }),
                ));
            };
            let new_value = value.as_str().ok_or_else(|| {
                style_typed_recoverable(
                    "style.update_tokens token values must be strings",
                    "style.input_invalid",
                    json!({
                        "token": token_name,
                        "suggestedAction": "Pass CSS token values as strings."
                    }),
                )
            })?;
            validate_style_token_value(new_value).map_err(|message| {
                style_typed_recoverable(
                    format!("style.update_tokens {message}"),
                    "style.token_value_invalid",
                    json!({
                        "token": token_name,
                        "value": new_value,
                        "suggestedAction": "Use a simple CSS token value without semicolons, braces, or newlines."
                    }),
                )
            })?;
            let (updated, old_value) =
                replace_css_variable_value(&content, css_variable, new_value, &ctx, &token_path)?;
            content = updated;
            changes.push(json!({
                "token": token_name,
                "cssVariable": css_variable,
                "before": old_value,
                "after": new_value,
            }));
        }

        self.workspace
            .write_string(&ctx, &token_path, &content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        record_read_path(&ctx, &token_path, &content)?;
        Ok(ToolResult::ok(json!({
            "tokenFile": display_workspace_path(&token_path, &ctx),
            "updated": true,
            "changes": changes,
        })))
    }
}

struct FsDeleteTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for FsDeleteTool {
    fn name(&self) -> &'static str {
        "fs.delete"
    }

    fn input_schema(&self) -> Value {
        path_schema()
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        match checked_delete_path(input, ctx) {
            Ok(_) => allow_with_input(input, "workspace delete path allowed"),
            Err(message) => deny(self.name(), message),
        }
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = checked_delete_path(&input, &ctx).map_err(ToolError::PermissionDenied)?;
        match self
            .workspace
            .path_kind(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?
        {
            WorkspacePathKind::Dir => self
                .workspace
                .remove_dir_all(&ctx, &path)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
            WorkspacePathKind::File => self
                .workspace
                .remove_file(&ctx, &path)
                .await
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        }
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "deleted": true }),
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalCommandBackend;

fn local_command_argv(ctx: &ToolContext, argv: &[String]) -> Vec<String> {
    argv.iter().map(|arg| local_command_arg(ctx, arg)).collect()
}

fn local_command_arg(ctx: &ToolContext, arg: &str) -> String {
    if arg == "/workspace" || arg == "workspace" {
        return ctx.workspace_root.to_string_lossy().to_string();
    }
    if let Some(relative) = arg
        .strip_prefix("/workspace/")
        .or_else(|| arg.strip_prefix("workspace/"))
    {
        return ctx
            .workspace_root
            .join(relative)
            .to_string_lossy()
            .to_string();
    }
    arg.to_string()
}

#[async_trait]
impl SandboxCommandBackend for LocalCommandBackend {
    async fn run(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput> {
        let argv = local_command_argv(ctx, argv);
        let mut command = TokioCommand::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stdout_task = take_output_reader(&mut child, true, stdout.clone(), None);
        let stderr_task = take_output_reader(&mut child, false, stderr.clone(), None);
        let started = Instant::now();
        let mut last_len = 0usize;
        let mut last_change = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let prompt_grace = Duration::from_millis(750);

        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= timeout {
                child.kill().await.ok();
                wait_output_reader(stdout_task).await;
                wait_output_reader(stderr_task).await;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shell.run timed out",
                ));
            }

            let current_stdout = stdout.lock().await.clone();
            let current_stderr = stderr.lock().await.clone();
            let current_len = current_stdout.len() + current_stderr.len();
            if current_len != last_len {
                last_len = current_len;
                last_change = Instant::now();
            } else if last_change.elapsed() >= prompt_grace
                && output_tail_looks_interactive(&current_stdout, &current_stderr)
            {
                child.kill().await.ok();
                wait_output_reader(stdout_task).await;
                wait_output_reader(stderr_task).await;
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "interactive prompt detected; rerun non-interactively with --yes/--no or use project.init/package.install plus fs.* edits",
                ));
            }
            time::sleep(Duration::from_millis(100)).await;
        };

        wait_output_reader(stdout_task).await;
        wait_output_reader(stderr_task).await;
        let stdout = stdout.lock().await.clone();
        let stderr = stderr.lock().await.clone();
        Ok(SandboxCommandOutput {
            status: status.code(),
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        })
    }

    async fn run_with_output_events(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
        progress: Option<ProgressSink>,
        tool_name: &str,
    ) -> io::Result<SandboxCommandOutput> {
        let argv = local_command_argv(ctx, argv);
        let mut command = TokioCommand::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stdout_task = take_output_reader(
            &mut child,
            true,
            stdout.clone(),
            progress
                .clone()
                .map(|progress| (progress, tool_name.to_string(), "stdout".to_string())),
        );
        let stderr_task = take_output_reader(
            &mut child,
            false,
            stderr.clone(),
            progress.map(|progress| (progress, tool_name.to_string(), "stderr".to_string())),
        );
        let started = Instant::now();
        let mut last_len = 0usize;
        let mut last_change = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let prompt_grace = Duration::from_millis(750);

        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= timeout {
                child.kill().await.ok();
                wait_output_reader(stdout_task).await;
                wait_output_reader(stderr_task).await;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shell.run timed out",
                ));
            }

            let current_stdout = stdout.lock().await.clone();
            let current_stderr = stderr.lock().await.clone();
            let current_len = current_stdout.len() + current_stderr.len();
            if current_len != last_len {
                last_len = current_len;
                last_change = Instant::now();
            } else if last_change.elapsed() >= prompt_grace
                && output_tail_looks_interactive(&current_stdout, &current_stderr)
            {
                child.kill().await.ok();
                wait_output_reader(stdout_task).await;
                wait_output_reader(stderr_task).await;
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "interactive prompt detected; rerun non-interactively with --yes/--no or use project.init/package.install plus fs.* edits",
                ));
            }
            time::sleep(Duration::from_millis(100)).await;
        };

        wait_output_reader(stdout_task).await;
        wait_output_reader(stderr_task).await;
        let stdout = stdout.lock().await.clone();
        let stderr = stderr.lock().await.clone();
        Ok(SandboxCommandOutput {
            status: status.code(),
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        })
    }
}

fn take_output_reader(
    child: &mut Child,
    stdout_stream: bool,
    buffer: Arc<Mutex<Vec<u8>>>,
    output_events: Option<(ProgressSink, String, String)>,
) -> tokio::task::JoinHandle<()> {
    let reader =
        if stdout_stream {
            child.stdout.take().map(|stream| {
                Box::pin(stream) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>
            })
        } else {
            child.stderr.take().map(|stream| {
                Box::pin(stream) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>
            })
        };
    tokio::spawn(async move {
        let Some(mut reader) = reader else {
            return;
        };
        let mut chunk = [0u8; 1024];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buffer.lock().await.extend_from_slice(&chunk[..n]);
                    if let Some((progress, tool_name, stream)) = &output_events {
                        progress
                            .emit_tool_output(
                                tool_name.clone(),
                                stream.clone(),
                                String::from_utf8_lossy(&chunk[..n]).to_string(),
                            )
                            .await;
                    }
                }
            }
        }
    })
}

async fn wait_output_reader(handle: tokio::task::JoinHandle<()>) {
    handle.await.ok();
}

fn output_tail_looks_interactive(stdout: &[u8], stderr: &[u8]) -> bool {
    let mut combined = Vec::new();
    combined.extend_from_slice(stdout);
    combined.extend_from_slice(stderr);
    let tail_start = combined.len().saturating_sub(2048);
    let tail = String::from_utf8_lossy(&combined[tail_start..]).to_lowercase();
    [
        "continue?",
        "proceed?",
        "yes/no",
        "(y/n)",
        "[y/n]",
        "press enter",
        "would you like",
        "do you want",
        "install dependencies?",
        "need to install",
    ]
    .iter()
    .any(|pattern| tail.contains(pattern))
}

struct ShellRunTool {
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ShellRunTool {
    fn name(&self) -> &'static str {
        "shell.run"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "argv": { "type": "array", "items": { "type": "string" } },
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &["argv"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let argv = input
            .get("argv")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError::new("shell.run requires argv"))?;
        if argv.is_empty() || !argv.iter().all(|item| item.as_str().is_some()) {
            return Err(ValidationError::new(
                "shell.run argv must be a non-empty string array",
            ));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let argv = argv_from_input(input).unwrap_or_default();
        check_command_policy(&argv)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let argv = argv_from_input(&input)?;
        let cwd = match input.get("cwd").and_then(Value::as_str) {
            Some(cwd) => {
                check_existing_path(&resolve_path(cwd, &ctx.workspace_root), &ctx.workspace_root)
                    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?
            }
            None => default_project_dir(&ctx),
        };
        let timeout_ms = input
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(60_000);
        let output = self
            .command
            .run_with_output_events(&ctx, &argv, &cwd, timeout_ms, None, self.name())
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    ToolError::Recoverable("shell.run timed out".to_string())
                } else if error.kind() == io::ErrorKind::Interrupted {
                    ToolError::Recoverable(error.to_string())
                } else {
                    ToolError::Recoverable(format!("shell.run failed to start: {error}"))
                }
            })?;
        if !output.success {
            return Err(ToolError::typed_recoverable(
                format!(
                    "shell.run exited with status {:?}\nstdout:\n{}\nstderr:\n{}",
                    output.status, output.stdout, output.stderr
                ),
                "shell.non_zero_exit",
                json!({
                    "status": output.status,
                    "stdout": truncate_for_metadata(&output.stdout),
                    "stderr": truncate_for_metadata(&output.stderr),
                    "suggestedAction": "Inspect stdout/stderr, fix the command arguments, or use a dedicated runtime tool when available."
                }),
            ));
        }
        Ok(ToolResult::ok(json!({
            "status": output.status,
            "success": output.success,
            "stdout": output.stdout,
            "stderr": output.stderr,
        })))
    }
}

struct ProjectInitTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectInitTool {
    fn name(&self) -> &'static str {
        "project.init"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "template": string_schema("Template key such as astro-website or fumadocs-docs"),
                "path": string_schema("Workspace relative app root")
            }),
            &["template"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        match input.get("template").and_then(Value::as_str) {
            Some("astro-website" | "fumadocs-docs") => Ok(input),
            Some(template) => Err(ValidationError::new(format!(
                "project.init unsupported template: {template}"
            ))),
            None => Err(ValidationError::new("project.init requires template")),
        }
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project initialization allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let template = required_str(&input, "template")?.to_string();
        let app_root_relative = normalize_workspace_relative_path(
            input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("project"),
        )?;
        let app_root = check_create_path(
            &ctx.workspace_root.join(&app_root_relative),
            &ctx.workspace_root,
        )
        .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?;

        cleanup_conflicting_template_files(&*self.workspace, &ctx, &app_root, &template).await?;
        write_project_template_files(&*self.workspace, &ctx, &app_root, &template).await?;
        write_workspace_json(
            &*self.workspace,
            &ctx,
            "state/style-contract.json",
            &runtime_style_contract(&template, &app_root_relative),
        )
        .await?;
        let state = json!({
            "appRoot": app_root_relative,
            "template": template,
            "templateKey": template,
            "templateVersion": format!("{template}@runtime-p2"),
            "framework": if template == "fumadocs-docs" { "fumadocs" } else { "astro" },
            "packageManager": "npm",
            "lockfile": "package-lock.json",
            "registry": ctx.npm_registry,
            "initializedAt": Utc::now().to_rfc3339(),
        });
        write_workspace_json(&*self.workspace, &ctx, "state/project.json", &state).await?;
        Ok(ToolResult::ok(json!({
            "appRoot": display_workspace_path(&app_root, &ctx),
            "statePath": "/workspace/state/project.json",
            "template": template,
            "packageManager": "npm",
            "styleContractPath": "/workspace/state/style-contract.json",
        })))
    }
}

struct ProjectWritePageTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectWritePageTool {
    fn name(&self) -> &'static str {
        "project.write_page"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "route": string_schema("Page route such as /, /pricing, or /docs/getting-started"),
                "title": string_schema("Page title"),
                "styleProfile": string_schema("Visual style profile, for example saas"),
                "sections": {
                    "type": "array",
                    "description": "Structured page sections. Each section may include kind, heading, body, and visual.",
                    "items": {
                        "type": "object",
                        "additionalProperties": true,
                        "properties": {
                            "kind": { "type": "string" },
                            "heading": { "type": "string" },
                            "body": { "type": "string" },
                            "visual": { "type": "string" }
                        }
                    }
                }
            }),
            &["route", "title", "sections"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "route", self.name())?;
        require_string(&input, "title", self.name())?;
        let Some(sections) = input.get("sections").and_then(Value::as_array) else {
            return Err(ValidationError::with_kind(
                "project.write_page requires sections array",
                "tool.input_schema_invalid",
            ));
        };
        if sections.is_empty() {
            return Err(ValidationError::with_kind(
                "project.write_page requires at least one section",
                "tool.input_schema_invalid",
            ));
        }
        let route = input
            .get("route")
            .and_then(Value::as_str)
            .expect("route was validated as string");
        project_page_relative_path(route)
            .map_err(|message| ValidationError::with_kind(message, "tool.input_schema_invalid"))?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        let Some(route) = input.get("route").and_then(Value::as_str) else {
            return deny(self.name(), "project.write_page requires route");
        };
        let Ok(relative_page_path) = project_page_relative_path(route) else {
            return deny(self.name(), "project.write_page route is invalid");
        };
        let app_root = project_app_root_relative(ctx);
        let path = app_root.join("src/pages").join(relative_page_path);
        let synthetic = json!({ "path": path.to_string_lossy().to_string() });
        match check_write_path_permission(&synthetic, ctx, self.name()) {
            PermissionResult::Allow { reason, .. } => PermissionResult::Allow {
                updated_input: input.clone(),
                reason,
            },
            other => other,
        }
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let route = required_str(&input, "route")?;
        let title = required_str(&input, "title")?;
        let style_profile = input
            .get("styleProfile")
            .and_then(Value::as_str)
            .unwrap_or("saas");
        let sections = input
            .get("sections")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::Recoverable("project.write_page missing sections".to_string())
            })?;
        let relative_page_path =
            project_page_relative_path(route).map_err(ToolError::Recoverable)?;
        let app_root = default_project_dir(&ctx);
        let raw_page_path = app_root.join("src/pages").join(&relative_page_path);
        let page_path = if raw_page_path.exists() {
            check_existing_path(&raw_page_path, &ctx.workspace_root)
        } else {
            check_create_path(&raw_page_path, &ctx.workspace_root)
        }
        .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))?;
        ensure_not_nested_package_root(&page_path, &ctx)?;
        let page = render_project_page(route, title, style_profile, sections, &relative_page_path);
        self.workspace
            .write_string(&ctx, &page_path, &page)
            .await
            .map_err(|error| {
                ToolError::Recoverable(format!("failed to write {}: {error}", page_path.display()))
            })?;
        Ok(ToolResult::ok(json!({
            "route": route,
            "path": display_workspace_path(&page_path, &ctx),
            "bytes": page.len(),
            "sections": sections.len(),
            "styleProfile": style_profile,
        })))
    }
}

struct ProjectInspectTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for ProjectInspectTool {
    fn name(&self) -> &'static str {
        "project.inspect"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project inspection allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let project = read_workspace_json(&*self.workspace, &ctx, "state/project.json").await;
        let app_root_relative = project
            .as_ref()
            .and_then(|state| state.get("appRoot").and_then(Value::as_str))
            .map(normalize_workspace_relative_path)
            .transpose()?
            .unwrap_or_else(|| PathBuf::from("project"));
        let app_root = ctx.workspace_root.join(&app_root_relative);
        let package_manager = package_manager_from_project_state_or_lockfiles(&ctx, &app_root);
        let package_json = self
            .workspace
            .read_to_string(&ctx, &app_root.join("package.json"))
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let style_contract =
            read_workspace_json(&*self.workspace, &ctx, "state/style-contract.json").await;
        let latest_build =
            read_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json").await;
        let dependency_state =
            read_workspace_json(&*self.workspace, &ctx, "state/dependency-state.json").await;
        let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json").await;
        let browser = read_workspace_json(&*self.workspace, &ctx, "state/browser.json").await;
        let key_source_files = project_key_source_files(&ctx, &app_root_relative, project.as_ref());

        Ok(ToolResult::ok(json!({
            "appRoot": format!("/workspace/{}", app_root_relative.to_string_lossy().replace('\\', "/")),
            "appRootRelative": app_root_relative.to_string_lossy().replace('\\', "/"),
            "packageManager": package_manager,
            "framework": project.as_ref().and_then(|state| state.get("framework")).cloned().unwrap_or(Value::Null),
            "templateKey": project.as_ref().and_then(|state| state.get("templateKey")).cloned().unwrap_or(Value::Null),
            "project": project,
            "package": package_json,
            "keySourceFiles": key_source_files,
            "styleContractPath": if style_contract.is_some() { json!("/workspace/state/style-contract.json") } else { Value::Null },
            "styleContract": style_contract,
            "latestBuild": latest_build,
            "dependencyState": dependency_state,
            "preview": preview,
            "browser": browser,
        })))
    }
}

struct ProjectBuildTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ProjectBuildTool {
    fn name(&self) -> &'static str {
        "project.build"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "project build allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|cwd| {
                check_existing_path(&resolve_path(cwd, &ctx.workspace_root), &ctx.workspace_root)
                    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
            })
            .transpose()?
            .unwrap_or_else(|| default_project_dir(&ctx));
        ensure_project_package_json(&*self.workspace, &ctx, &cwd).await?;
        validate_project_source_contract(&*self.workspace, &ctx, &cwd).await?;
        let timeout_ms = input
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(180_000);
        let package_manager = package_manager_from_input_or_project(&json!({}), &ctx, &cwd)?;
        let dependency_restore = maybe_restore_project_dependencies(
            &*self.workspace,
            &*self.command,
            &ctx,
            &progress,
            &cwd,
            &package_manager,
        )
        .await?;
        let argv = project_build_argv(&package_manager);
        let started_at = Utc::now();
        let output = self.command.run(&ctx, &argv, &cwd, timeout_ms).await;
        let finished_at = Utc::now();
        let (status, output, error_message) = match output {
            Ok(output) => {
                let status = if output.success { "success" } else { "failed" };
                (status, Some(output), None)
            }
            Err(error) => {
                let status = if error.kind() == io::ErrorKind::TimedOut {
                    "timeout"
                } else {
                    "failed"
                };
                (status, None, Some(error.to_string()))
            }
        };
        let log_name = format!("build-{}.log", finished_at.timestamp_millis());
        let log_path = format!("outputs/build/{log_name}");
        let log_text = match &output {
            Some(output) => format!(
                "$ {}\n\ncwd: {}\nstatus: {:?}\nstartedAt: {}\nfinishedAt: {}\n\nstdout:\n{}\n\nstderr:\n{}\n",
                argv.join(" "),
                display_workspace_path(&cwd, &ctx),
                output.status,
                started_at.to_rfc3339(),
                finished_at.to_rfc3339(),
                output.stdout,
                output.stderr
            ),
            None => format!(
                "$ {}\n\ncwd: {}\nstatus: {status}\nstartedAt: {}\nfinishedAt: {}\n\nerror:\n{}\n",
                argv.join(" "),
                display_workspace_path(&cwd, &ctx),
                started_at.to_rfc3339(),
                finished_at.to_rfc3339(),
                error_message.as_deref().unwrap_or("build command failed to start")
            ),
        };
        self.workspace
            .write_string(&ctx, &ctx.workspace_root.join(&log_path), &log_text)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        self.workspace
            .write_string(
                &ctx,
                &ctx.workspace_root.join("outputs/build/build.log"),
                &log_text,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let build_id = format!("build-{}", finished_at.timestamp_millis());
        let source_snapshot_path = format!("outputs/build/source-snapshots/{build_id}");
        let source_snapshot_uri = format!("file:///workspace/{source_snapshot_path}");
        snapshot_project_source(&*self.workspace, &ctx, &cwd, &source_snapshot_path).await?;
        let source_snapshot_text = format!(
            "buildId: {build_id}\ncwd: {}\nstatus: {status}\nfinishedAt: {}\nlogPath: /workspace/{log_path}\n",
            display_workspace_path(&cwd, &ctx),
            finished_at.to_rfc3339(),
        );
        self.workspace
            .write_string(
                &ctx,
                &ctx.workspace_root.join("outputs/build/source-snapshot.txt"),
                &source_snapshot_text,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;

        let static_output_dir = if status == "success" {
            detect_static_preview_output_dir(&ctx, &cwd)
        } else {
            None
        };
        let static_output_path = static_output_dir
            .as_ref()
            .map(|path| display_workspace_path(path, &ctx));
        let static_output_name = static_output_dir
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string);

        let latest = json!({
            "buildId": build_id,
            "status": status,
            "success": status == "success",
            "cwd": display_workspace_path(&cwd, &ctx),
            "argv": argv,
            "packageManager": package_manager,
            "dependencyRestoreAttempted": dependency_restore.attempted,
            "dependencyRestoreSucceeded": dependency_restore.succeeded,
            "dependencyRestoreReason": dependency_restore.reason,
            "dependencyRestoreLogPath": dependency_restore.log_path,
            "startedAt": started_at.to_rfc3339(),
            "finishedAt": finished_at.to_rfc3339(),
            "exitCode": output.as_ref().and_then(|output| output.status),
            "logPath": format!("/workspace/{log_path}"),
            "sourceSnapshotUri": source_snapshot_uri,
            "staticOutputPath": static_output_path,
            "staticOutputName": static_output_name,
            "error": error_message,
        });
        write_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json", &latest).await?;
        if status != "success" {
            let classification =
                classify_project_build_failure(status, output.as_ref(), error_message.as_deref());
            return Err(ToolError::typed_recoverable(
                format!("project.build {status}; log: /workspace/{log_path}"),
                classification.error_kind,
                json!({
                    "logPath": format!("/workspace/{log_path}"),
                    "status": status,
                    "exitCode": output.as_ref().and_then(|output| output.status),
                    "stderr": output.as_ref().map(|output| truncate_for_metadata(&output.stderr)),
                    "error": error_message,
                    "suggestedAction": classification.suggested_action,
                }),
            ));
        }
        Ok(ToolResult::ok(latest))
    }
}

struct BuildFailureClassification {
    error_kind: &'static str,
    suggested_action: &'static str,
}

fn classify_project_build_failure(
    status: &str,
    output: Option<&SandboxCommandOutput>,
    error_message: Option<&str>,
) -> BuildFailureClassification {
    let stderr = output.map(|output| output.stderr.as_str()).unwrap_or("");
    let lowered = format!("{} {}", stderr, error_message.unwrap_or("")).to_lowercase();
    if output.and_then(|output| output.status) == Some(127)
        || lowered.contains("command not found")
        || lowered.contains("module not found")
        || lowered.contains("cannot find module")
    {
        return BuildFailureClassification {
            error_kind: "build.missing_dependency",
            suggested_action: "Run project.ensure_dependencies with mode=restore, verify dependency installation completed, then rerun project.build or preview.publish.",
        };
    }
    if status == "timeout" {
        return BuildFailureClassification {
            error_kind: "build.timeout",
            suggested_action: "Increase timeoutMs if the build is legitimately long, or inspect diagnostics.build_log before retrying project.build.",
        };
    }
    BuildFailureClassification {
        error_kind: "build.failed",
        suggested_action:
            "Open diagnostics.build_log, fix the source or dependency error, then rerun project.build or preview.publish.",
    }
}

fn truncate_for_metadata(text: &str) -> String {
    const LIMIT: usize = 2048;
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(LIMIT).collect::<String>())
    }
}

struct ProjectEnsureDependenciesTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for ProjectEnsureDependenciesTool {
    fn name(&self) -> &'static str {
        "project.ensure_dependencies"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "mode": string_schema("Install mode: restore or add"),
                "packages": { "type": "array", "items": { "type": "string" } },
                "cwd": string_schema("Workspace cwd"),
                "packageManager": string_schema("Package manager: npm or pnpm"),
                "registry": string_schema("Internal registry URL"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        validate_package_install_like_input(&input, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        package_install_permission(self.name(), input, ctx)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let result = run_package_install(
            self.name(),
            &*self.workspace,
            &*self.command,
            input,
            ctx,
            progress,
        )
        .await?;
        Ok(ToolResult::ok(json!({
            "ensured": true,
            "dependencyState": result,
        })))
    }
}

struct PackageInstallTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PackageInstallTool {
    fn name(&self) -> &'static str {
        "package.install"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "packages": { "type": "array", "items": { "type": "string" } },
                "mode": string_schema("Install mode: restore or add"),
                "packageManager": string_schema("Package manager: npm or pnpm"),
                "registry": string_schema("Internal registry URL"),
                "cwd": string_schema("Workspace cwd"),
                "timeoutMs": { "type": "integer", "minimum": 1 }
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        validate_package_install_like_input(&input, self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        package_install_permission(self.name(), input, ctx)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let result = run_package_install(
            self.name(),
            &*self.workspace,
            &*self.command,
            input,
            ctx,
            progress,
        )
        .await?;
        Ok(ToolResult::ok(result))
    }
}

struct PreviewRebuildingTool;

#[async_trait]
impl Tool for PreviewRebuildingTool {
    fn name(&self) -> &'static str {
        "preview.rebuilding"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({ "previousVersionId": string_schema("Previous promoted version id") }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview rebuild event allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let previous_version_id = input
            .get("previousVersionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        ctx.store
            .append_event(AgentEvent::PreviewRebuilding {
                run_id: ctx.run.id.clone(),
                previous_version_id,
                timestamp: Utc::now(),
            })
            .await;
        Ok(ToolResult::ok(json!({ "rebuilding": true })))
    }
}

struct PreviewReportCandidateTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for PreviewReportCandidateTool {
    fn name(&self) -> &'static str {
        "preview.report_candidate"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "url": string_schema("Candidate preview URL"),
                "screenshotId": string_schema("Screenshot artifact id"),
                "sourceSnapshotUri": string_schema("Source snapshot URI")
            }),
            &["url"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "url", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
        if !is_internal_preview_url(url) {
            return PermissionResult::Deny {
                message: "preview.report_candidate public preview URL is not allowed".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "public preview candidate URL denied".to_string(),
                },
            };
        }
        allow_with_input(input, "preview candidate report allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let url = required_str(&input, "url")?.to_string();
        verify_preview_accessible(&url).await?;
        let screenshot_id = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.report_candidate requires screenshotId from browser.screenshot before creating a candidate".to_string(),
                    "preview.screenshot_missing",
                    json!({
                        "suggestedAction": "Call browser.screenshot and pass screenshotId to preview.report_candidate."
                    }),
                )
            })?;
        let source_snapshot_uri = input
            .get("sourceSnapshotUri")
            .and_then(Value::as_str)
            .map(str::to_string);
        report_preview_candidate(
            &*self.workspace,
            &ctx,
            url,
            screenshot_id,
            source_snapshot_uri.as_deref(),
        )
        .await
        .map(ToolResult::ok)
    }
}

struct PreviewPublishTool {
    workspace: Arc<dyn WorkspaceBackend>,
    command: Arc<dyn SandboxCommandBackend>,
}

#[async_trait]
impl Tool for PreviewPublishTool {
    fn name(&self) -> &'static str {
        "preview.publish"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "cwd": string_schema("Workspace cwd"),
                "buildTimeoutMs": { "type": "integer", "minimum": 1 },
                "url": string_schema("Preview URL"),
                "port": { "type": "integer", "minimum": 1 },
                "command": string_schema("Preview command label"),
                "mode": string_schema("Preview mode: static or framework"),
                "screenshotId": string_schema("Screenshot artifact id")
            }),
            &[],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        if let Some(url) = input.get("url") {
            require_string(&json!({ "url": url.clone() }), "url", self.name())?;
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        if let Some(url) = input.get("url").and_then(Value::as_str) {
            if !is_internal_preview_url(url) {
                return PermissionResult::Deny {
                    message: "preview.publish public preview URL is not allowed".to_string(),
                    reason: PermissionReason::Rule {
                        source: RuleSource::Runtime,
                        rule_content: "public preview publish URL denied".to_string(),
                    },
                };
            }
        }
        allow_with_input(input, "preview publish allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let build_tool = ProjectBuildTool {
            workspace: self.workspace.clone(),
            command: self.command.clone(),
        };
        let mut build_input = json!({});
        if let Some(cwd) = input.get("cwd").cloned() {
            build_input["cwd"] = cwd;
        }
        if let Some(timeout) = input.get("buildTimeoutMs").cloned() {
            build_input["timeoutMs"] = timeout;
        }
        let build = build_tool
            .call(build_input, ctx.clone(), progress.clone())
            .await?
            .content;

        let preview_tool = PreviewStartTool {
            workspace: self.workspace.clone(),
        };
        let mut preview_input = json!({});
        for key in ["url", "port", "command", "mode"] {
            if let Some(value) = input.get(key).cloned() {
                preview_input[key] = value;
            }
        }
        let preview = preview_tool
            .call(preview_input, ctx.clone(), progress.clone())
            .await?
            .content;
        let url = preview
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable(
                    "preview.publish preview.start did not return url".to_string(),
                )
            })?
            .to_string();

        let browser_tool = BrowserOpenTool {
            workspace: self.workspace.clone(),
        };
        browser_tool
            .call(json!({ "url": url.clone() }), ctx.clone(), progress.clone())
            .await?;

        let screenshot_tool = BrowserScreenshotTool {
            workspace: self.workspace.clone(),
        };
        let screenshot_input = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(|screenshot_id| json!({ "screenshotId": screenshot_id }))
            .unwrap_or_else(|| json!({}));
        let screenshot = screenshot_tool
            .call(screenshot_input, ctx.clone(), progress)
            .await?
            .content;
        let screenshot_id = screenshot
            .get("screenshotId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::Recoverable(
                    "preview.publish browser.screenshot did not return screenshotId".to_string(),
                )
            })?
            .to_string();

        let published =
            report_preview_candidate(&*self.workspace, &ctx, url, screenshot_id, None).await?;
        Ok(ToolResult::ok(json!({
            "published": true,
            "build": build,
            "preview": preview,
            "screenshot": screenshot,
            "promotion": published,
        })))
    }
}

async fn report_preview_candidate(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    url: String,
    screenshot_id: String,
    source_snapshot_uri: Option<&str>,
) -> Result<Value, ToolError> {
    verify_preview_accessible(&url).await?;
    verify_screenshot_artifact(workspace, ctx, &screenshot_id).await?;
    let latest_build = read_workspace_json(workspace, ctx, "outputs/build/latest.json")
        .await
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires successful project.build evidence".to_string(),
                "preview.build_missing",
                json!({
                    "suggestedAction": "Run project.build or preview.publish before reporting a candidate."
                }),
            )
        })?;
    if !latest_build
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(typed_recoverable(
            "preview.report_candidate blocked because latest project.build did not succeed"
                .to_string(),
            "preview.build_failed",
            json!({
                "latestBuild": latest_build,
                "suggestedAction": "Fix the build error, rerun project.build, then publish again."
            }),
        ));
    }
    let latest_source_snapshot_uri = latest_build
        .get("sourceSnapshotUri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            typed_recoverable(
                "preview.report_candidate requires build sourceSnapshotUri evidence".to_string(),
                "preview.source_snapshot_missing",
                json!({
                    "latestBuild": latest_build.clone(),
                    "suggestedAction": "Rerun project.build so sourceSnapshotUri is recorded."
                }),
            )
        })?;
    let source_snapshot_uri = source_snapshot_uri.unwrap_or(latest_source_snapshot_uri);
    if source_snapshot_uri != latest_source_snapshot_uri {
        return Err(typed_recoverable(
            format!(
                "preview.report_candidate sourceSnapshotUri {source_snapshot_uri} does not match latest project.build {latest_source_snapshot_uri}"
            ),
            "preview.source_snapshot_mismatch",
            json!({
                "receivedSourceSnapshotUri": source_snapshot_uri,
                "latestSourceSnapshotUri": latest_source_snapshot_uri,
                "suggestedAction": "Use the latest project.build sourceSnapshotUri or rerun project.build."
            }),
        ));
    }
    let candidate = ctx
        .store
        .create_project_version_candidate(
            &ctx.project_id,
            &ctx.run.id,
            url.clone(),
            Some(screenshot_id.clone()),
            Some(source_snapshot_uri.to_string()),
        )
        .await;
    ctx.store
        .append_event(AgentEvent::PreviewCandidate {
            run_id: ctx.run.id.clone(),
            url,
            version_id: candidate.id.clone(),
            screenshot_id: Some(screenshot_id.clone()),
            timestamp: Utc::now(),
        })
        .await;
    let review_run = ctx
        .store
        .create_child_run(
            &ctx.run.id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{}", candidate.id)),
            vec![],
        )
        .await
        .map_err(|error| {
            ToolError::Recoverable(format!("failed to create visual review child run: {error}"))
        })?;
    ctx.store
        .append_conversation_item(
            &ctx.project_id,
            Some(&ctx.run.id),
            "progress",
            Some("assistant"),
            "Queued visual review for candidate preview.",
            Some(json!({
                "versionId": candidate.id.clone(),
                "reviewRunId": review_run.id.clone(),
            })),
        )
        .await;
    let gate_report =
        promotion_gate_report_from_workspace(workspace, ctx, Some(&screenshot_id)).await;
    ctx.store
        .update_run_status(&review_run.id, AgentRunStatus::Completed)
        .await
        .map_err(|error| {
            ToolError::Recoverable(format!(
                "failed to complete visual review child run: {error}"
            ))
        })?;
    let promoted = promote_preview(
        &ctx.store,
        &ctx.project_id,
        &ctx.run.id,
        &candidate.id,
        gate_report,
    )
    .await
    .map_err(|error| ToolError::Recoverable(format!("preview promotion rejected: {error}")))?;
    Ok(json!({
        "versionId": promoted.id,
        "reviewRunId": review_run.id.clone(),
        "status": promoted.status,
        "url": promoted.preview_url,
    }))
}

struct PreviewStartTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for PreviewStartTool {
    fn name(&self) -> &'static str {
        "preview.start"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "url": string_schema("Preview URL"),
                "port": { "type": "integer", "minimum": 1 },
                "command": string_schema("Preview command label"),
                "mode": string_schema("Preview mode: static or framework")
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview start allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let build = read_workspace_json(&*self.workspace, &ctx, "outputs/build/latest.json")
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "preview.start requires a successful project.build first".to_string(),
                    "preview.build_missing",
                    json!({
                        "suggestedAction": "Run project.build or preview.publish before preview.start."
                    }),
                )
            })?;
        if build.get("status").and_then(Value::as_str) != Some("success")
            || build.get("success").and_then(Value::as_bool) != Some(true)
        {
            return Err(typed_recoverable(
                "preview.start blocked because latest project.build did not succeed".to_string(),
                "preview.build_failed",
                json!({
                    "latestBuild": build.clone(),
                    "suggestedAction": "Fix the build error, rerun project.build, then start preview."
                }),
            ));
        }
        let cwd = default_project_dir(&ctx);
        let port = input
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .or_else(|| input.get("url").and_then(Value::as_str).and_then(url_port))
            .unwrap_or(4321);
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}"))
            .to_string();
        let static_output_dir = if verify_preview_accessible(&url).await.is_err() {
            let static_output = start_static_preview_server(&ctx, &cwd, &build, port).await?;
            wait_for_preview_accessible(&url, Duration::from_secs(10)).await?;
            Some(static_output)
        } else {
            optional_static_preview_output_dir(&ctx, &cwd, &build)
        };
        let state = json!({
            "status": "running",
            "url": url,
            "port": port,
            "command": input.get("command").and_then(Value::as_str).unwrap_or("static"),
            "mode": input.get("mode").and_then(Value::as_str).unwrap_or("static"),
            "cwd": display_workspace_path(&cwd, &ctx),
            "staticOutputPath": static_output_dir.as_ref().map(|path| display_workspace_path(path, &ctx)),
            "pid": read_preview_pid(&ctx),
            "build": build,
            "accessible": true,
        });
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

fn static_preview_output_candidates(ctx: &ToolContext) -> [&'static str; 2] {
    if is_fumadocs_docs_project(ctx) {
        ["out", "dist"]
    } else {
        ["dist", "out"]
    }
}

fn detect_static_preview_output_dir(ctx: &ToolContext, app_root: &Path) -> Option<PathBuf> {
    static_preview_output_candidates(ctx)
        .into_iter()
        .map(|name| app_root.join(name))
        .find(|path| path.is_dir())
}

fn static_preview_output_dir_from_build(
    ctx: &ToolContext,
    latest_build: &Value,
) -> Option<PathBuf> {
    latest_build
        .get("staticOutputPath")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| resolve_path(path, &ctx.workspace_root))
        .filter(|path| path.is_dir())
}

fn optional_static_preview_output_dir(
    ctx: &ToolContext,
    app_root: &Path,
    latest_build: &Value,
) -> Option<PathBuf> {
    static_preview_output_dir_from_build(ctx, latest_build)
        .or_else(|| detect_static_preview_output_dir(ctx, app_root))
}

fn resolve_static_preview_output_dir(
    ctx: &ToolContext,
    app_root: &Path,
    latest_build: &Value,
) -> Result<PathBuf, ToolError> {
    if let Some(resolved) = static_preview_output_dir_from_build(ctx, latest_build) {
        return check_existing_path(&resolved, &ctx.workspace_root)
            .map_err(|error| preview_static_output_missing(ctx, &resolved, error));
    }

    detect_static_preview_output_dir(ctx, app_root).ok_or_else(|| {
        preview_static_output_missing(
            ctx,
            &app_root.join(static_preview_output_candidates(ctx)[0]),
            PermissionError::CannotResolve(app_root.to_path_buf()),
        )
    })
}

fn preview_static_output_missing(
    ctx: &ToolContext,
    path: &Path,
    error: PermissionError,
) -> ToolError {
    typed_recoverable(
        format!("preview.start missing dist/out static output: {error:?}"),
        "preview.dist_missing",
        json!({
            "path": display_workspace_path(path, ctx),
            "candidates": static_preview_output_candidates(ctx)
                .into_iter()
                .map(|name| display_workspace_path(&default_project_dir(ctx).join(name), ctx))
                .collect::<Vec<_>>(),
            "suggestedAction": "Run project.build successfully before starting static preview."
        }),
    )
}

async fn start_static_preview_server(
    ctx: &ToolContext,
    app_root: &Path,
    latest_build: &Value,
    port: u16,
) -> Result<PathBuf, ToolError> {
    let static_output = resolve_static_preview_output_dir(ctx, app_root, latest_build)?;
    check_existing_path(&static_output, &ctx.workspace_root)
        .map_err(|error| preview_static_output_missing(ctx, &static_output, error))?;
    stop_preview_pid(ctx);
    let log_dir = ctx.workspace_root.join("outputs/preview");
    fs::create_dir_all(&log_dir).map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let stdout = fs::File::create(log_dir.join("preview.stdout.log"))
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let stderr = fs::File::create(log_dir.join("preview.stderr.log"))
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let mut command = TokioCommand::new("python3");
    command
        .arg("-m")
        .arg("http.server")
        .arg(port.to_string())
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--directory")
        .arg(&static_output)
        .current_dir(app_root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command.spawn().map_err(|error| {
        ToolError::Recoverable(format!("preview.start failed to spawn: {error}"))
    })?;
    let pid = child.id().unwrap_or_default();
    std::mem::drop(child);
    write_preview_pid(ctx, pid).map_err(|error| ToolError::Recoverable(error.to_string()))?;
    Ok(static_output)
}

async fn wait_for_preview_accessible(url: &str, timeout: Duration) -> Result<(), ToolError> {
    let started = Instant::now();
    loop {
        match verify_preview_accessible(url).await {
            Ok(()) => return Ok(()),
            Err(error) if started.elapsed() < timeout => {
                time::sleep(Duration::from_millis(200)).await;
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
}

fn preview_pid_path(ctx: &ToolContext) -> PathBuf {
    ctx.workspace_root.join("state/preview.pid")
}

fn write_preview_pid(ctx: &ToolContext, pid: u32) -> io::Result<()> {
    let path = preview_pid_path(ctx);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, pid.to_string())
}

fn read_preview_pid(ctx: &ToolContext) -> Option<u32> {
    fs::read_to_string(preview_pid_path(ctx))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

fn stop_preview_pid(ctx: &ToolContext) {
    let Some(pid) = read_preview_pid(ctx) else {
        return;
    };
    if pid > 0 {
        #[cfg(unix)]
        {
            let _ = StdCommand::new("kill").arg(pid.to_string()).status();
        }
        #[cfg(windows)]
        {
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status();
        }
    }
    let _ = fs::remove_file(preview_pid_path(ctx));
}

struct PreviewStatusTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for PreviewStatusTool {
    fn name(&self) -> &'static str {
        "preview.status"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview status allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(
            read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
                .await
                .unwrap_or_else(|| {
                    json!({
                        "status": "stopped",
                        "accessible": false,
                        "url": Value::Null,
                    })
                }),
        ))
    }
}

struct PreviewStopTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for PreviewStopTool {
    fn name(&self) -> &'static str {
        "preview.stop"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "preview stop allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        stop_preview_pid(&ctx);
        let mut state = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| json!({ "url": Value::Null }));
        state["status"] = json!("stopped");
        state["accessible"] = json!(false);
        state["pid"] = Value::Null;
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

struct DiagnosticsBuildLogTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for DiagnosticsBuildLogTool {
    fn name(&self) -> &'static str {
        "diagnostics.build_log"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "build log diagnostics allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let path = ctx.workspace_root.join("outputs/build/build.log");
        let text = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .unwrap_or_default();
        let has_terminal_error = has_terminal_error(&text);
        Ok(ToolResult::ok(json!({
            "path": "/workspace/outputs/build/build.log",
            "text": text,
            "hasTerminalError": has_terminal_error,
        })))
    }
}

struct DiagnosticsTypescriptTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for DiagnosticsTypescriptTool {
    fn name(&self) -> &'static str {
        "diagnostics.typescript"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "typescript diagnostics allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(
            read_workspace_json(&*self.workspace, &ctx, "outputs/reports/typescript.json")
                .await
                .unwrap_or_else(|| {
                    json!({
                        "ok": true,
                        "diagnostics": [],
                    })
                }),
        ))
    }
}

struct BrowserOpenTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserOpenTool {
    fn name(&self) -> &'static str {
        "browser.open"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({ "url": string_schema("URL to inspect") }), &["url"])
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "url", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
        if !is_internal_preview_url(url) {
            return PermissionResult::Deny {
                message: "browser.open public internet access is not allowed".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "public internet egress denied".to_string(),
                },
            };
        }
        allow_with_input(input, "browser open internal preview allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let url = required_str(&input, "url")?.to_string();
        let state = json!({
            "url": url,
            "consoleErrors": [],
            "opened": true,
        });
        write_workspace_json(&*self.workspace, &ctx, "state/browser.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

struct BrowserScreenshotTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &'static str {
        "browser.screenshot"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "screenshotId": string_schema("Screenshot artifact id"),
                "blank": { "type": "boolean" }
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "browser screenshot allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let screenshot_id = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| ctx.store.next_id("screenshot"));
        let is_blank = input.get("blank").and_then(Value::as_bool).unwrap_or(false);
        let path = ctx
            .workspace_root
            .join("outputs/screenshots")
            .join(format!("{screenshot_id}.json"));
        let artifact = json!({
            "screenshotId": screenshot_id,
            "blank": is_blank,
            "url": read_workspace_json(&*self.workspace, &ctx, "state/browser.json")
                .await
                .and_then(|state| state.get("url").cloned())
                .unwrap_or(Value::Null),
        });
        self.workspace
            .write_string(
                &ctx,
                &path,
                &serde_json::to_string_pretty(&artifact)
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        Ok(ToolResult::ok(json!({
            "screenshotId": artifact["screenshotId"],
            "path": format!("/workspace/outputs/screenshots/{}.json", artifact["screenshotId"].as_str().unwrap_or("unknown")),
            "blank": is_blank,
        })))
    }
}

struct BrowserInspectTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserInspectTool {
    fn name(&self) -> &'static str {
        "browser.inspect"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "browser inspect allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let browser = read_workspace_json(&*self.workspace, &ctx, "state/browser.json")
            .await
            .unwrap_or_else(|| {
                json!({
                    "url": Value::Null,
                    "consoleErrors": [],
                    "opened": false,
                })
            });
        let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| {
                json!({
                    "status": "stopped",
                    "accessible": false,
                })
            });
        Ok(ToolResult::ok(json!({
            "url": browser.get("url").cloned().unwrap_or(Value::Null),
            "opened": browser.get("opened").cloned().unwrap_or(json!(false)),
            "consoleErrors": browser.get("consoleErrors").cloned().unwrap_or_else(|| json!([])),
            "preview": preview,
        })))
    }
}

fn path_schema() -> Value {
    object_schema(
        json!({ "path": string_schema("Workspace path") }),
        &["path"],
    )
}

fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Recoverable(format!("missing {key}")))
}

fn required_u64(input: &Value, key: &str) -> Result<u64, ToolError> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::Recoverable(format!("missing numeric {key}")))
}

fn required_u64_validation(input: &Value, key: &str, tool: &str) -> Result<u64, ValidationError> {
    input.get(key).and_then(Value::as_u64).ok_or_else(|| {
        ValidationError::with_kind(
            format!("{tool} requires numeric {key}"),
            "tool.input_schema_invalid",
        )
    })
}

fn require_string(input: &Value, key: &str, tool: &str) -> Result<(), ValidationError> {
    if input.get(key).and_then(Value::as_str).is_some() {
        return Ok(());
    }
    Err(ValidationError::new(format!("{tool} requires {key}")))
}

fn validate_write_payload_budget(input: &Value, tool: &str) -> Result<(), ValidationError> {
    let serialized_bytes = serde_json::to_vec(input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    let text_chars = input
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    if serialized_bytes > MAX_DIRECT_WRITE_ARGUMENT_BYTES
        || text_chars > MAX_DIRECT_WRITE_TEXT_CHARS
    {
        return Err(
            ValidationError::with_kind(LARGE_WRITE_GUIDANCE, "tool.input_too_large").with_metadata(
                json!({
                    "tool": tool,
                    "path": input.get("path").and_then(Value::as_str).unwrap_or("unknown"),
                    "inputChars": text_chars,
                    "serializedBytes": serialized_bytes,
                    "maxInputChars": MAX_DIRECT_WRITE_TEXT_CHARS,
                    "maxSerializedBytes": MAX_DIRECT_WRITE_ARGUMENT_BYTES,
                    "guidance": LARGE_WRITE_GUIDANCE,
                }),
            ),
        );
    }
    Ok(())
}

fn validate_chunk_payload_budget(input: &Value) -> Result<(), ValidationError> {
    let serialized_bytes = serde_json::to_vec(input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    let text_chars = input
        .get("text")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    if serialized_bytes > MAX_CHUNK_ARGUMENT_BYTES || text_chars > MAX_CHUNK_TEXT_CHARS {
        return Err(ValidationError::with_kind(
            "fs.write_chunk input too large. Split the file into smaller chunks before retrying.",
            "tool.input_too_large",
        )
        .with_metadata(json!({
            "tool": "fs.write_chunk",
            "path": input.get("path").and_then(Value::as_str).unwrap_or("unknown"),
            "inputChars": text_chars,
            "serializedBytes": serialized_bytes,
            "maxInputChars": MAX_CHUNK_TEXT_CHARS,
            "maxSerializedBytes": MAX_CHUNK_ARGUMENT_BYTES,
            "guidance": "Split the file into smaller chunks before retrying fs.write_chunk.",
        })));
    }
    Ok(())
}

fn validate_chunk_bounds(index: u64, total: u64) -> Result<(), ValidationError> {
    if total == 0 || total > MAX_CHUNKS_PER_WRITE || index >= total {
        return Err(ValidationError::with_kind(
            format!(
                "chunk bounds invalid: index={index}, total={total}, max={MAX_CHUNKS_PER_WRITE}"
            ),
            "tool.input_schema_invalid",
        ));
    }
    Ok(())
}

fn validate_chunk_bounds_tool(index: u64, total: u64) -> Result<(), ToolError> {
    if total == 0 || total > MAX_CHUNKS_PER_WRITE || index >= total {
        return Err(ToolError::Recoverable(format!(
            "chunk bounds invalid: index={index}, total={total}, max={MAX_CHUNKS_PER_WRITE}"
        )));
    }
    Ok(())
}

fn safe_session_id(session_id: &str) -> Result<String, ToolError> {
    let sanitized = session_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    if sanitized.is_empty() {
        return Err(ToolError::Recoverable(
            "sessionId must contain at least one ASCII letter, number, '-' or '_'".to_string(),
        ));
    }
    Ok(sanitized)
}

fn staged_session_dir(ctx: &ToolContext, session_id: &str) -> PathBuf {
    ctx.workspace_root
        .join("outputs")
        .join("staged-writes")
        .join(session_id)
}

fn staged_manifest_path(ctx: &ToolContext, session_id: &str) -> PathBuf {
    staged_session_dir(ctx, session_id).join("manifest.json")
}

fn staged_chunk_path(ctx: &ToolContext, session_id: &str, index: u64) -> PathBuf {
    staged_session_dir(ctx, session_id).join(format!("chunk-{index:05}.txt"))
}

async fn update_chunk_manifest(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    session_id: &str,
    final_path: &Path,
    index: u64,
    total: u64,
) -> Result<(), ToolError> {
    let manifest_path = staged_manifest_path(ctx, session_id);
    let display_path = display_workspace_path(final_path, ctx);
    let mut manifest = workspace
        .read_to_string(ctx, &manifest_path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| {
            json!({
                "sessionId": session_id,
                "runId": ctx.run.id,
                "path": display_path,
                "total": total,
                "chunks": [],
                "createdAt": Utc::now(),
                "updatedAt": Utc::now(),
            })
        });
    if manifest.get("runId").and_then(Value::as_str) != Some(ctx.run.id.as_str()) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} belongs to another run"
        )));
    }
    if manifest.get("path").and_then(Value::as_str) != Some(display_path.as_str()) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} targets a different path"
        )));
    }
    if manifest.get("total").and_then(Value::as_u64) != Some(total) {
        return Err(ToolError::Recoverable(format!(
            "chunk session {session_id} has different total"
        )));
    }
    let chunks = manifest
        .as_object_mut()
        .and_then(|object| object.get_mut("chunks"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ToolError::Recoverable("chunk manifest is invalid".to_string()))?;
    if chunks.iter().any(|value| value.as_u64() == Some(index)) {
        return Err(ToolError::Recoverable(format!(
            "duplicate chunk {index} for session {session_id}"
        )));
    }
    chunks.push(json!(index));
    chunks.sort_by_key(|value| value.as_u64().unwrap_or(u64::MAX));
    manifest["updatedAt"] = json!(Utc::now());
    write_workspace_json_path(workspace, ctx, &manifest_path, &manifest).await
}

async fn read_chunk_manifest(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    session_id: &str,
) -> Option<Value> {
    workspace
        .read_to_string(ctx, &staged_manifest_path(ctx, session_id))
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn validate_chunk_manifest_for_commit(
    manifest: &Value,
    ctx: &ToolContext,
    final_path: &Path,
    total: u64,
) -> Result<(), ToolError> {
    let display_path = display_workspace_path(final_path, ctx);
    if manifest.get("runId").and_then(Value::as_str) != Some(ctx.run.id.as_str()) {
        return Err(ToolError::Recoverable(
            "chunk session belongs to another run".to_string(),
        ));
    }
    if manifest.get("path").and_then(Value::as_str) != Some(display_path.as_str()) {
        return Err(ToolError::Recoverable(
            "chunk session targets a different path".to_string(),
        ));
    }
    if manifest.get("total").and_then(Value::as_u64) != Some(total) {
        return Err(ToolError::Recoverable(
            "chunk session total does not match commit total".to_string(),
        ));
    }
    let chunks = manifest
        .get("chunks")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::Recoverable("chunk manifest is invalid".to_string()))?;
    for index in 0..total {
        if !chunks.iter().any(|value| value.as_u64() == Some(index)) {
            return Err(ToolError::Recoverable(format!(
                "missing chunk {index}/{total} in session manifest"
            )));
        }
    }
    Ok(())
}

async fn cleanup_expired_staged_writes(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
) -> Result<(), ToolError> {
    let root = ctx.workspace_root.join("outputs/staged-writes");
    let Ok(entries) = workspace.list_dir(ctx, &root).await else {
        return Ok(());
    };
    for entry in entries {
        if entry.kind != WorkspaceEntryKind::Dir {
            continue;
        }
        let manifest_path = entry.path.join("manifest.json");
        let Some(manifest) = workspace
            .read_to_string(ctx, &manifest_path)
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let Some(updated_at) = manifest.get("updatedAt").and_then(Value::as_str) else {
            continue;
        };
        let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at) else {
            continue;
        };
        if Utc::now()
            .signed_duration_since(updated_at.with_timezone(&Utc))
            .num_seconds()
            > STAGED_WRITE_TTL_SECS
        {
            let _ = workspace.remove_dir_all(ctx, &entry.path).await;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn resolve_path(path: &str, workspace_root: &Path) -> PathBuf {
    if path == "/workspace" {
        return workspace_root.to_path_buf();
    }
    if let Some(relative) = path.strip_prefix("/workspace/") {
        return workspace_root.join(relative);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn validate_workspace_path_input(
    input: &Value,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError::new(format!("{tool_name} requires path")))?;
    let resolved = resolve_path(path, &ctx.workspace_root);
    match check_workspace_path(&resolved, &ctx.workspace_root) {
        Ok(_) => Ok(()),
        Err(PermissionError::SecretPath(_)) => Ok(()),
        Err(error) => Err(path_validation_error(tool_name, path, &resolved, error)),
    }
}

fn path_validation_error(
    tool_name: &str,
    received_path: &str,
    resolved: &Path,
    error: PermissionError,
) -> ValidationError {
    let (error_kind, guidance, suggested_path) = match error {
        PermissionError::ExternalDirectory(_) => (
            "path.external_directory",
            "Use workspace-relative paths such as project/src/pages/index.astro.",
            Some("project"),
        ),
        PermissionError::InvalidPathComponent(_) => (
            "path.invalid_component",
            "Remove '..' or other invalid path components and stay inside the workspace.",
            Some("project"),
        ),
        PermissionError::SecretPath(_) => (
            "path.secret",
            "Choose a non-secret project source path.",
            None,
        ),
        PermissionError::CannotResolve(_) => (
            "path.cannot_resolve",
            "Use an existing workspace path or a creatable path under the project app root.",
            Some("project"),
        ),
    };
    ValidationError::with_kind(
        format!("{tool_name} path is not usable: {received_path}"),
        error_kind,
    )
    .with_metadata(json!({
        "tool": tool_name,
        "receivedPath": received_path,
        "resolvedPath": resolved.display().to_string(),
        "suggestedPath": suggested_path,
        "guidance": guidance,
    }))
}

fn validate_fumadocs_app_router_write_path(
    input: &Value,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return Ok(());
    };
    let resolved = resolve_path(path, &ctx.workspace_root);
    if !is_forbidden_fumadocs_pages_router_path(&resolved, ctx) {
        return Ok(());
    }
    Err(fumadocs_routing_root_validation_error(
        tool_name, path, &resolved, ctx,
    ))
}

fn fumadocs_routing_root_validation_error(
    tool_name: &str,
    received_path: &str,
    resolved: &Path,
    ctx: &ToolContext,
) -> ValidationError {
    let app_root = default_project_dir(ctx);
    let app_root_display = display_workspace_path(&app_root, ctx);
    ValidationError::with_kind(
        format!("{tool_name} cannot write a pages-router file in a fumadocs-docs project: {received_path}"),
        "docs.routing_root_forbidden",
    )
    .with_metadata(json!({
        "tool": tool_name,
        "receivedPath": received_path,
        "resolvedPath": display_workspace_path(resolved, ctx),
        "appRoot": app_root_display,
        "forbiddenPaths": [
            format!("{app_root_display}/pages"),
            format!("{app_root_display}/src/pages")
        ],
        "suggestedAction": "Keep fumadocs-docs projects on the Next app router. Write docs routes under app/docs/[[...slug]] and MDX content under content/docs; do not create project/pages or project/src/pages."
    }))
}

fn typed_recoverable(
    message: impl Into<String>,
    error_kind: impl Into<String>,
    metadata: Value,
) -> ToolError {
    ToolError::typed_recoverable(message, error_kind, metadata)
}

fn style_validation_error(message: impl Into<String>) -> ValidationError {
    ValidationError::with_kind(message, "style.input_invalid").with_metadata(json!({
        "suggestedAction": "Pass a non-empty tokens object using token names declared in state/style-contract.json."
    }))
}

fn style_typed_recoverable(
    message: impl Into<String>,
    error_kind: impl Into<String>,
    metadata: Value,
) -> ToolError {
    let mut metadata = metadata;
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "tool".to_string(),
            Value::String("style.update_tokens".to_string()),
        );
    }
    typed_recoverable(message, error_kind, metadata)
}

fn patch_recovery_metadata(
    ctx: &ToolContext,
    path: &Path,
    old_str: &str,
    match_count: usize,
    content: Option<&str>,
) -> Value {
    json!({
        "path": display_workspace_path(path, ctx),
        "oldStrPreview": old_str.chars().take(160).collect::<String>(),
        "matchCount": match_count,
        "suggestedAction": if match_count > 1 {
            "Provide a larger unique oldStr or set replaceAll=true when every occurrence should change."
        } else {
            "Read the file again and retry with a small exact snippet from current contents."
        },
        "nearestSnippets": content
            .map(|content| nearest_patch_snippets(content, old_str))
            .unwrap_or_default(),
    })
}

fn nearest_patch_snippets(content: &str, old_str: &str) -> Vec<Value> {
    let needle = old_str
        .split_whitespace()
        .max_by_key(|part| part.len())
        .unwrap_or("")
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_');
    if needle.len() < 3 {
        return Vec::new();
    }
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .take(3)
        .map(|(index, line)| {
            json!({
                "line": index + 1,
                "text": line.trim().chars().take(240).collect::<String>(),
            })
        })
        .collect()
}

fn checked_existing_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let path = required_str(input, "path")?;
    check_workspace_path(
        &resolve_path(path, &ctx.workspace_root),
        &ctx.workspace_root,
    )
    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
}

fn checked_write_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let path = required_str(input, "path")?;
    let path = resolve_path(path, &ctx.workspace_root);
    if path.exists() {
        check_existing_path(&path, &ctx.workspace_root)
    } else {
        check_create_path(&path, &ctx.workspace_root)
    }
    .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
    .and_then(|path| {
        ensure_not_nested_package_root(&path, ctx)?;
        ensure_fumadocs_app_router_write_path(&path, ctx)?;
        Ok(path)
    })
}

fn checked_delete_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, String> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "fs.delete requires path".to_string())?;
    let path = check_existing_path(
        &resolve_path(path, &ctx.workspace_root),
        &ctx.workspace_root,
    )
    .map_err(|error| format!("{error:?}"))?;
    let app_root = default_project_dir(ctx);
    let app_root = fs::canonicalize(&app_root).map_err(|error| error.to_string())?;
    if path == ctx.workspace_root
        || path == app_root
        || path == ctx.workspace_root.join("inputs")
        || path == ctx.workspace_root.join("state")
        || path == ctx.workspace_root.join("outputs")
        || !path.starts_with(&app_root)
    {
        return Err(format!(
            "fs.delete is limited to non-root paths under {}",
            display_workspace_path(&app_root, ctx)
        ));
    }
    Ok(path)
}

fn check_existing_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    match input.get("path").and_then(Value::as_str).map(|path| {
        check_workspace_path(
            &resolve_path(path, &ctx.workspace_root),
            &ctx.workspace_root,
        )
    }) {
        Some(Ok(_)) => allow_with_input(input, "workspace path allowed"),
        Some(Err(error)) => deny(tool, format!("{error:?}")),
        None => deny(tool, "missing path"),
    }
}

fn check_existing_write_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    match input.get("path").and_then(Value::as_str).map(|path| {
        check_workspace_path(
            &resolve_path(path, &ctx.workspace_root),
            &ctx.workspace_root,
        )
    }) {
        Some(Ok(path)) => {
            if let Err(error) = ensure_not_nested_package_root(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_fumadocs_app_router_write_path(&path, ctx) {
                deny(tool, error.message())
            } else {
                allow_with_input(input, "workspace edit path allowed")
            }
        }
        Some(Err(error)) => deny(tool, format!("{error:?}")),
        None => deny(tool, "missing path"),
    }
}

fn check_write_path_permission(input: &Value, ctx: &ToolContext, tool: &str) -> PermissionResult {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return deny(tool, "missing path");
    };
    let path = resolve_path(path, &ctx.workspace_root);
    let result = if path.exists() {
        check_existing_path(&path, &ctx.workspace_root)
    } else {
        check_create_path(&path, &ctx.workspace_root)
    };
    match result {
        Ok(path) => {
            if let Err(error) = ensure_not_nested_package_root(&path, ctx) {
                deny(tool, error.message())
            } else if let Err(error) = ensure_fumadocs_app_router_write_path(&path, ctx) {
                deny(tool, error.message())
            } else {
                allow_with_input(input, "workspace write path allowed")
            }
        }
        Err(error) => deny(tool, format!("{error:?}")),
    }
}

fn allow_with_input(input: &Value, reason: impl Into<String>) -> PermissionResult {
    PermissionResult::Allow {
        updated_input: input.clone(),
        reason: PermissionReason::Other {
            reason: reason.into(),
        },
    }
}

fn deny(tool: &str, reason: impl Into<String>) -> PermissionResult {
    let reason = reason.into();
    PermissionResult::Deny {
        message: format!("{tool} denied: {reason}"),
        reason: PermissionReason::Rule {
            source: RuleSource::Runtime,
            rule_content: reason,
        },
    }
}

async fn collect_search_matches(
    workspace: &dyn WorkspaceBackend,
    path: &Path,
    ctx: &ToolContext,
    query: &str,
    matches: &mut Vec<Value>,
) -> Result<(), ToolError> {
    let mut stack = vec![path.to_path_buf()];
    while let Some(path) = stack.pop() {
        match workspace
            .path_kind(ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?
        {
            WorkspacePathKind::File => {
                let text = workspace
                    .read_to_string(ctx, &path)
                    .await
                    .unwrap_or_default();
                for (index, line) in text.lines().enumerate() {
                    if line.contains(query) {
                        matches.push(json!({
                            "path": display_workspace_path(&path, ctx),
                            "line": index + 1,
                            "text": line,
                        }));
                    }
                }
            }
            WorkspacePathKind::Dir => {
                for entry in workspace
                    .list_dir(ctx, &path)
                    .await
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?
                {
                    stack.push(entry.path);
                }
            }
        }
    }
    Ok(())
}

fn default_project_dir(ctx: &ToolContext) -> PathBuf {
    ctx.workspace_root.join(project_app_root_relative(ctx))
}

fn project_app_root_relative(ctx: &ToolContext) -> PathBuf {
    fs::read_to_string(ctx.workspace_root.join("state/project.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("appRoot")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .and_then(|path| normalize_workspace_relative_path(&path).ok())
        .unwrap_or_else(|| PathBuf::from("project"))
}

fn package_manager_from_project_state_or_lockfiles(ctx: &ToolContext, app_root: &Path) -> String {
    fs::read_to_string(ctx.workspace_root.join("state/project.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("packageManager")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            if app_root.join("pnpm-lock.yaml").exists() {
                Some("pnpm".to_string())
            } else if app_root.join("package-lock.json").exists() {
                Some("npm".to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "npm".to_string())
}

fn project_key_source_files(
    ctx: &ToolContext,
    app_root_relative: &Path,
    project_state: Option<&Value>,
) -> Vec<Value> {
    let template = project_state
        .and_then(|state| state.get("templateKey").and_then(Value::as_str))
        .unwrap_or("astro-website");
    let candidates: &[&str] = if template == "fumadocs-docs" {
        &[
            "package.json",
            "next.config.mjs",
            "source.config.ts",
            "app/global.css",
            "app/tokens.css",
            "app/docs/layout.jsx",
            "app/docs/[[...slug]]/page.jsx",
            "content/docs/index.mdx",
            "content/docs/meta.json",
        ]
    } else {
        &[
            "package.json",
            "astro.config.mjs",
            "src/styles/tokens.css",
            "src/styles/global.css",
            "src/pages/index.astro",
            "src/components/ui/Button.astro",
        ]
    };
    candidates
        .iter()
        .map(|relative| {
            let path = app_root_relative.join(relative);
            let absolute = ctx.workspace_root.join(&path);
            json!({
                "path": format!("/workspace/{}", path.to_string_lossy().replace('\\', "/")),
                "exists": absolute.exists(),
            })
        })
        .collect()
}

fn normalize_workspace_relative_path(path: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(ToolError::PermissionDenied(
            "workspace path must be relative".to_string(),
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ToolError::PermissionDenied(
            "workspace path must stay inside the workspace".to_string(),
        ));
    }
    let normalized = normalize_path(path);
    if normalized.as_os_str().is_empty() {
        return Err(ToolError::PermissionDenied(
            "workspace path must stay inside the workspace".to_string(),
        ));
    }
    Ok(normalized)
}

fn ensure_not_nested_package_root(path: &Path, ctx: &ToolContext) -> Result<(), ToolError> {
    if path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
        return Ok(());
    }
    if !matches!(
        ctx.run.phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair
    ) {
        return Ok(());
    }
    let app_root = default_project_dir(ctx);
    let app_package = app_root.join("package.json");
    if path != app_package && path.starts_with(&app_root) {
        let app_root_display = display_workspace_path(&app_root, ctx);
        let path_display = display_workspace_path(path, ctx);
        return Err(typed_recoverable(
            format!(
                "nested package root denied: write source files under {app_root_display} instead of creating {path_display}"
            ),
            "path.nested_package_root",
            json!({
                "path": path_display,
                "appRoot": app_root_display,
                "suggestedAction": "Use the existing app package.json at the app root, or write source files under the app root without creating another package.json."
            }),
        ));
    }
    Ok(())
}

fn ensure_fumadocs_app_router_write_path(path: &Path, ctx: &ToolContext) -> Result<(), ToolError> {
    if !is_forbidden_fumadocs_pages_router_path(path, ctx) {
        return Ok(());
    }
    let app_root = default_project_dir(ctx);
    let app_root_display = display_workspace_path(&app_root, ctx);
    let path_display = display_workspace_path(path, ctx);
    Err(typed_recoverable(
        format!(
            "fumadocs-docs route root denied: write app-router docs under {app_root_display}/app instead of creating {path_display}"
        ),
        "docs.routing_root_forbidden",
        json!({
            "path": path_display,
            "appRoot": app_root_display,
            "forbiddenPaths": [
                format!("{app_root_display}/pages"),
                format!("{app_root_display}/src/pages")
            ],
            "suggestedAction": "Keep fumadocs-docs projects on the Next app router. Write docs routes under app/docs/[[...slug]] and MDX content under content/docs; do not create project/pages or project/src/pages."
        }),
    ))
}

fn is_forbidden_fumadocs_pages_router_path(path: &Path, ctx: &ToolContext) -> bool {
    if !is_fumadocs_docs_project(ctx) {
        return false;
    }
    let app_root = default_project_dir(ctx);
    let pages_root = app_root.join("pages");
    let src_pages_root = app_root.join("src/pages");
    path == pages_root
        || path.starts_with(&pages_root)
        || path == src_pages_root
        || path.starts_with(&src_pages_root)
}

fn is_fumadocs_docs_project(ctx: &ToolContext) -> bool {
    let state = fs::read_to_string(ctx.workspace_root.join("state/project.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if state
        .as_ref()
        .and_then(|value| value.get("templateKey").or_else(|| value.get("template")))
        .and_then(Value::as_str)
        == Some("fumadocs-docs")
    {
        return true;
    }
    let app_root = default_project_dir(ctx);
    fs::read_to_string(app_root.join("package.json")).is_ok_and(|package_json| {
        package_json.contains("\"fumadocs-ui\"") || package_json.contains("\"fumadocs-mdx\"")
    })
}

fn display_workspace_path(path: &Path, ctx: &ToolContext) -> String {
    path.strip_prefix(&ctx.workspace_root)
        .map(|path| format!("/workspace/{}", path.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

async fn write_workspace_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &str,
    value: &Value,
) -> Result<(), ToolError> {
    let path = ctx.workspace_root.join(path);
    write_workspace_json_path(workspace, ctx, &path, value).await
}

async fn write_workspace_json_path(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
    value: &Value,
) -> Result<(), ToolError> {
    workspace
        .write_string(
            ctx,
            path,
            &serde_json::to_string_pretty(value)
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))
}

async fn record_chunk_write_health(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    chunk_write: Value,
) -> Result<(), ToolError> {
    let mut health = read_workspace_json(workspace, ctx, "state/run-health.json")
        .await
        .unwrap_or_else(|| json!({ "chunkWrites": [] }));
    let chunk_writes = health
        .as_object_mut()
        .and_then(|object| object.get_mut("chunkWrites"))
        .and_then(Value::as_array_mut);
    match chunk_writes {
        Some(entries) => {
            entries.push(chunk_write);
            if entries.len() > 20 {
                let drain_count = entries.len() - 20;
                entries.drain(0..drain_count);
            }
        }
        None => {
            health["chunkWrites"] = json!([chunk_write]);
        }
    }
    write_workspace_json(workspace, ctx, "state/run-health.json", &health).await
}

fn record_read_path(ctx: &ToolContext, path: &Path, content: &str) -> Result<(), ToolError> {
    let display_path = display_workspace_path(path, ctx);
    let tracking_path = ctx.workspace_root.join("state/read-tracking.json");
    let mut tracking = fs::read_to_string(&tracking_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({ "paths": [] }));
    if !tracking.is_object() {
        tracking = json!({ "paths": [] });
    }
    let paths = tracking
        .as_object_mut()
        .and_then(|object| object.get_mut("paths"))
        .and_then(Value::as_array_mut);
    let entry = json!({
        "path": display_path,
        "runId": ctx.run.id,
        "readAt": Utc::now(),
        "contentHash": sha256_hex(content.as_bytes()),
        "bytes": content.len(),
    });
    match paths {
        Some(entries) => {
            entries.retain(|value| {
                value.get("path").and_then(Value::as_str)
                    != entry.get("path").and_then(Value::as_str)
                    || value.get("runId").and_then(Value::as_str)
                        != entry.get("runId").and_then(Value::as_str)
            });
            entries.push(entry);
            if entries.len() > 100 {
                let drain_count = entries.len() - 100;
                entries.drain(0..drain_count);
            }
        }
        None => {
            tracking["paths"] = json!([entry]);
        }
    }
    if let Some(parent) = tracking_path.parent() {
        fs::create_dir_all(parent).map_err(|error| ToolError::Recoverable(error.to_string()))?;
    }
    fs::write(
        tracking_path,
        serde_json::to_string_pretty(&tracking)
            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
    )
    .map_err(|error| ToolError::Recoverable(error.to_string()))
}

fn read_tracking_entry(ctx: &ToolContext, path: &Path) -> Option<Value> {
    let display_path = display_workspace_path(path, ctx);
    fs::read_to_string(ctx.workspace_root.join("state/read-tracking.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|tracking| tracking.get("paths").cloned())
        .and_then(|paths| paths.as_array().cloned())
        .and_then(|entries| {
            entries.into_iter().find(|entry| {
                entry.get("path").and_then(Value::as_str) == Some(display_path.as_str())
                    && entry.get("runId").and_then(Value::as_str) == Some(ctx.run.id.as_str())
            })
        })
}

pub fn cleanup_staged_writes_for_run(workspace_root: &Path, run_id: &str) {
    let root = workspace_root.join("outputs/staged-writes");
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let belongs_to_run = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|manifest| {
                manifest
                    .get("runId")
                    .and_then(Value::as_str)
                    .map(|manifest_run_id| manifest_run_id == run_id)
            })
            .unwrap_or(false);
        if belongs_to_run {
            let _ = fs::remove_dir_all(path);
        }
    }
}

async fn read_workspace_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &str,
) -> Option<Value> {
    workspace
        .read_to_string(ctx, &ctx.workspace_root.join(path))
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

#[derive(Debug, Clone, Default)]
struct DependencyRestoreOutcome {
    attempted: bool,
    succeeded: bool,
    reason: Option<String>,
    log_path: Option<String>,
}

async fn maybe_restore_project_dependencies(
    workspace: &dyn WorkspaceBackend,
    command: &dyn SandboxCommandBackend,
    ctx: &ToolContext,
    progress: &ProgressSink,
    cwd: &Path,
    package_manager: &str,
) -> Result<DependencyRestoreOutcome, ToolError> {
    let reason = dependency_restore_reason(workspace, ctx, cwd).await?;
    let Some(reason) = reason else {
        return Ok(DependencyRestoreOutcome::default());
    };
    let registry = ctx.npm_registry.clone();
    if is_public_registry(&registry) && ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
        return Err(typed_recoverable(
            "project.build dependency restore requires package.install policy, and public npm registry is denied outside local-e2e policy profile".to_string(),
            "build.missing_dependency",
            json!({
                "registry": registry,
                "policyProfile": format!("{:?}", ctx.policy_profile),
                "suggestedAction": "Use an allowed internal registry or local-e2e policy for public registry restores."
            }),
        ));
    }
    progress
        .emit_tool_output(
            "package.install",
            "stdout",
            format!("runtime dependency restore before project.build: {reason}\n"),
        )
        .await;
    let argv = package_install_argv(package_manager, "restore", &[], &registry);
    let output = command
        .run_with_output_events(
            ctx,
            &argv,
            cwd,
            120_000,
            Some(progress.clone()),
            "package.install",
        )
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                typed_recoverable(
                    "project.build dependency restore timed out".to_string(),
                    "build.missing_dependency",
                    json!({
                        "reason": reason,
                        "packageManager": package_manager,
                        "suggestedAction": "Retry project.build or run project.ensure_dependencies after checking package registry connectivity."
                    }),
                )
            } else if error.kind() == io::ErrorKind::Interrupted {
                ToolError::Recoverable(error.to_string())
            } else {
                typed_recoverable(
                    format!("project.build dependency restore failed to start {package_manager}: {error}"),
                    "build.missing_dependency",
                    json!({
                        "reason": reason,
                        "packageManager": package_manager,
                        "suggestedAction": "Run project.ensure_dependencies or verify the package manager is available."
                    }),
                )
            }
        })?;
    let restore_tool_use_id = format!("{}-restore", progress.tool_use_id());
    let log_path =
        write_package_install_log(workspace, ctx, &restore_tool_use_id, &argv, &output).await?;
    let state = json!({
        "needsRestore": !output.success,
        "reason": reason,
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": log_path,
        "packageManager": package_manager,
        "status": output.status,
        "success": output.success,
    });
    write_workspace_json(workspace, ctx, "state/dependency-state.json", &state).await?;
    if !output.success {
        return Err(typed_recoverable(
            format!(
                "project.build dependency restore failed with status {:?}; log: {}",
                output.status, log_path
            ),
            "build.missing_dependency",
            json!({
                "reason": reason,
                "packageManager": package_manager,
                "status": output.status,
                "logPath": log_path,
                "suggestedAction": "Open diagnostics.build_log or rerun project.ensure_dependencies after fixing dependency errors."
            }),
        ));
    }
    Ok(DependencyRestoreOutcome {
        attempted: true,
        succeeded: true,
        reason: Some(reason),
        log_path: Some(log_path),
    })
}

fn validate_package_install_like_input(
    input: &Value,
    tool_name: &str,
) -> Result<(), ValidationError> {
    let packages = package_specs_from_input(input);
    if input.get("packages").is_some()
        && !input
            .get("packages")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(|item| item.as_str().is_some()))
    {
        return Err(ValidationError::new(format!(
            "{tool_name} packages must be a string array"
        )));
    }
    let mode = package_install_mode_from_input(input)?;
    match mode.as_str() {
        "add" if packages.is_empty() => {
            return Err(ValidationError::new(format!(
                "{tool_name} mode=add requires a non-empty packages array"
            )));
        }
        "restore" if !packages.is_empty() => {
            return Err(ValidationError::new(format!(
                "{tool_name} mode=restore must omit packages"
            )));
        }
        "add" | "restore" => {}
        _ => unreachable!("package_install_mode_from_input validates mode"),
    }
    if let Some(package_manager) = input.get("packageManager").and_then(Value::as_str) {
        validate_package_manager(package_manager)?;
    }
    Ok(())
}

fn package_install_permission(
    tool_name: &str,
    input: &Value,
    ctx: &ToolContext,
) -> PermissionResult {
    let registry = input
        .get("registry")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.npm_registry);
    let packages = package_specs_from_input(input);
    let public_registry = is_public_registry(registry)
        || packages
            .iter()
            .any(|package| package.starts_with("http://") || package.starts_with("https://"));
    if public_registry {
        if ctx.policy_profile != RuntimePolicyProfile::LocalE2e {
            return deny(
                tool_name,
                "public npm registry is denied outside local-e2e policy profile",
            );
        }
        return allow_with_input(input, "local-e2e public package source allowed");
    }
    for package in &packages {
        if let Some(local_path) = package.strip_prefix("file:") {
            let resolved = resolve_path(local_path, &default_project_dir(ctx));
            if let Err(error) = check_existing_path(&resolved, &ctx.workspace_root) {
                return deny(tool_name, format!("{error:?}"));
            }
        }
    }
    allow_with_input(input, "internal registry package install allowed")
}

async fn run_package_install(
    tool_name: &str,
    workspace: &dyn WorkspaceBackend,
    command: &dyn SandboxCommandBackend,
    input: Value,
    ctx: ToolContext,
    progress: ProgressSink,
) -> Result<Value, ToolError> {
    let packages = package_specs_from_input(&input);
    let mode = package_install_mode_from_input(&input)
        .map_err(|error| ToolError::Recoverable(error.message))?;
    let registry = input
        .get("registry")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.npm_registry)
        .to_string();
    let cwd = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(|cwd| {
            check_existing_path(&resolve_path(cwd, &ctx.workspace_root), &ctx.workspace_root)
                .map_err(|error| ToolError::PermissionDenied(format!("{error:?}")))
        })
        .transpose()?
        .unwrap_or_else(|| default_project_dir(&ctx));
    ensure_project_package_json(workspace, &ctx, &cwd).await?;

    let package_manager = package_manager_from_input_or_project(&input, &ctx, &cwd)?;
    let argv = package_install_argv(&package_manager, &mode, &packages, &registry);
    let timeout_ms = input
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(120_000);
    let output = command
        .run_with_output_events(
            &ctx,
            &argv,
            &cwd,
            timeout_ms,
            Some(progress.clone()),
            tool_name,
        )
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                ToolError::typed_recoverable(
                    format!("{tool_name} timed out"),
                    "dependency.install_timeout",
                    json!({
                        "toolName": tool_name,
                        "packageManager": package_manager,
                        "mode": mode,
                        "packages": packages,
                        "registry": registry,
                        "cwd": display_workspace_path(&cwd, &ctx),
                        "timeoutMs": timeout_ms,
                        "suggestedAction": "Retry project.ensure_dependencies with a larger timeoutMs after checking registry connectivity, then rerun project.build or preview.publish.",
                    }),
                )
            } else if error.kind() == io::ErrorKind::Interrupted {
                ToolError::Recoverable(error.to_string())
            } else {
                ToolError::typed_recoverable(
                    format!("{tool_name} failed to start {package_manager}: {error}"),
                    "dependency.install_failed",
                    json!({
                        "toolName": tool_name,
                        "packageManager": package_manager,
                        "mode": mode,
                        "packages": packages,
                        "registry": registry,
                        "cwd": display_workspace_path(&cwd, &ctx),
                        "suggestedAction": "Verify the package manager is available and retry project.ensure_dependencies before building.",
                    }),
                )
            }
        })?;
    let log_path =
        write_package_install_log(workspace, &ctx, progress.tool_use_id(), &argv, &output).await?;
    let dependency_state = json!({
        "needsRestore": !output.success,
        "reason": if output.success { Value::Null } else { json!(format!("{tool_name}_failed")) },
        "lastRestoreAt": Utc::now().to_rfc3339(),
        "lastRestoreLogPath": log_path.clone(),
        "packageManager": package_manager.clone(),
        "mode": mode.clone(),
        "packages": packages.clone(),
        "status": output.status,
        "success": output.success,
    });
    write_workspace_json(
        workspace,
        &ctx,
        "state/dependency-state.json",
        &dependency_state,
    )
    .await?;
    if !output.success {
        return Err(ToolError::typed_recoverable(
            format!(
                "{tool_name} failed with status {:?}; log: {}",
                output.status, log_path
            ),
            "dependency.install_failed",
            json!({
                "toolName": tool_name,
                "packageManager": package_manager,
                "mode": mode,
                "packages": packages,
                "registry": registry,
                "cwd": display_workspace_path(&cwd, &ctx),
                "status": output.status,
                "logPath": log_path,
                "stderr": truncate_for_metadata(&output.stderr),
                "suggestedAction": "Open the package install log, fix registry or package errors, then rerun project.ensure_dependencies.",
            }),
        ));
    }

    Ok(json!({
        "installed": dependency_state["packages"],
        "registry": registry,
        "mode": dependency_state["mode"],
        "packageManager": dependency_state["packageManager"],
        "manager": dependency_state["packageManager"],
        "command": argv,
        "cwd": display_workspace_path(&cwd, &ctx),
        "status": output.status,
        "success": true,
        "logPath": log_path,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

async fn dependency_restore_reason(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<Option<String>, ToolError> {
    if read_workspace_json(workspace, ctx, "state/dependency-state.json")
        .await
        .and_then(|state| state.get("needsRestore").and_then(Value::as_bool))
        == Some(true)
    {
        return Ok(Some(
            "source_snapshot_restored_without_node_modules".to_string(),
        ));
    }
    let package_json = workspace
        .read_to_string(ctx, &cwd.join("package.json"))
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    if !package_json_declares_dependencies(&package_json) {
        return Ok(None);
    }
    if workspace
        .path_kind(ctx, &cwd.join("node_modules"))
        .await
        .is_err()
    {
        return Ok(Some("node_modules_missing".to_string()));
    }
    Ok(None)
}

fn package_json_declares_dependencies(package_json: &str) -> bool {
    serde_json::from_str::<Value>(package_json).is_ok_and(|value| {
        ["dependencies", "devDependencies", "optionalDependencies"]
            .iter()
            .any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_object)
                    .is_some_and(|dependencies| !dependencies.is_empty())
            })
    })
}

async fn snapshot_project_source(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    source_root: &Path,
    snapshot_relative: &str,
) -> Result<(), ToolError> {
    let snapshot_root = ctx.workspace_root.join(snapshot_relative);
    let _ = workspace.remove_dir_all(ctx, &snapshot_root).await;
    let skip_dir_names = source_snapshot_skip_dir_names();
    workspace
        .copy_dir_all(ctx, source_root, &snapshot_root, &skip_dir_names)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let manifest = json!({
        "sourceRoot": display_workspace_path(source_root, ctx),
        "snapshotRoot": format!("/workspace/{snapshot_relative}"),
        "createdAt": Utc::now().to_rfc3339(),
    });
    write_workspace_json(
        workspace,
        ctx,
        &format!("{snapshot_relative}/.snapshot.json"),
        &manifest,
    )
    .await?;
    Ok(())
}

fn source_snapshot_skip_dir_names() -> Vec<String> {
    ["node_modules", "dist", "out", ".next", ".astro", ".source"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

async fn promotion_gate_report_from_workspace(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    screenshot_id: Option<&str>,
) -> PromotionGateReport {
    let latest_build = read_workspace_json(workspace, ctx, "outputs/build/latest.json").await;
    let preview = read_workspace_json(workspace, ctx, "state/preview.json").await;
    let screenshot = match screenshot_id {
        Some(id) => {
            read_workspace_json(workspace, ctx, &format!("outputs/screenshots/{id}.json")).await
        }
        None => None,
    };

    PromotionGateReport {
        build_log_has_terminal_error: latest_build
            .as_ref()
            .map(|build| {
                build.get("status").and_then(Value::as_str) != Some("success")
                    || build.get("success").and_then(Value::as_bool) != Some(true)
            })
            .unwrap_or(true),
        preview_accessible: preview
            .as_ref()
            .and_then(|value| value.get("accessible"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_blank: screenshot
            .as_ref()
            .and_then(|value| value.get("blank"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        screenshot_available: screenshot.is_some(),
        blocking_findings: 0,
    }
}

async fn ensure_project_package_json(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
) -> Result<(), ToolError> {
    let package_json = project_dir.join("package.json");
    if workspace.path_kind(ctx, &package_json).await.is_ok() {
        return Ok(());
    }
    workspace
        .write_string(
            ctx,
            &package_json,
            &serde_json::to_string_pretty(&json!({
                "type": "module",
                "private": true,
                "dependencies": {}
            }))
            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))
}

async fn validate_project_source_contract(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
) -> Result<(), ToolError> {
    let state = read_workspace_json(workspace, ctx, "state/project.json").await;
    let template = state
        .as_ref()
        .and_then(|value| value.get("templateKey").or_else(|| value.get("template")))
        .and_then(Value::as_str);
    let package_json = workspace
        .read_to_string(ctx, &project_dir.join("package.json"))
        .await
        .unwrap_or_default();
    let is_fumadocs = template == Some("fumadocs-docs")
        || package_json.contains("\"fumadocs-ui\"")
        || package_json.contains("\"fumadocs-mdx\"");
    if !is_fumadocs {
        return Ok(());
    }
    validate_fumadocs_docs_contract(workspace, ctx, project_dir).await
}

async fn validate_fumadocs_docs_contract(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
) -> Result<(), ToolError> {
    let mut missing = Vec::new();
    let has_pages_root = workspace
        .path_kind(ctx, &project_dir.join("pages"))
        .await
        .is_ok();
    let has_src_pages_root = workspace
        .path_kind(ctx, &project_dir.join("src/pages"))
        .await
        .is_ok();
    if has_pages_root || has_src_pages_root {
        missing.push("project/pages and project/src/pages are forbidden for fumadocs-docs; keep routes under app/".to_string());
    }
    let source_config = read_contract_file(
        workspace,
        ctx,
        project_dir,
        "source.config.ts",
        &mut missing,
    )
    .await;
    if source_config
        .as_deref()
        .is_some_and(|text| !(text.contains("defineDocs") && text.contains("content/docs")))
    {
        missing.push("source.config.ts must define docs dir content/docs".to_string());
    }

    let mut source_loader =
        read_contract_file(workspace, ctx, project_dir, "lib/source.js", &mut missing).await;
    if source_loader.is_none() {
        missing.retain(|item| item != "missing lib/source.js");
        source_loader =
            read_contract_file(workspace, ctx, project_dir, "lib/source.ts", &mut missing).await;
    }
    if source_loader.as_deref().is_some_and(|text| {
        !(text.contains("baseUrl: '/docs'") && text.contains("toFumadocsSource()"))
    }) {
        missing.push("lib/source.js must load Fumadocs source at /docs".to_string());
    }

    let docs_layout = read_contract_file(
        workspace,
        ctx,
        project_dir,
        "app/docs/layout.jsx",
        &mut missing,
    )
    .await;
    if docs_layout
        .as_deref()
        .is_some_and(|text| !(text.contains("DocsLayout") && text.contains("source.pageTree")))
    {
        missing.push("app/docs/layout.jsx must render DocsLayout with source.pageTree".to_string());
    }

    let docs_page = read_contract_file(
        workspace,
        ctx,
        project_dir,
        "app/docs/[[...slug]]/page.jsx",
        &mut missing,
    )
    .await;
    if docs_page.as_deref().is_some_and(|text| {
        !(text.contains("generateStaticParams") && text.contains("source.getPage"))
    }) {
        missing.push(
            "app/docs/[[...slug]]/page.jsx must map slugs through source.getPage".to_string(),
        );
    }

    let home = read_contract_file(workspace, ctx, project_dir, "app/page.jsx", &mut missing).await;
    if home
        .as_deref()
        .is_some_and(|text| !text.contains("href=\"/docs\""))
    {
        missing.push("app/page.jsx must link the home route to /docs".to_string());
    }

    let index_mdx = read_contract_file(
        workspace,
        ctx,
        project_dir,
        "content/docs/index.mdx",
        &mut missing,
    )
    .await;
    if index_mdx
        .as_deref()
        .is_some_and(|text| !(text.trim_start().starts_with("---") && text.contains("\ntitle:")))
    {
        missing.push("content/docs/index.mdx must include frontmatter title".to_string());
    }

    let meta = read_contract_file(
        workspace,
        ctx,
        project_dir,
        "content/docs/meta.json",
        &mut missing,
    )
    .await;
    if let Some(meta) = meta {
        match serde_json::from_str::<Value>(&meta) {
            Ok(value) => {
                let pages = value.get("pages").and_then(Value::as_array);
                if !pages.is_some_and(|pages| {
                    !pages.is_empty() && pages.iter().any(|page| page.as_str() == Some("index"))
                }) {
                    missing.push("content/docs/meta.json must list index in pages".to_string());
                }
            }
            Err(_) => missing.push("content/docs/meta.json must be valid JSON".to_string()),
        }
    }

    if missing.is_empty() {
        return Ok(());
    }
    Err(typed_recoverable(
        format!("Docs source contract invalid: {}", missing.join(", ")),
        if missing.iter().any(|item| {
            item.contains("project/pages")
                && item.contains("project/src/pages")
                && item.contains("forbidden")
        }) {
            "docs.routing_root_forbidden"
        } else {
            "docs.source_contract_invalid"
        },
        json!({
            "missing": missing,
            "appRoot": display_workspace_path(project_dir, ctx),
            "suggestedAction": "Repair the fumadocs-docs app-router scaffold: keep routes under app/, docs content under content/docs, and do not create project/pages or project/src/pages."
        }),
    ))
}

async fn read_contract_file(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    project_dir: &Path,
    relative: &str,
    missing: &mut Vec<String>,
) -> Option<String> {
    match workspace
        .read_to_string(ctx, &project_dir.join(relative))
        .await
    {
        Ok(text) => Some(text),
        Err(_) => {
            missing.push(relative.to_string());
            None
        }
    }
}

async fn write_project_template_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
    template: &str,
) -> Result<(), ToolError> {
    if template == "fumadocs-docs" {
        return write_fumadocs_template_files(workspace, ctx, app_root).await;
    }

    let package = if template == "fumadocs-docs" {
        json!({
            "name": "anydesign-docs",
            "version": "0.0.0",
            "private": true,
            "type": "module",
            "scripts": {
                "build": "astro build",
                "preview": "astro preview --host 0.0.0.0"
            },
            "dependencies": {
                "astro": "^5.0.0",
                "tailwindcss": "^4.3.2"
            },
            "devDependencies": {}
        })
    } else {
        json!({
            "name": "anydesign-website",
            "version": "0.0.0",
            "private": true,
            "type": "module",
            "scripts": {
                "build": "astro build",
                "preview": "astro preview --host 0.0.0.0"
            },
            "dependencies": {
                "astro": "^5.0.0",
                "tailwindcss": "^4.3.2"
            },
            "devDependencies": {}
        })
    };
    let package_text = serde_json::to_string_pretty(&package)
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let lock = json!({
        "name": package.get("name").and_then(Value::as_str).unwrap_or("anydesign-app"),
        "version": "0.0.0",
        "lockfileVersion": 3,
        "requires": true,
        "packages": {
            "": {
                "name": package.get("name").and_then(Value::as_str).unwrap_or("anydesign-app"),
                "version": "0.0.0",
                "dependencies": package.get("dependencies").cloned().unwrap_or_else(|| json!({})),
                "devDependencies": package.get("devDependencies").cloned().unwrap_or_else(|| json!({}))
            }
        }
    });
    let files = vec![
        (app_root.join("package.json"), format!("{package_text}\n")),
        (
            app_root.join("package-lock.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&lock)
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?
            ),
        ),
        (
            app_root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\n\nexport default defineConfig({});\n"
                .to_string(),
        ),
        (
            app_root.join("tsconfig.json"),
            "{\n  \"extends\": \"astro/tsconfigs/strict\"\n}\n".to_string(),
        ),
        (
            app_root.join("src/pages/index.astro"),
            template_index_page(template),
        ),
        (
            app_root.join("src/styles/tokens.css"),
            runtime_website_tokens_css().to_string(),
        ),
        (
            app_root.join("src/styles/global.css"),
            runtime_website_global_css().to_string(),
        ),
        (
            app_root.join("src/components/ui/Button.astro"),
            runtime_website_button_component().to_string(),
        ),
    ];
    for (path, text) in files {
        workspace
            .write_string(ctx, &path, &text)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    }
    Ok(())
}

async fn cleanup_conflicting_template_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
    template: &str,
) -> Result<(), ToolError> {
    let (dirs, files): (&[&str], &[&str]) = if template == "fumadocs-docs" {
        (&["src"], &["astro.config.mjs"])
    } else {
        (
            &["app", "content", "lib", "components"],
            &[
                "next.config.mjs",
                "postcss.config.mjs",
                "source.config.ts",
                "mdx-components.jsx",
                "next-env.d.ts",
            ],
        )
    };

    for relative in dirs {
        remove_workspace_path_if_exists(workspace, ctx, &app_root.join(relative)).await?;
    }
    for relative in files {
        remove_workspace_path_if_exists(workspace, ctx, &app_root.join(relative)).await?;
    }
    Ok(())
}

async fn remove_workspace_path_if_exists(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), ToolError> {
    match workspace.path_kind(ctx, path).await {
        Ok(WorkspacePathKind::Dir) => workspace.remove_dir_all(ctx, path).await.map_err(|error| {
            ToolError::Recoverable(format!(
                "failed to remove stale template directory {}: {error}",
                display_workspace_path(path, ctx)
            ))
        }),
        Ok(WorkspacePathKind::File) => workspace.remove_file(ctx, path).await.map_err(|error| {
            ToolError::Recoverable(format!(
                "failed to remove stale template file {}: {error}",
                display_workspace_path(path, ctx)
            ))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ToolError::Recoverable(format!(
            "failed to inspect stale template path {}: {error}",
            display_workspace_path(path, ctx)
        ))),
    }
}

async fn verify_screenshot_artifact(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    screenshot_id: &str,
) -> Result<(), ToolError> {
    if screenshot_id.trim().is_empty()
        || screenshot_id.contains('/')
        || screenshot_id.contains('\\')
        || screenshot_id.contains("..")
    {
        return Err(typed_recoverable(
            "preview.report_candidate screenshotId must be a simple browser.screenshot artifact id"
                .to_string(),
            "preview.screenshot_invalid",
            json!({
                "screenshotId": screenshot_id,
                "suggestedAction": "Call browser.screenshot and pass its screenshotId."
            }),
        ));
    }
    let path = format!("outputs/screenshots/{screenshot_id}.json");
    let artifact = read_workspace_json(workspace, ctx, &path).await.ok_or_else(|| {
        typed_recoverable(
            format!(
                "preview.report_candidate requires existing screenshot artifact {screenshot_id}; call browser.screenshot first"
            ),
            "preview.screenshot_missing",
            json!({
                "screenshotId": screenshot_id,
                "expectedPath": format!("/workspace/{path}"),
                "suggestedAction": "Call browser.screenshot before preview.report_candidate."
            }),
        )
    })?;
    if artifact
        .get("blank")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        return Err(typed_recoverable(
            format!("preview.report_candidate rejected blank screenshot artifact {screenshot_id}"),
            "preview.screenshot_blank",
            json!({
                "screenshotId": screenshot_id,
                "path": format!("/workspace/{path}"),
                "suggestedAction": "Fix the preview and capture a non-blank screenshot."
            }),
        ));
    }
    Ok(())
}

async fn write_fumadocs_template_files(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    app_root: &Path,
) -> Result<(), ToolError> {
    let package = json!({
        "name": "anydesign-docs",
        "version": "0.0.0",
        "private": true,
        "type": "module",
        "packageManager": "npm@10.9.0",
        "scripts": {
            "build": "next build --webpack",
            "dev": "next dev --hostname 0.0.0.0",
            "preview": "serve out --listen 3000"
        },
        "dependencies": {
            "fumadocs-core": "^16.10.7",
            "fumadocs-mdx": "^15.0.13",
            "fumadocs-ui": "^16.10.7",
            "next": "^16.2.10",
            "react": "^19.2.7",
            "react-dom": "^19.2.7",
            "serve": "^14.2.5"
        },
        "devDependencies": {
            "@tailwindcss/postcss": "^4.3.2",
            "@types/mdx": "latest",
            "@types/node": "latest",
            "@types/react": "latest",
            "postcss": "^8.5.6",
            "tailwindcss": "^4.3.2",
            "typescript": "5.9.3"
        }
    });
    let package_text = serde_json::to_string_pretty(&package)
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    let lock = json!({
        "name": "anydesign-docs",
        "version": "0.0.0",
        "lockfileVersion": 3,
        "requires": true,
        "packages": {
            "": {
                "name": "anydesign-docs",
                "version": "0.0.0",
                "dependencies": package.get("dependencies").cloned().unwrap_or_else(|| json!({})),
                "devDependencies": package.get("devDependencies").cloned().unwrap_or_else(|| json!({}))
            }
        }
    });
    let files = vec![
        (app_root.join("package.json"), format!("{package_text}\n")),
        (
            app_root.join("package-lock.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&lock)
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?
            ),
        ),
        (
            app_root.join("postcss.config.mjs"),
            "const config = {\n  plugins: {\n    '@tailwindcss/postcss': {},\n  },\n};\n\nexport default config;\n".to_string(),
        ),
        (
            app_root.join("next.config.mjs"),
            "import { createMDX } from 'fumadocs-mdx/next';\n\n/** @type {import('next').NextConfig} */\nconst nextConfig = {\n  output: 'export',\n  reactStrictMode: true,\n};\n\nconst withMDX = createMDX();\n\nexport default withMDX(nextConfig);\n".to_string(),
        ),
        (
            app_root.join("tsconfig.json"),
            "{\n  \"compilerOptions\": {\n    \"target\": \"ES2017\",\n    \"lib\": [\"dom\", \"dom.iterable\", \"esnext\"],\n    \"allowJs\": true,\n    \"skipLibCheck\": true,\n    \"strict\": false,\n    \"noEmit\": true,\n    \"esModuleInterop\": true,\n    \"module\": \"esnext\",\n    \"moduleResolution\": \"bundler\",\n    \"resolveJsonModule\": true,\n    \"isolatedModules\": true,\n    \"jsx\": \"preserve\",\n    \"incremental\": true,\n    \"plugins\": [{ \"name\": \"next\" }]\n  },\n  \"include\": [\"next-env.d.ts\", \"**/*.ts\", \"**/*.tsx\", \"**/*.js\", \"**/*.jsx\", \".next/types/**/*.ts\", \".source/**/*.ts\"],\n  \"exclude\": [\"node_modules\"]\n}\n".to_string(),
        ),
        (
            app_root.join("next-env.d.ts"),
            "/// <reference types=\"next\" />\n/// <reference types=\"next/image-types/global\" />\n\n// This file is generated by the runtime template.\n".to_string(),
        ),
        (
            app_root.join("source.config.ts"),
            "import { defineDocs, defineConfig } from 'fumadocs-mdx/config';\n\nexport const docs = defineDocs({\n  dir: 'content/docs',\n});\n\nexport default defineConfig();\n".to_string(),
        ),
        (
            app_root.join("lib/source.js"),
            "import { docs } from '../.source/server';\nimport { loader } from 'fumadocs-core/source';\n\nexport const source = loader({\n  baseUrl: '/docs',\n  source: docs.toFumadocsSource(),\n});\n".to_string(),
        ),
        (
            app_root.join("lib/layout.shared.jsx"),
            "export function baseOptions() {\n  return {\n    nav: {\n      title: 'AnyDesign Runtime Docs',\n    },\n  };\n}\n".to_string(),
        ),
        (
            app_root.join("components/mdx.jsx"),
            "import defaultMdxComponents from 'fumadocs-ui/mdx';\n\nexport function getMDXComponents(components = {}) {\n  return {\n    ...defaultMdxComponents,\n    ...components,\n  };\n}\n\nexport const useMDXComponents = getMDXComponents;\n".to_string(),
        ),
        (
            app_root.join("mdx-components.jsx"),
            "export { useMDXComponents } from './components/mdx';\n".to_string(),
        ),
        (
            app_root.join("app/global.css"),
            runtime_fumadocs_global_css().to_string(),
        ),
        (
            app_root.join("app/tokens.css"),
            runtime_fumadocs_tokens_css().to_string(),
        ),
        (
            app_root.join("components/ui/button.jsx"),
            runtime_fumadocs_button_component().to_string(),
        ),
        (
            app_root.join("app/layout.jsx"),
            "import './global.css';\nimport { RootProvider } from 'fumadocs-ui/provider/next';\n\nexport default function RootLayout({ children }) {\n  return (\n    <html lang=\"en\" suppressHydrationWarning>\n      <body className=\"flex min-h-screen flex-col\">\n        <RootProvider>{children}</RootProvider>\n      </body>\n    </html>\n  );\n}\n".to_string(),
        ),
        (
            app_root.join("app/page.jsx"),
            "export default function Home() {\n  return (\n    <main>\n      <h1>AnyDesign Runtime Docs</h1>\n      <a href=\"/docs\">Open docs</a>\n    </main>\n  );\n}\n".to_string(),
        ),
        (
            app_root.join("app/docs/layout.jsx"),
            "import { source } from '../../lib/source';\nimport { baseOptions } from '../../lib/layout.shared';\nimport { DocsLayout } from 'fumadocs-ui/layouts/docs';\n\nexport default function Layout({ children }) {\n  return (\n    <DocsLayout tree={source.pageTree} {...baseOptions()}>\n      {children}\n    </DocsLayout>\n  );\n}\n".to_string(),
        ),
        (
            app_root.join("app/docs/[[...slug]]/page.jsx"),
            "import { notFound } from 'next/navigation';\nimport { source } from '../../../lib/source';\nimport { getMDXComponents } from '../../../components/mdx';\nimport { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/layouts/docs/page';\n\nexport function generateStaticParams() {\n  return source.generateParams();\n}\n\nexport async function generateMetadata({ params }) {\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) return { title: 'AnyDesign Runtime Docs' };\n  return { title: page.data.title, description: page.data.description };\n}\n\nexport default async function Page({ params }) {\n  const resolved = await params;\n  const page = source.getPage(resolved.slug);\n  if (!page) notFound();\n  const MDXContent = page.data.body;\n  return (\n    <DocsPage toc={page.data.toc} full={page.data.full}>\n      <DocsTitle>{page.data.title}</DocsTitle>\n      <DocsDescription>{page.data.description}</DocsDescription>\n      <DocsBody>\n        <MDXContent components={getMDXComponents()} />\n      </DocsBody>\n    </DocsPage>\n  );\n}\n".to_string(),
        ),
        (
            app_root.join("content/docs/index.mdx"),
            "---\ntitle: Overview\ndescription: Runtime generated documentation overview\n---\n\n# Overview\n\nThis documentation project was initialized by the runtime lifecycle.\n".to_string(),
        ),
        (
            app_root.join("content/docs/runtime-flow.mdx"),
            "---\ntitle: Runtime Flow\n---\n\n# Runtime Flow\n\nCreate, generate, build, edit, and promote previews through the runtime API.\n".to_string(),
        ),
        (
            app_root.join("content/docs/meta.json"),
            serde_json::to_string_pretty(&json!({
                "title": "AnyDesign Runtime Docs",
                "pages": ["index", "runtime-flow"]
            }))
            .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        ),
    ];
    for (path, text) in files {
        workspace
            .write_string(ctx, &path, &text)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    }
    Ok(())
}

fn template_index_page(template: &str) -> String {
    if template == "fumadocs-docs" {
        return r#"---
const title = 'AnyDesign Docs';
---
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width" />
    <title>{title}</title>
  </head>
  <body>
    <main>
      <h1>{title}</h1>
      <p>Runtime generated documentation site.</p>
    </main>
  </body>
</html>
"#
        .to_string();
    }
    r#"---
import '../styles/global.css';
const title = 'AnyDesign Website';
---
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width" />
    <title>{title}</title>
  </head>
  <body class="runtime-page">
    <main class="runtime-shell">
      <section class="runtime-hero" aria-labelledby="page-title">
        <p class="runtime-kicker">astro-website</p>
        <h1 id="page-title">{title}</h1>
        <p class="runtime-lede">Runtime generated website.</p>
      </section>
    </main>
  </body>
</html>
"#
    .to_string()
}

fn runtime_style_contract(template: &str, app_root_relative: &Path) -> Value {
    let app_root = format!(
        "/workspace/{}",
        app_root_relative.to_string_lossy().replace('\\', "/")
    );
    let (token_file, global_css_file, component_root) = if template == "fumadocs-docs" {
        (
            format!("{app_root}/app/tokens.css"),
            format!("{app_root}/app/global.css"),
            format!("{app_root}/components/ui"),
        )
    } else {
        (
            format!("{app_root}/src/styles/tokens.css"),
            format!("{app_root}/src/styles/global.css"),
            format!("{app_root}/src/components/ui"),
        )
    };
    json!({
        "version": "runtime-style-contract@p2",
        "template": template,
        "tokenFile": token_file,
        "globalCssFile": global_css_file,
        "componentRoot": component_root,
        "tailwind": {
            "version": "4",
            "entryImport": "@import \"tailwindcss\"",
            "themeSource": "css-variables"
        },
        "tokens": {
            "color.background": "--runtime-bg",
            "color.surface": "--runtime-surface",
            "color.surfaceStrong": "--runtime-surface-strong",
            "color.text": "--runtime-text",
            "color.muted": "--runtime-muted",
            "color.primary": "--runtime-primary",
            "color.primaryContrast": "--runtime-primary-contrast",
            "color.border": "--runtime-border",
            "radius.card": "--runtime-radius-card",
            "radius.control": "--runtime-radius-control",
            "font.sans": "--runtime-font-sans",
            "shadow.soft": "--runtime-shadow-soft"
        },
        "editableTokens": [
            "color.background",
            "color.surface",
            "color.surfaceStrong",
            "color.text",
            "color.muted",
            "color.primary",
            "color.primaryContrast",
            "color.border",
            "radius.card",
            "radius.control",
            "font.sans",
            "shadow.soft"
        ]
    })
}

fn runtime_website_tokens_css() -> &'static str {
    r#":root {
  --runtime-bg: #f7f8fb;
  --runtime-surface: #ffffff;
  --runtime-surface-strong: #eef3fb;
  --runtime-text: #17202f;
  --runtime-muted: #526173;
  --runtime-primary: #2563eb;
  --runtime-primary-contrast: #ffffff;
  --runtime-border: #d7deea;
  --runtime-radius-card: 8px;
  --runtime-radius-control: 8px;
  --runtime-font-sans: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --runtime-shadow-soft: 0 18px 48px rgba(23, 32, 47, 0.12);
}
"#
}

fn runtime_website_global_css() -> &'static str {
    r#"@import "tailwindcss";
@import "./tokens.css";

@theme {
  --color-runtime-bg: var(--runtime-bg);
  --color-runtime-surface: var(--runtime-surface);
  --color-runtime-primary: var(--runtime-primary);
  --color-runtime-text: var(--runtime-text);
  --font-runtime-sans: var(--runtime-font-sans);
}

* {
  box-sizing: border-box;
}

html {
  background: var(--runtime-bg);
  color: var(--runtime-text);
  font-family: var(--runtime-font-sans);
}

body {
  margin: 0;
  min-height: 100vh;
  background:
    radial-gradient(circle at 20% 0%, color-mix(in srgb, var(--runtime-primary) 18%, transparent), transparent 32rem),
    linear-gradient(180deg, var(--runtime-bg), var(--runtime-surface-strong));
  color: var(--runtime-text);
}

a {
  color: inherit;
}

.runtime-page {
  min-height: 100vh;
}

.runtime-shell {
  width: min(1120px, calc(100% - 32px));
  margin: 0 auto;
  padding: 64px 0;
}

.runtime-hero,
.runtime-section {
  border-top: 1px solid var(--runtime-border);
}

.runtime-hero {
  display: grid;
  gap: 16px;
  padding: 40px 0 52px;
}

.runtime-kicker {
  margin: 0;
  color: var(--runtime-primary);
  font-size: 13px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.runtime-hero h1 {
  margin: 0;
  max-width: 780px;
  font-size: clamp(38px, 6vw, 76px);
  line-height: 0.95;
  letter-spacing: 0;
}

.runtime-lede,
.runtime-section p {
  margin: 0;
  max-width: 720px;
  color: var(--runtime-muted);
  font-size: 16px;
  line-height: 1.65;
}

.runtime-sections {
  display: grid;
  gap: 18px;
}

.runtime-section {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(180px, 320px);
  gap: 24px;
  align-items: stretch;
  padding: 24px 0;
}

.runtime-section h2 {
  margin: 0 0 10px;
  font-size: 26px;
  letter-spacing: 0;
}

.runtime-visual {
  min-height: 120px;
  border: 1px solid var(--runtime-border);
  border-radius: var(--runtime-radius-card);
  background: var(--runtime-surface);
  box-shadow: var(--runtime-shadow-soft);
  padding: 18px;
  color: var(--runtime-muted);
  font-size: 14px;
  line-height: 1.5;
}

.runtime-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 40px;
  border: 1px solid var(--runtime-primary);
  border-radius: var(--runtime-radius-control);
  background: var(--runtime-primary);
  color: var(--runtime-primary-contrast);
  font-weight: 700;
  text-decoration: none;
}

@media (max-width: 760px) {
  .runtime-shell {
    width: min(100% - 24px, 1120px);
    padding: 36px 0;
  }

  .runtime-section {
    grid-template-columns: 1fr;
  }
}
"#
}

fn runtime_website_button_component() -> &'static str {
    r##"---
const { href = "#", label = "Open" } = Astro.props;
---
<a class="runtime-button" href={href}>{label}</a>
"##
}

fn runtime_fumadocs_tokens_css() -> &'static str {
    r#":root {
  --runtime-bg: #f8fafc;
  --runtime-surface: #ffffff;
  --runtime-surface-strong: #eef2ff;
  --runtime-text: #111827;
  --runtime-muted: #5b6472;
  --runtime-primary: #2563eb;
  --runtime-primary-contrast: #ffffff;
  --runtime-border: #d8dee9;
  --runtime-radius-card: 8px;
  --runtime-radius-control: 8px;
  --runtime-font-sans: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --runtime-shadow-soft: 0 16px 44px rgba(17, 24, 39, 0.10);
}
"#
}

fn runtime_fumadocs_global_css() -> &'static str {
    r#"@import 'tailwindcss';
@import './tokens.css';
@import 'fumadocs-ui/css/neutral.css';
@import 'fumadocs-ui/css/preset.css';

body {
  background: var(--runtime-bg);
  color: var(--runtime-text);
  font-family: var(--runtime-font-sans);
}

:root {
  --fd-primary: var(--runtime-primary);
  --fd-background: var(--runtime-bg);
  --fd-foreground: var(--runtime-text);
  --fd-muted-foreground: var(--runtime-muted);
  --fd-border: var(--runtime-border);
}

.runtime-button {
  border-radius: var(--runtime-radius-control);
  background: var(--runtime-primary);
  color: var(--runtime-primary-contrast);
}
"#
}

fn runtime_fumadocs_button_component() -> &'static str {
    r#"export function Button({ children, className = '', ...props }) {
  return (
    <button className={`runtime-button px-4 py-2 font-semibold ${className}`} {...props}>
      {children}
    </button>
  );
}
"#
}

fn project_page_relative_path(route: &str) -> Result<PathBuf, String> {
    let trimmed = route.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return Err("route must start with /".to_string());
    }
    if trimmed.contains('\\') || trimmed.contains("..") || trimmed.contains("//") {
        return Err("route must stay within src/pages".to_string());
    }
    let without_slash = trimmed.trim_matches('/');
    if without_slash.is_empty() {
        return Ok(PathBuf::from("index.astro"));
    }
    let mut path = PathBuf::new();
    for part in without_slash.split('/') {
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err(
                "route segments may only contain ASCII letters, numbers, '-' or '_'".to_string(),
            );
        }
        path.push(part);
    }
    path.set_extension("astro");
    Ok(path)
}

fn render_project_page(
    route: &str,
    title: &str,
    style_profile: &str,
    sections: &[Value],
    relative_page_path: &Path,
) -> String {
    let escaped_title = html_escape(title);
    let global_css_import = project_page_global_css_import(relative_page_path);
    let rendered_sections = sections
        .iter()
        .enumerate()
        .map(|(index, section)| render_project_page_section(index, section))
        .collect::<Vec<_>>()
        .join("\n\n");
    let style_class = match style_profile {
        "saas" | "enterprise" | "docs" => style_profile,
        _ => "saas",
    };
    format!(
        r#"---
import '{global_css_import}';
const title = '{title_js}';
---
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{{title}}</title>
  </head>
  <body class="runtime-page {style_class}">
    <main class="runtime-shell">
      <header class="runtime-hero">
        <div class="runtime-kicker">{route}</div>
        <h1>{escaped_title}</h1>
      </header>
      <div class="runtime-sections">
{rendered_sections}
      </div>
    </main>
  </body>
</html>
"#,
        title_js = js_string_escape(title),
        global_css_import = global_css_import,
        route = html_escape(route),
    )
}

fn project_page_global_css_import(relative_page_path: &Path) -> String {
    let parent_depth = relative_page_path
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter(|component| matches!(component, Component::Normal(_)))
                .count()
        })
        .unwrap_or(0);
    format!("{}styles/global.css", "../".repeat(parent_depth + 1))
}

fn validate_style_token_value(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("token values must be non-empty".to_string());
    }
    if trimmed.len() > 256 {
        return Err("token values must be 256 characters or fewer".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, ';' | '{' | '}' | '\n' | '\r'))
    {
        return Err("token values may not contain ';', braces, or newlines".to_string());
    }
    Ok(())
}

fn replace_css_variable_value(
    content: &str,
    css_variable: &str,
    new_value: &str,
    ctx: &ToolContext,
    token_path: &Path,
) -> Result<(String, String), ToolError> {
    let marker = format!("{css_variable}:");
    let count = content.matches(&marker).count();
    if count == 0 {
        return Err(style_typed_recoverable(
            format!("style.update_tokens could not find CSS variable {css_variable} in token file"),
            "style.token_variable_missing",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "suggestedAction": "Repair the token CSS file or regenerate it from the runtime template before retrying style.update_tokens."
            }),
        ));
    }
    if count > 1 {
        return Err(style_typed_recoverable(
            format!("style.update_tokens found CSS variable {css_variable} multiple times in token file"),
            "style.token_variable_ambiguous",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "matchCount": count,
                "suggestedAction": "Keep one canonical CSS variable declaration in the runtime token file before retrying."
            }),
        ));
    }
    let start = content.find(&marker).expect("count checked above");
    let value_start = start + marker.len();
    let semicolon_offset = content[value_start..].find(';').ok_or_else(|| {
        style_typed_recoverable(
            format!("style.update_tokens CSS variable {css_variable} is missing a semicolon"),
            "style.token_file_invalid",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "suggestedAction": "Fix the CSS variable declaration so it ends with a semicolon, then retry."
            }),
        )
    })?;
    let value_end = value_start + semicolon_offset;
    let old_value = content[value_start..value_end].trim().to_string();
    let updated = format!(
        "{} {}{}",
        &content[..value_start],
        new_value.trim(),
        &content[value_end..]
    );
    Ok((updated, old_value))
}

fn render_project_page_section(index: usize, section: &Value) -> String {
    let kind = section
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("section");
    let heading = section
        .get("heading")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Section {}", index + 1));
    let body = section.get("body").and_then(Value::as_str).unwrap_or("");
    let visual = section
        .get("visual")
        .and_then(Value::as_str)
        .unwrap_or(kind);
    format!(
        r#"        <section class="runtime-section" data-kind="{kind}">
          <div>
            <h2>{heading}</h2>
            <p>{body}</p>
          </div>
          <aside class="runtime-visual">{visual}</aside>
        </section>"#,
        kind = html_escape(kind),
        heading = html_escape(&heading),
        body = html_escape(body),
        visual = html_escape(visual),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn js_string_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

async fn write_package_install_log(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    tool_use_id: &str,
    args: &[String],
    output: &SandboxCommandOutput,
) -> Result<String, ToolError> {
    let text = format!(
        "$ {}\n\nstatus: {:?}\n\nstdout:\n{}\n\nstderr:\n{}\n",
        args.join(" "),
        output.status,
        output.stdout,
        output.stderr
    );
    let log_path = format!("outputs/build/package-install-{tool_use_id}.log");
    let path = ctx.workspace_root.join(&log_path);
    workspace
        .write_string(ctx, &path, &text)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    workspace
        .write_string(
            ctx,
            &ctx.workspace_root
                .join("outputs/build/package-install-latest.log"),
            &text,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    Ok(format!("/workspace/{log_path}"))
}

fn has_terminal_error(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    ["error:", "failed", "panic", "exception"]
        .iter()
        .any(|needle| lowered.contains(needle))
}

fn argv_from_input(input: &Value) -> Result<Vec<String>, ToolError> {
    input
        .get("argv")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .ok_or_else(|| ToolError::Recoverable("shell.run requires argv".to_string()))
}

fn package_specs_from_input(input: &Value) -> Vec<String> {
    input
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn package_install_mode_from_input(input: &Value) -> Result<String, ValidationError> {
    if let Some(mode) = input.get("mode").and_then(Value::as_str) {
        if matches!(mode, "restore" | "add") {
            return Ok(mode.to_string());
        }
        return Err(ValidationError::new(
            "package.install mode must be restore or add",
        ));
    }
    if package_specs_from_input(input).is_empty() {
        Ok("restore".to_string())
    } else {
        Ok("add".to_string())
    }
}

fn validate_package_manager(package_manager: &str) -> Result<(), ValidationError> {
    if matches!(package_manager, "npm" | "pnpm") {
        Ok(())
    } else {
        Err(ValidationError::new(
            "package.install packageManager must be npm or pnpm",
        ))
    }
}

fn package_manager_from_input_or_project(
    input: &Value,
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<String, ToolError> {
    if let Some(package_manager) = input.get("packageManager").and_then(Value::as_str) {
        validate_package_manager(package_manager)
            .map_err(|error| ToolError::Recoverable(error.message))?;
        return Ok(package_manager.to_string());
    }
    if let Some(package_manager) = project_state_string(ctx, "packageManager") {
        validate_package_manager(&package_manager)
            .map_err(|error| ToolError::Recoverable(error.message))?;
        return Ok(package_manager);
    }
    if cwd.join("pnpm-lock.yaml").exists() {
        return Ok("pnpm".to_string());
    }
    if cwd.join("package-lock.json").exists() {
        return Ok("npm".to_string());
    }
    Ok("npm".to_string())
}

fn project_state_string(ctx: &ToolContext, key: &str) -> Option<String> {
    fs::read_to_string(ctx.workspace_root.join("state/project.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string))
}

fn package_install_argv(
    package_manager: &str,
    mode: &str,
    packages: &[String],
    registry: &str,
) -> Vec<String> {
    let mut argv = match (package_manager, mode) {
        ("npm", "restore") | ("npm", "add") => vec![
            "npm".to_string(),
            "install".to_string(),
            "--ignore-scripts".to_string(),
            "--audit=false".to_string(),
            "--fund=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        ("pnpm", "restore") => vec![
            "pnpm".to_string(),
            "install".to_string(),
            "--ignore-scripts".to_string(),
            "--config.audit=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        ("pnpm", "add") => vec![
            "pnpm".to_string(),
            "add".to_string(),
            "--ignore-scripts".to_string(),
            "--config.audit=false".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ],
        _ => vec![package_manager.to_string()],
    };
    if mode == "add" {
        argv.extend(packages.iter().cloned());
    }
    argv
}

fn project_build_argv(package_manager: &str) -> Vec<String> {
    match package_manager {
        "pnpm" => vec!["pnpm".to_string(), "run".to_string(), "build".to_string()],
        _ => vec!["npm".to_string(), "run".to_string(), "build".to_string()],
    }
}

fn is_public_registry(registry: &str) -> bool {
    registry.contains("registry.npmjs.org") || !registry.contains("internal")
}

fn is_internal_preview_url(url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return false;
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local")
}

fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next()?
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn url_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.starts_with('[') {
        let after_bracket = host_port.split_once(']')?.1;
        return after_bracket
            .strip_prefix(':')
            .and_then(|port| port.parse().ok())
            .or_else(|| default_port_for_scheme(scheme));
    }
    let mut parts = host_port.rsplitn(2, ':');
    let maybe_port = parts.next()?;
    if parts.next().is_some() {
        maybe_port.parse().ok()
    } else {
        default_port_for_scheme(scheme)
    }
}

fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

async fn verify_preview_accessible(url: &str) -> Result<(), ToolError> {
    let host = url_host(url)
        .ok_or_else(|| ToolError::Recoverable(format!("preview.start invalid url: {url}")))?;
    let port = url_port(url)
        .ok_or_else(|| ToolError::Recoverable(format!("preview.start missing port: {url}")))?;
    time::timeout(
        Duration::from_millis(750),
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| ToolError::Recoverable(format!("preview.start timed out connecting to {url}")))?
    .map_err(|error| {
        ToolError::Recoverable(format!("preview.start could not reach {url}: {error}"))
    })?;
    Ok(())
}
