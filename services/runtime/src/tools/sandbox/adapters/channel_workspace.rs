use crate::{
    config::RuntimeConfig, tools::runtime::ToolContext, workspace_auth::WorkspaceChannelJwtIssuer,
};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use super::super::ports::{
    export_workspace_tree, WorkspaceBackend, WorkspaceEntry, WorkspaceEntryKind,
    WorkspaceExportReceipt, WorkspacePathKind,
};
use super::channel_path::{normalize_path, workspace_channel_path};
use super::channel_transport::{
    SandboxBindingEndpointResolver, WebSocketWorkspaceChannelTransport, WorkspaceChannelClientTls,
    WorkspaceChannelEndpointResolver, WorkspaceChannelRequest, WorkspaceChannelTransport,
};

#[derive(Clone)]
pub struct SandboxChannelWorkspaceBackend {
    timeout: Duration,
    endpoint_resolver: Arc<dyn WorkspaceChannelEndpointResolver>,
    tls: Option<WorkspaceChannelClientTls>,
}

impl Default for SandboxChannelWorkspaceBackend {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            endpoint_resolver: Arc::new(SandboxBindingEndpointResolver::default()),
            tls: None,
        }
    }
}

impl SandboxChannelWorkspaceBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_runtime_config(config: &RuntimeConfig) -> io::Result<Self> {
        let key_file = config
            .workspace_channel_signing_key_file
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "workspace channel signing key is not configured",
                )
            })?;
        let issuer = WorkspaceChannelJwtIssuer::from_pkcs8_der_file(
            key_file,
            config.workspace_channel_token_ttl_seconds,
        )?;
        let tls = WorkspaceChannelClientTls::from_runtime_config(config)?;
        let scheme = if tls.is_some() { "wss" } else { "ws" };
        Ok(Self::new().with_tls(tls).with_endpoint_resolver(Arc::new(
            SandboxBindingEndpointResolver::with_token_issuer(issuer).with_channel_scheme(scheme),
        )))
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
    ) -> io::Result<JsonWorkspaceChannelBackend<WebSocketWorkspaceChannelTransport>> {
        let endpoint = self.endpoint_resolver.endpoint(ctx).await?;
        let authorization = self.endpoint_resolver.authorization(ctx).await?;
        if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported workspace channel endpoint: {endpoint}"),
            ));
        }
        let mut transport = WebSocketWorkspaceChannelTransport::new(endpoint)
            .with_timeout(self.timeout)
            .with_tls(self.tls.clone());
        if let Some(authorization) = authorization {
            transport = transport.with_authorization(authorization);
        }
        Ok(JsonWorkspaceChannelBackend::new(
            transport,
            ctx.workspace_root.clone(),
        ))
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

    async fn write_bytes(&self, ctx: &ToolContext, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.channel_backend(ctx)
            .await?
            .write_bytes(ctx, path, bytes)
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

    async fn export_tree(
        &self,
        ctx: &ToolContext,
        from: &Path,
        target_root: &Path,
        excluded_files: &[String],
    ) -> io::Result<WorkspaceExportReceipt> {
        self.channel_backend(ctx)
            .await?
            .export_tree(ctx, from, target_root, excluded_files)
            .await
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

    async fn read_bytes(&self, _ctx: &ToolContext, path: &Path) -> io::Result<Vec<u8>> {
        let value = self.request("fs.readBytes", path, json!({})).await?;
        let encoded = value.get("base64").and_then(Value::as_str).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "fs.readBytes missing base64")
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    async fn write_string(&self, _ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        self.request("fs.write", path, json!({ "text": text }))
            .await?;
        Ok(())
    }

    async fn write_bytes(&self, _ctx: &ToolContext, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.request(
            "fs.writeBytes",
            path,
            json!({ "base64": base64::engine::general_purpose::STANDARD.encode(bytes) }),
        )
        .await?;
        Ok(())
    }

    async fn rename(&self, _ctx: &ToolContext, from: &Path, to: &Path) -> io::Result<()> {
        self.request("fs.rename", from, json!({ "to": self.workspace_path(to)? }))
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

    async fn export_tree(
        &self,
        ctx: &ToolContext,
        from: &Path,
        target_root: &Path,
        excluded_files: &[String],
    ) -> io::Result<WorkspaceExportReceipt> {
        let request = WorkspaceChannelRequest {
            op: "archive.export",
            path: self.workspace_path(from)?,
            payload: json!({ "excludedFiles": excluded_files }),
        };
        match self
            .transport
            .export_tree(request, target_root, excluded_files)
            .await
        {
            Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                export_workspace_tree(self, ctx, from, target_root, excluded_files).await
            }
            result => result,
        }
    }
}
