use crate::{
    channel_manager::ChannelManager,
    config::{RuntimeConfig, WorkspaceChannelTlsMode},
    tools::runtime::ToolContext,
    types::sha256_hex,
    workspace_auth::{WorkspaceChannelClaims, WorkspaceChannelJwtIssuer},
};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use rustls::{
    pki_types::{CertificateDer, ServerName},
    ClientConfig, RootCertStore,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::TcpStream, time};
use tokio_tungstenite::{
    client_async, connect_async,
    tungstenite::{
        client::IntoClientRequest,
        handshake::client::{Request as WebSocketRequest, Response as WebSocketResponse},
        http::header::AUTHORIZATION,
        Message,
    },
    MaybeTlsStream, WebSocketStream,
};
use x509_parser::{extensions::GeneralName, parse_x509_certificate};

use super::super::ports::WorkspaceExportReceipt;

#[async_trait]
pub trait WorkspaceChannelTransport: Send + Sync {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value>;

    async fn export_tree(
        &self,
        _request: WorkspaceChannelRequest,
        _target_root: &Path,
        _excluded_files: &[String],
    ) -> io::Result<WorkspaceExportReceipt> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "workspace channel transport does not support streaming export",
        ))
    }
}

#[async_trait]
pub trait WorkspaceChannelEndpointResolver: Send + Sync {
    async fn endpoint(&self, ctx: &ToolContext) -> io::Result<String>;

    async fn authorization(&self, _ctx: &ToolContext) -> io::Result<Option<String>> {
        Ok(None)
    }
}

pub struct SandboxBindingEndpointResolver {
    token_issuer: Option<Arc<WorkspaceChannelJwtIssuer>>,
    channel_manager: Arc<ChannelManager>,
    channel_scheme: &'static str,
}

impl Default for SandboxBindingEndpointResolver {
    fn default() -> Self {
        Self {
            token_issuer: None,
            channel_manager: ChannelManager::shared(),
            channel_scheme: "ws",
        }
    }
}

impl SandboxBindingEndpointResolver {
    pub fn with_token_issuer(token_issuer: WorkspaceChannelJwtIssuer) -> Self {
        Self {
            token_issuer: Some(Arc::new(token_issuer)),
            channel_manager: ChannelManager::shared(),
            channel_scheme: "ws",
        }
    }

    pub fn with_channel_scheme(mut self, channel_scheme: &'static str) -> Self {
        self.channel_scheme = channel_scheme;
        self
    }
}

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
        self.channel_manager
            .endpoint(
                &ctx.store,
                &binding,
                &ctx.run.id,
                crate::sandbox_adapter::WORKSPACE_CHANNEL_PORT,
                self.channel_scheme,
                "/workspace",
            )
            .await
            .map_err(|error| io::Error::new(io::ErrorKind::NotConnected, error))
    }

    async fn authorization(&self, ctx: &ToolContext) -> io::Result<Option<String>> {
        let Some(issuer) = &self.token_issuer else {
            return Ok(None);
        };
        let binding_id = ctx.run.sandbox_id.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "run is not bound to a sandbox channel",
            )
        })?;
        let binding = ctx
            .store
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "sandbox binding not found"))?;
        let pod_uid = binding.pod_uid.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sandbox binding has no verified pod UID",
            )
        })?;
        let token = issuer.issue(WorkspaceChannelClaims {
            iss: String::new(),
            aud: String::new(),
            exp: 0,
            iat: 0,
            jti: sha256_hex(&rand::random::<[u8; 32]>()),
            sandbox_binding_id: binding.id,
            sandbox_name: binding.sandbox_name,
            pod_uid,
            project_id: binding.project_id,
            run_id: ctx.run.id.clone(),
            operations: vec![
                "fs.read".to_string(),
                "fs.write".to_string(),
                "process.exec".to_string(),
                "process.manage".to_string(),
                "archive.export".to_string(),
            ],
        })?;
        Ok(Some(format!("Bearer {token}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceChannelRequest {
    pub op: &'static str,
    pub path: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct WorkspaceChannelClientTls {
    client_config: Arc<ClientConfig>,
    expected_server_san: Arc<String>,
}

impl WorkspaceChannelClientTls {
    pub fn from_runtime_config(config: &RuntimeConfig) -> io::Result<Option<Self>> {
        if config.workspace_channel_tls_mode == WorkspaceChannelTlsMode::DebugLoopback {
            return Ok(None);
        }
        let ca_file = required_tls_file(&config.workspace_channel_ca_file, "CA")?;
        let cert_file =
            required_tls_file(&config.workspace_channel_client_cert_file, "client cert")?;
        let key_file = required_tls_file(&config.workspace_channel_client_key_file, "client key")?;
        // remote-fs-boundary: allow-begin runtime-owned-workspace-channel-tls-secret
        let mut ca_reader = io::BufReader::new(fs::File::open(ca_file)?);
        let ca_certs = rustls_pemfile::certs(&mut ca_reader).collect::<Result<Vec<_>, _>>()?;
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(ca_certs);
        if added == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace channel CA contains no certificates",
            ));
        }
        let mut cert_reader = io::BufReader::new(fs::File::open(cert_file)?);
        let cert_chain = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;
        let mut key_reader = io::BufReader::new(fs::File::open(key_file)?);
        let private_key = rustls_pemfile::private_key(&mut key_reader)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "client key is missing"))?;
        // remote-fs-boundary: allow-end runtime-owned-workspace-channel-tls-secret
        let client_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(Self {
            client_config: Arc::new(client_config),
            expected_server_san: Arc::new(config.workspace_channel_server_san.clone()),
        }))
    }
}

fn required_tls_file<'a>(path: &'a Option<PathBuf>, label: &str) -> io::Result<&'a Path> {
    path.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("workspace channel {label} file is not configured"),
        )
    })
}

#[derive(Debug, Clone)]
pub struct WebSocketWorkspaceChannelTransport {
    endpoint: String,
    authorization: Option<String>,
    timeout: Duration,
    tls: Option<WorkspaceChannelClientTls>,
}

impl WebSocketWorkspaceChannelTransport {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            authorization: None,
            timeout: Duration::from_secs(30),
            tls: None,
        }
    }

    pub fn with_authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_tls(mut self, tls: Option<WorkspaceChannelClientTls>) -> Self {
        self.tls = tls;
        self
    }
}

#[async_trait]
impl WorkspaceChannelTransport for WebSocketWorkspaceChannelTransport {
    async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value> {
        let endpoint = self.endpoint.clone();
        let authorization = self.authorization.clone();
        let timeout = self.timeout;
        let tls = self.tls.clone();
        time::timeout(timeout, async move {
            let mut last_error = None;
            let max_attempts = if authorization.is_some() { 1 } else { 3 };
            for attempt in 1..=max_attempts {
                match websocket_channel_request_once(
                    &endpoint,
                    authorization.as_deref(),
                    request.clone(),
                    tls.as_ref(),
                )
                .await
                {
                    Ok(value) => return Ok(value),
                    Err(error)
                        if is_transient_workspace_channel_error(&error)
                            && attempt < max_attempts =>
                    {
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

    async fn export_tree(
        &self,
        request: WorkspaceChannelRequest,
        target_root: &Path,
        excluded_files: &[String],
    ) -> io::Result<WorkspaceExportReceipt> {
        time::timeout(
            self.timeout,
            websocket_channel_export_once(
                &self.endpoint,
                self.authorization.as_deref(),
                request,
                target_root,
                excluded_files,
                self.tls.as_ref(),
            ),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "workspace export timed out"))?
    }
}

const WORKSPACE_EXPORT_MAX_BYTES: u64 = 256 * 1024 * 1024;
const WORKSPACE_EXPORT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const WORKSPACE_EXPORT_MAX_FILES: usize = 20_000;

struct StreamingExportFile {
    path: String,
    expected_hash: String,
    remaining: u64,
    file: tokio::fs::File,
    digest: Sha256,
}

async fn websocket_channel_export_once(
    endpoint: &str,
    authorization: Option<&str>,
    request: WorkspaceChannelRequest,
    target_root: &Path,
    excluded_files: &[String],
    tls: Option<&WorkspaceChannelClientTls>,
) -> io::Result<WorkspaceExportReceipt> {
    let mut handshake = endpoint
        .into_client_request()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if let Some(authorization) = authorization {
        handshake.headers_mut().insert(
            AUTHORIZATION,
            authorization
                .parse()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
        );
    }
    handshake.headers_mut().insert(
        "x-anydesign-workspace-operation",
        "archive.export"
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    );
    let (mut socket, _) = connect_workspace_channel(handshake, tls).await?;
    socket
        .send(Message::Text(
            serde_json::to_string(&json!({
                "op": request.op,
                "path": request.path,
                "payload": request.payload,
            }))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .into(),
        ))
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
    // remote-fs-boundary: allow-begin runtime-storage-streaming-export-sink
    if target_root.exists() {
        tokio::fs::remove_dir_all(target_root).await?;
    }
    tokio::fs::create_dir_all(target_root).await?;
    // remote-fs-boundary: allow-end runtime-storage-streaming-export-sink
    let excluded = excluded_files
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut started = false;
    let mut current: Option<StreamingExportFile> = None;
    let mut manifest = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
        match message {
            Message::Text(text) => {
                if current.as_ref().is_some_and(|file| file.remaining != 0) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "workspace export file ended before declared size",
                    ));
                }
                if let Some(file) = current.take() {
                    let actual_hash = format!("{:x}", file.digest.finalize());
                    if actual_hash != file.expected_hash {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("workspace export checksum mismatch: {}", file.path),
                        ));
                    }
                }
                let frame: Value = serde_json::from_str(&text)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                match frame.get("type").and_then(Value::as_str) {
                    Some("archive.start") => {
                        if started
                            || frame.get("format").and_then(Value::as_str)
                                != Some("anydesign-tree-stream@1")
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "invalid workspace export start frame",
                            ));
                        }
                        started = true;
                    }
                    Some("archive.file") if started => {
                        let relative =
                            frame.get("path").and_then(Value::as_str).ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "workspace export file path missing",
                                )
                            })?;
                        let relative_path = PathBuf::from(relative);
                        if relative_path.is_absolute()
                            || relative_path
                                .components()
                                .any(|component| !matches!(component, Component::Normal(_)))
                            || excluded.contains(relative)
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "workspace export path is invalid",
                            ));
                        }
                        let bytes =
                            frame.get("bytes").and_then(Value::as_u64).ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "workspace export file size missing",
                                )
                            })?;
                        if bytes > WORKSPACE_EXPORT_MAX_FILE_BYTES
                            || manifest.len() >= WORKSPACE_EXPORT_MAX_FILES
                            || total_bytes.saturating_add(bytes) > WORKSPACE_EXPORT_MAX_BYTES
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::FileTooLarge,
                                "workspace export exceeds configured limits",
                            ));
                        }
                        let expected_hash = frame
                            .get("sha256")
                            .and_then(Value::as_str)
                            .filter(|hash| {
                                hash.len() == 64
                                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                            })
                            .ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "workspace export file hash missing",
                                )
                            })?
                            .to_string();
                        let output = target_root.join(&relative_path);
                        // remote-fs-boundary: allow-begin runtime-storage-streaming-export-sink
                        if let Some(parent) = output.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }
                        let file = tokio::fs::File::create(output).await?;
                        // remote-fs-boundary: allow-end runtime-storage-streaming-export-sink
                        manifest.push(json!({
                            "path": relative,
                            "bytes": bytes,
                            "sha256": expected_hash,
                        }));
                        total_bytes += bytes;
                        current = Some(StreamingExportFile {
                            path: relative.to_string(),
                            expected_hash,
                            remaining: bytes,
                            file,
                            digest: Sha256::new(),
                        });
                    }
                    Some("archive.end") if started => {
                        manifest.sort_by(|left, right| {
                            left["path"].as_str().cmp(&right["path"].as_str())
                        });
                        let manifest_hash =
                            sha256_hex(&serde_json::to_vec(&manifest).map_err(|error| {
                                io::Error::new(io::ErrorKind::InvalidData, error)
                            })?);
                        if frame.get("fileCount").and_then(Value::as_u64)
                            != Some(manifest.len() as u64)
                            || frame.get("totalBytes").and_then(Value::as_u64) != Some(total_bytes)
                            || frame.get("manifestHash").and_then(Value::as_str)
                                != Some(manifest_hash.as_str())
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "workspace export manifest mismatch",
                            ));
                        }
                        return Ok(WorkspaceExportReceipt {
                            target_root: target_root.to_path_buf(),
                            file_count: manifest.len(),
                            total_bytes,
                            manifest_hash,
                        });
                    }
                    Some("archive.error") => {
                        return Err(io::Error::other(
                            frame
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("workspace export failed"),
                        ));
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unexpected workspace export control frame",
                        ))
                    }
                }
            }
            Message::Binary(bytes) => {
                let file = current.as_mut().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "workspace export binary frame has no file header",
                    )
                })?;
                if bytes.len() as u64 > file.remaining {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "workspace export sent more bytes than declared",
                    ));
                }
                file.file.write_all(&bytes).await?;
                file.digest.update(&bytes);
                file.remaining -= bytes.len() as u64;
            }
            Message::Close(_) => break,
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
            }
            Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "workspace export stream ended before manifest",
    ))
}

async fn websocket_channel_request_once(
    endpoint: &str,
    authorization: Option<&str>,
    request: WorkspaceChannelRequest,
    tls: Option<&WorkspaceChannelClientTls>,
) -> io::Result<Value> {
    let mut handshake = endpoint
        .into_client_request()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if let Some(authorization) = authorization {
        handshake.headers_mut().insert(
            AUTHORIZATION,
            authorization
                .parse()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
        );
    }
    let operation = workspace_channel_operation(request.op).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("unsupported workspace channel operation: {}", request.op),
        )
    })?;
    handshake.headers_mut().insert(
        "x-anydesign-workspace-operation",
        operation
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    );
    let (mut socket, _) = connect_workspace_channel(handshake, tls).await?;
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
        let kind = match response.get("code").and_then(Value::as_str) {
            Some("ENOENT") => io::ErrorKind::NotFound,
            Some("EACCES" | "EPERM") => io::ErrorKind::PermissionDenied,
            Some("EEXIST") => io::ErrorKind::AlreadyExists,
            _ => io::ErrorKind::Other,
        };
        return Err(io::Error::new(kind, message.to_string()));
    }
    if let Some(result) = response.get("result") {
        return Ok(result.clone());
    }
    Ok(response)
}

fn workspace_channel_operation(op: &str) -> Option<&'static str> {
    match op {
        "fs.read" | "fs.readBytes" | "fs.list" | "fs.stat" => Some("fs.read"),
        "fs.write" | "fs.writeBytes" | "fs.removeFile" | "fs.removeDirAll" | "fs.copyDir"
        | "fs.rename" => Some("fs.write"),
        "process.exec" => Some("process.exec"),
        "process.start" | "process.status" | "process.stop" => Some("process.manage"),
        "archive.export" => Some("archive.export"),
        _ => None,
    }
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

async fn connect_workspace_channel(
    handshake: WebSocketRequest,
    tls: Option<&WorkspaceChannelClientTls>,
) -> io::Result<(
    WebSocketStream<MaybeTlsStream<TcpStream>>,
    WebSocketResponse,
)> {
    if let Some(tls) = tls {
        let host = handshake
            .uri()
            .host()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "channel host is missing"))?
            .to_string();
        let port = handshake.uri().port_u16().unwrap_or(443);
        let tcp = TcpStream::connect((host.as_str(), port)).await?;
        let server_name = ServerName::try_from(host)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid channel host"))?;
        let tls_stream = tokio_rustls::TlsConnector::from(tls.client_config.clone())
            .connect(server_name, tcp)
            .await
            .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))?;
        let stream = MaybeTlsStream::Rustls(tls_stream);
        verify_workspace_channel_server_san(&stream, &tls.expected_server_san)?;
        client_async(handshake, stream)
            .await
            .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))
    } else {
        connect_async(handshake)
            .await
            .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))
    }
}

fn verify_workspace_channel_server_san(
    stream: &MaybeTlsStream<TcpStream>,
    expected_san: &str,
) -> io::Result<()> {
    let MaybeTlsStream::Rustls(stream) = stream else {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mTLS workspace channel did not negotiate rustls",
        ));
    };
    let certificates =
        stream.get_ref().1.peer_certificates().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "server cert is missing")
        })?;
    let certificate: &CertificateDer<'_> = certificates
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "server cert is missing"))?;
    let (_, parsed) = parse_x509_certificate(certificate.as_ref())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "server cert is invalid"))?;
    let matches = parsed
        .subject_alternative_name()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "server SAN is invalid"))?
        .is_some_and(|extension| {
            extension
                .value
                .general_names
                .iter()
                .any(|name| matches!(name, GeneralName::URI(uri) if *uri == expected_san))
        });
    if !matches {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "workspace channel server SPIFFE SAN mismatch",
        ));
    }
    Ok(())
}
