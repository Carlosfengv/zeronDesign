use crate::tools::runtime::ToolContext;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use super::super::ports::{SandboxCommandBackend, SandboxCommandOutput, SandboxProcessLease};
use super::channel_path::{normalize_path, workspace_channel_path};
use super::channel_transport::{
    SandboxBindingEndpointResolver, WebSocketWorkspaceChannelTransport, WorkspaceChannelClientTls,
    WorkspaceChannelEndpointResolver, WorkspaceChannelRequest, WorkspaceChannelTransport,
};

#[derive(Clone)]
pub struct SandboxChannelCommandBackend {
    timeout: Duration,
    endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
    tls: Option<WorkspaceChannelClientTls>,
}

impl Default for SandboxChannelCommandBackend {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            endpoint_resolver: Arc::new(SandboxBindingEndpointResolver::default()),
            tls: None,
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

    pub fn with_tls(mut self, tls: Option<WorkspaceChannelClientTls>) -> Self {
        self.tls = tls;
        self
    }

    async fn channel_backend(
        &self,
        ctx: &ToolContext,
        timeout: Duration,
    ) -> io::Result<JsonWorkspaceChannelCommandBackend<WebSocketWorkspaceChannelTransport>> {
        let endpoint = self.endpoint_resolver.endpoint(ctx).await?;
        let authorization = self.endpoint_resolver.authorization(ctx).await?;
        if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported workspace channel endpoint: {endpoint}"),
            ));
        }
        let mut transport = WebSocketWorkspaceChannelTransport::new(endpoint)
            .with_timeout(timeout)
            .with_tls(self.tls.clone());
        if let Some(authorization) = authorization {
            transport = transport.with_authorization(authorization);
        }
        Ok(JsonWorkspaceChannelCommandBackend::new(
            transport,
            ctx.workspace_root.clone(),
        ))
    }
}

fn workspace_command_request_timeout(base: Duration, command_timeout_ms: u64) -> Duration {
    base.max(Duration::from_millis(command_timeout_ms).saturating_add(Duration::from_secs(5)))
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
        let request_timeout = workspace_command_request_timeout(self.timeout, timeout_ms);
        self.channel_backend(ctx, request_timeout)
            .await?
            .run(ctx, argv, cwd, timeout_ms)
            .await
    }

    async fn start_process(
        &self,
        ctx: &ToolContext,
        lease_id: &str,
        argv: &[String],
        cwd: &Path,
    ) -> io::Result<SandboxProcessLease> {
        self.channel_backend(ctx, self.timeout)
            .await?
            .start_process(ctx, lease_id, argv, cwd)
            .await
    }

    async fn process_status(
        &self,
        ctx: &ToolContext,
        lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        self.channel_backend(ctx, self.timeout)
            .await?
            .process_status(ctx, lease_id)
            .await
    }

    async fn stop_process(
        &self,
        ctx: &ToolContext,
        lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        self.channel_backend(ctx, self.timeout)
            .await?
            .stop_process(ctx, lease_id)
            .await
    }
}

#[cfg(test)]
mod workspace_command_timeout_tests {
    use super::workspace_command_request_timeout;
    use std::time::Duration;

    #[test]
    fn process_exec_transport_outlives_the_requested_command_timeout() {
        assert_eq!(
            workspace_command_request_timeout(Duration::from_secs(30), 120_000),
            Duration::from_secs(125)
        );
        assert_eq!(
            workspace_command_request_timeout(Duration::from_secs(30), 1_000),
            Duration::from_secs(30)
        );
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
    #[allow(clippy::let_and_return)]
    pub fn new(transport: T, workspace_root: impl Into<PathBuf>) -> Self {
        // remote-fs-boundary: allow-begin channel-local-root-alias
        let workspace_root = workspace_root.into();
        let backend = Self {
            transport: Arc::new(transport),
            workspace_root: fs::canonicalize(&workspace_root)
                .unwrap_or_else(|_| normalize_path(&workspace_root)),
        };
        // remote-fs-boundary: allow-end channel-local-root-alias
        backend
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
            .request(
                "process.exec",
                cwd,
                json!({
                    "argv": argv,
                    "timeoutMs": timeout_ms,
                }),
            )
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

    async fn start_process(
        &self,
        _ctx: &ToolContext,
        lease_id: &str,
        argv: &[String],
        cwd: &Path,
    ) -> io::Result<SandboxProcessLease> {
        let value = self
            .request(
                "process.start",
                cwd,
                json!({ "leaseId": lease_id, "argv": argv }),
            )
            .await?;
        process_lease_from_value(value)
    }

    async fn process_status(
        &self,
        _ctx: &ToolContext,
        lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        let value = self
            .request(
                "process.status",
                &self.workspace_root,
                json!({ "leaseId": lease_id }),
            )
            .await?;
        process_lease_from_value(value)
    }

    async fn stop_process(
        &self,
        _ctx: &ToolContext,
        lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        let value = self
            .request(
                "process.stop",
                &self.workspace_root,
                json!({ "leaseId": lease_id }),
            )
            .await?;
        process_lease_from_value(value)
    }
}

fn process_lease_from_value(value: Value) -> io::Result<SandboxProcessLease> {
    Ok(SandboxProcessLease {
        lease_id: value
            .get("leaseId")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "process leaseId missing"))?
            .to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "process status missing"))?
            .to_string(),
        pid: value
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        exit_code: value
            .get("exitCode")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
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
