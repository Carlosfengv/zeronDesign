use crate::tools::runtime::ToolContext;
use async_trait::async_trait;
use std::{fs, io, path::Path};

use super::super::ports::{
    WorkspaceBackend, WorkspaceEntry, WorkspaceEntryKind, WorkspacePathKind,
};

// remote-fs-boundary: allow-begin local-workspace-backend
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

    async fn read_bytes(&self, _ctx: &ToolContext, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    async fn write_string(&self, _ctx: &ToolContext, path: &Path, text: &str) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)
    }

    async fn write_bytes(&self, _ctx: &ToolContext, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)
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
// remote-fs-boundary: allow-end local-workspace-backend
