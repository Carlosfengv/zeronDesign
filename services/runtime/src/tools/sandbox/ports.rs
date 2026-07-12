use crate::{
    tools::runtime::{ProgressSink, ToolContext},
    types::sha256_hex,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: WorkspaceEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceExportReceipt {
    pub target_root: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
    pub manifest_hash: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProcessLease {
    pub lease_id: String,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
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

    async fn start_process(
        &self,
        _ctx: &ToolContext,
        _lease_id: &str,
        _argv: &[String],
        _cwd: &Path,
    ) -> io::Result<SandboxProcessLease> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sandbox command backend does not support process leases",
        ))
    }

    async fn process_status(
        &self,
        _ctx: &ToolContext,
        _lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sandbox command backend does not support process leases",
        ))
    }

    async fn stop_process(
        &self,
        _ctx: &ToolContext,
        _lease_id: &str,
    ) -> io::Result<SandboxProcessLease> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sandbox command backend does not support process leases",
        ))
    }
}

#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    async fn read_to_string(&self, ctx: &ToolContext, path: &Path) -> io::Result<String>;
    async fn read_bytes(&self, ctx: &ToolContext, path: &Path) -> io::Result<Vec<u8>> {
        Ok(self.read_to_string(ctx, path).await?.into_bytes())
    }
    async fn write_string(&self, ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()>;
    async fn write_bytes(&self, ctx: &ToolContext, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_string(ctx, path, text).await
    }
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
    async fn export_tree(
        &self,
        ctx: &ToolContext,
        from: &Path,
        target_root: &Path,
        excluded_files: &[String],
    ) -> io::Result<WorkspaceExportReceipt> {
        export_workspace_tree(self, ctx, from, target_root, excluded_files).await
    }
}

pub(super) async fn export_workspace_tree<B: WorkspaceBackend + ?Sized>(
    workspace: &B,
    ctx: &ToolContext,
    from: &Path,
    target_root: &Path,
    excluded_files: &[String],
) -> io::Result<WorkspaceExportReceipt> {
    // remote-fs-boundary: allow-begin runtime-storage-export-sink
    if target_root.exists() {
        fs::remove_dir_all(target_root)?;
    }
    fs::create_dir_all(target_root)?;
    // remote-fs-boundary: allow-end runtime-storage-export-sink
    let excluded = excluded_files
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut stack = vec![from.to_path_buf()];
    let mut manifest = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some(directory) = stack.pop() {
        let mut entries = workspace.list_dir(ctx, &directory).await?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        for entry in entries {
            let relative = entry.path.strip_prefix(from).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "export path escapes source root",
                )
            })?;
            let relative_string = relative.to_string_lossy().replace('\\', "/");
            match entry.kind {
                WorkspaceEntryKind::Dir => stack.push(entry.path),
                WorkspaceEntryKind::File => {
                    if excluded.contains(relative_string.as_str()) {
                        continue;
                    }
                    let bytes = workspace.read_bytes(ctx, &entry.path).await?;
                    let output = target_root.join(relative);
                    // remote-fs-boundary: allow-begin runtime-storage-export-sink
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&output, &bytes)?;
                    // remote-fs-boundary: allow-end runtime-storage-export-sink
                    total_bytes = total_bytes.saturating_add(bytes.len() as u64);
                    manifest.push(json!({
                        "path": relative_string,
                        "bytes": bytes.len(),
                        "sha256": sha256_hex(&bytes),
                    }));
                }
            }
        }
    }
    manifest.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest_hash = sha256_hex(
        &serde_json::to_vec(&manifest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
    );
    Ok(WorkspaceExportReceipt {
        target_root: target_root.to_path_buf(),
        file_count: manifest.len(),
        total_bytes,
        manifest_hash,
    })
}
