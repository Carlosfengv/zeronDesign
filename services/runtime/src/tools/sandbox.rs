use crate::{
    permission::{
        check_command_policy, check_create_path, check_existing_path, PermissionReason,
        PermissionResult, RuleSource,
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
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpStream, process::Command, time};
use tokio_tungstenite::{connect_async, tungstenite::Message};

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
        Arc::new(FsPatchTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(FsDeleteTool {
            workspace: workspace_backend.clone(),
        }),
        Arc::new(ShellRunTool {
            command: command_backend.clone(),
        }),
        Arc::new(PackageInstallTool {
            workspace: workspace_backend.clone(),
            command: command_backend,
        }),
        Arc::new(PreviewRebuildingTool),
        Arc::new(PreviewReportCandidateTool {
            workspace: workspace_backend.clone(),
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
}

#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    async fn read_to_string(&self, ctx: &ToolContext, path: &Path) -> io::Result<String>;
    async fn write_string(&self, ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()>;
    async fn list_dir(&self, ctx: &ToolContext, path: &Path) -> io::Result<Vec<WorkspaceEntry>>;
    async fn path_kind(&self, ctx: &ToolContext, path: &Path) -> io::Result<WorkspacePathKind>;
    async fn remove_file(&self, ctx: &ToolContext, path: &Path) -> io::Result<()>;
    async fn remove_dir_all(&self, ctx: &ToolContext, path: &Path) -> io::Result<()>;
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

#[derive(Debug, Clone, Default)]
pub struct LocalWorkspaceBackend;

#[async_trait]
impl WorkspaceBackend for LocalWorkspaceBackend {
    async fn read_to_string(&self, _ctx: &ToolContext, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    async fn write_string(&self, _ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        fs::write(path, text)
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
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
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
                ToolError::Recoverable(format!("failed to read {}: {error}", path.display()))
            })?;
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
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
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
                ToolError::Recoverable(format!("failed to list {}: {error}", path.display()))
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
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "query", self.name())?;
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
                "text": string_schema("File contents")
            }),
            &["path", "text"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "text", self.name())?;
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
                "newStr": string_schema("Replacement text")
            }),
            &["path", "oldStr", "newStr"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "path", self.name())?;
        require_string(&input, "oldStr", self.name())?;
        require_string(&input, "newStr", self.name())?;
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
        let old_str = required_str(&input, "oldStr")?;
        let new_str = required_str(&input, "newStr")?;
        let content = self
            .workspace
            .read_to_string(&ctx, &path)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err(ToolError::Recoverable(
                "oldStr not found in file".to_string(),
            ));
        }
        if count > 1 {
            return Err(ToolError::Recoverable(
                "oldStr found multiple times, provide more context".to_string(),
            ));
        }
        let new_content = content.replacen(old_str, new_str, 1);
        self.workspace
            .write_string(&ctx, &path, &new_content)
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        Ok(ToolResult::ok(
            json!({ "path": display_workspace_path(&path, &ctx), "patched": true }),
        ))
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

#[async_trait]
impl SandboxCommandBackend for LocalCommandBackend {
    async fn run(
        &self,
        _ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput> {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).current_dir(cwd);
        let output = time::timeout(Duration::from_millis(timeout_ms), command.output())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "shell.run timed out"))??;
        Ok(SandboxCommandOutput {
            status: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
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
            .run(&ctx, &argv, &cwd, timeout_ms)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    ToolError::Recoverable("shell.run timed out".to_string())
                } else {
                    ToolError::Recoverable(format!("shell.run failed to start: {error}"))
                }
            })?;
        if !output.success {
            return Err(ToolError::Recoverable(format!(
                "shell.run exited with status {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status, output.stdout, output.stderr
            )));
        }
        Ok(ToolResult::ok(json!({
            "status": output.status,
            "success": output.success,
            "stdout": output.stdout,
            "stderr": output.stderr,
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
                "registry": string_schema("Internal registry URL")
            }),
            &["packages"],
        )
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        let packages = input
            .get("packages")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError::new("package.install requires packages"))?;
        if packages.is_empty() || !packages.iter().all(|item| item.as_str().is_some()) {
            return Err(ValidationError::new(
                "package.install packages must be a non-empty string array",
            ));
        }
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, ctx: &ToolContext) -> PermissionResult {
        let registry = input.get("registry").and_then(Value::as_str);
        let packages = package_specs_from_input(input);
        if registry.is_none_or(is_public_registry)
            || packages
                .iter()
                .any(|package| package.starts_with("http://") || package.starts_with("https://"))
        {
            return PermissionResult::Ask {
                message: "Installing from public registry requires approval".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "package.install public registry".to_string(),
                },
                suggestions: None,
            };
        }
        for package in &packages {
            if let Some(local_path) = package.strip_prefix("file:") {
                let resolved = resolve_path(local_path, &default_project_dir(ctx));
                if let Err(error) = check_existing_path(&resolved, &ctx.workspace_root) {
                    return deny(self.name(), format!("{error:?}"));
                }
            }
        }
        allow_with_input(input, "internal registry package install allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let packages = package_specs_from_input(&input);
        let registry = input
            .get("registry")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Recoverable("package.install requires registry".to_string()))?
            .to_string();
        let cwd = default_project_dir(&ctx);
        ensure_project_package_json(&*self.workspace, &ctx, &cwd).await?;

        let mut args = vec![
            "install".to_string(),
            "--ignore-scripts".to_string(),
            "--package-lock=false".to_string(),
            "--audit=false".to_string(),
            "--fund=false".to_string(),
            "--registry".to_string(),
            registry.clone(),
        ];
        args.extend(packages.iter().cloned());
        let timeout_ms = input
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000);
        let argv = std::iter::once("npm".to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        let output = self
            .command
            .run(&ctx, &argv, &cwd, timeout_ms)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    ToolError::Recoverable("package.install timed out".to_string())
                } else {
                    ToolError::Recoverable(format!("package.install failed to start npm: {error}"))
                }
            })?;
        let log_path = write_package_install_log(&*self.workspace, &ctx, &argv, &output).await?;
        if !output.success {
            return Err(ToolError::Recoverable(format!(
                "package.install failed with status {:?}; log: {log_path}",
                output.status
            )));
        }

        Ok(ToolResult::ok(json!({
            "installed": packages,
            "registry": registry,
            "manager": "npm",
            "command": argv,
            "status": output.status,
            "success": true,
            "logPath": log_path,
            "stdout": output.stdout,
            "stderr": output.stderr,
        })))
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
            .map(str::to_string);
        let source_snapshot_uri = input
            .get("sourceSnapshotUri")
            .and_then(Value::as_str)
            .map(str::to_string);
        let candidate = ctx
            .store
            .create_project_version_candidate(
                &ctx.project_id,
                &ctx.run.id,
                url.clone(),
                screenshot_id.clone(),
                source_snapshot_uri,
            )
            .await;
        ctx.store
            .append_event(AgentEvent::PreviewCandidate {
                run_id: ctx.run.id.clone(),
                url,
                version_id: candidate.id.clone(),
                screenshot_id: screenshot_id.clone(),
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
            promotion_gate_report_from_workspace(&*self.workspace, &ctx, screenshot_id.as_deref())
                .await;
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
        Ok(ToolResult::ok(json!({
            "versionId": promoted.id,
            "reviewRunId": review_run.id.clone(),
            "status": promoted.status,
            "url": promoted.preview_url,
        })))
    }
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
                "command": string_schema("Preview command label")
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
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("http://127.0.0.1:4321")
            .to_string();
        verify_preview_accessible(&url).await?;
        let state = json!({
            "status": "running",
            "url": url,
            "port": input.get("port").and_then(Value::as_u64).unwrap_or(4321),
            "command": input.get("command").and_then(Value::as_str).unwrap_or("preview"),
            "accessible": true,
        });
        write_workspace_json(&*self.workspace, &ctx, "state/preview.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
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
        let mut state = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| json!({ "url": Value::Null }));
        state["status"] = json!("stopped");
        state["accessible"] = json!(false);
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

fn require_string(input: &Value, key: &str, tool: &str) -> Result<(), ValidationError> {
    if input.get(key).and_then(Value::as_str).is_some() {
        return Ok(());
    }
    Err(ValidationError::new(format!("{tool} requires {key}")))
}

fn resolve_path(path: &str, workspace_root: &Path) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn checked_existing_path(input: &Value, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let path = required_str(input, "path")?;
    check_existing_path(
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
    let project_root = ctx.workspace_root.join("project");
    let project_root = fs::canonicalize(&project_root).map_err(|error| error.to_string())?;
    if path == ctx.workspace_root
        || path == project_root
        || path == ctx.workspace_root.join("inputs")
        || path == ctx.workspace_root.join("state")
        || path == ctx.workspace_root.join("outputs")
        || !path.starts_with(&project_root)
    {
        return Err("fs.delete is limited to non-root paths under /workspace/project".to_string());
    }
    Ok(path)
}

fn check_existing_path_permission(
    input: &Value,
    ctx: &ToolContext,
    tool: &str,
) -> PermissionResult {
    match input.get("path").and_then(Value::as_str).map(|path| {
        check_existing_path(
            &resolve_path(path, &ctx.workspace_root),
            &ctx.workspace_root,
        )
    }) {
        Some(Ok(_)) => allow_with_input(input, "workspace path allowed"),
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
        Ok(_) => allow_with_input(input, "workspace write path allowed"),
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
    let project = ctx.workspace_root.join("project");
    if project.exists() {
        project
    } else {
        ctx.workspace_root.clone()
    }
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
    workspace
        .write_string(
            ctx,
            &path,
            &serde_json::to_string_pretty(value)
                .map_err(|error| ToolError::Recoverable(error.to_string()))?,
        )
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))
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

async fn promotion_gate_report_from_workspace(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    screenshot_id: Option<&str>,
) -> PromotionGateReport {
    let build_log_path = ctx.workspace_root.join("outputs/build/build.log");
    let build_log = workspace.read_to_string(ctx, &build_log_path).await.ok();
    let preview = read_workspace_json(workspace, ctx, "state/preview.json").await;
    let screenshot = match screenshot_id {
        Some(id) => {
            read_workspace_json(workspace, ctx, &format!("outputs/screenshots/{id}.json")).await
        }
        None => None,
    };

    PromotionGateReport {
        build_log_has_terminal_error: build_log.as_deref().map_or(true, has_terminal_error),
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

async fn write_package_install_log(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    args: &[String],
    output: &SandboxCommandOutput,
) -> Result<String, ToolError> {
    let path = ctx.workspace_root.join("outputs/build/package-install.log");
    let text = format!(
        "$ {}\n\nstatus: {:?}\n\nstdout:\n{}\n\nstderr:\n{}\n",
        args.join(" "),
        output.status,
        output.stdout,
        output.stderr
    );
    workspace
        .write_string(ctx, &path, &text)
        .await
        .map_err(|error| ToolError::Recoverable(error.to_string()))?;
    Ok("/workspace/outputs/build/package-install.log".to_string())
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
