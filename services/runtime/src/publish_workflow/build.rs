use crate::{
    artifact_publisher::{source_snapshot_fingerprint, ArtifactFile, FileArtifactPublisher},
    templates::{BuiltInTemplateRegistry, SourceSnapshot, TemplateId, TemplateRegistry},
    types::{canonical_json_hash, sha256_hex},
    visual_contracts::DraftSnapshot,
};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const MAX_SOURCE_FILES: usize = 20_000;
const MAX_OUTPUT_FILES: usize = 20_000;
const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 32 * 1024;

#[derive(Debug)]
pub struct ProductionBuildOutput {
    pub output_root: PathBuf,
    pub output_hash: String,
}

pub async fn build_frozen_snapshot(
    runtime_storage_dir: &Path,
    workflow_id: &str,
    snapshot: &DraftSnapshot,
) -> Result<ProductionBuildOutput> {
    if snapshot.template_id != "next-app"
        || !matches!(
            snapshot.template_version.as_str(),
            "next-app@1" | "next-app@2"
        )
    {
        bail!("PublishWorkflow currently supports the next-app@1/@2 production contracts");
    }
    let files = verified_source_files(runtime_storage_dir, snapshot)?;
    if files.is_empty() || files.len() > MAX_SOURCE_FILES {
        bail!("source snapshot file count is outside the production build limit");
    }
    validate_template_source(&files)?;

    let workflow_root = runtime_storage_dir
        .join("publish-workflows")
        .join(safe_segment(workflow_id));
    let source_root = workflow_root.join("source");
    if source_root.exists() {
        fs::remove_dir_all(&source_root)?;
    }
    fs::create_dir_all(&source_root)?;
    for file in files {
        validate_relative_path(&file.path)?;
        let target = source_root.join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, file.bytes)?;
    }

    run_command(
        &source_root,
        &[
            "npm",
            "ci",
            "--include=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ],
        Duration::from_secs(180),
        runtime_storage_dir,
        false,
    )
    .await?;
    run_command(
        &source_root,
        &["npm", "run", "build"],
        Duration::from_secs(240),
        runtime_storage_dir,
        true,
    )
    .await?;

    let output_root = source_root.join("out");
    remove_scaffolding_markers(&output_root)?;
    validate_static_export(&output_root)
}

pub fn verify_frozen_snapshot(runtime_storage_dir: &Path, snapshot: &DraftSnapshot) -> Result<()> {
    verified_source_files(runtime_storage_dir, snapshot).map(|_| ())
}

pub(super) fn cleanup_build_workspace(runtime_storage_dir: &Path, workflow_id: &str) -> Result<()> {
    let workflow_root = runtime_storage_dir
        .join("publish-workflows")
        .join(safe_segment(workflow_id));
    if workflow_root.exists() {
        fs::remove_dir_all(workflow_root)?;
    }
    Ok(())
}

fn verified_source_files(
    runtime_storage_dir: &Path,
    snapshot: &DraftSnapshot,
) -> Result<Vec<ArtifactFile>> {
    let snapshot_id = snapshot_id_from_uri(&snapshot.source_snapshot_uri, &snapshot.project_id)?;
    let files = FileArtifactPublisher::read_source_snapshot(
        runtime_storage_dir,
        &snapshot.project_id,
        snapshot_id,
    )?;
    let actual = source_fingerprint(&files)?;
    if actual != snapshot.source_hash {
        bail!("publish.source_identity_stale: frozen source bytes do not match sourceHash");
    }
    Ok(files)
}

pub(super) fn source_fingerprint(files: &[ArtifactFile]) -> Result<String> {
    source_snapshot_fingerprint(files)
}

fn validate_template_source(files: &[ArtifactFile]) -> Result<()> {
    let registry = BuiltInTemplateRegistry::built_in();
    let template = registry.current(&TemplateId::parse("next-app")?)?;
    let mut snapshot = SourceSnapshot::default();
    for file in files {
        let path = normalized_path(&file.path)?;
        snapshot.files.insert(
            path.clone(),
            std::str::from_utf8(&file.bytes).ok().map(str::to_string),
        );
        if let Some(root) = path.split('/').next() {
            snapshot.present_roots.insert(root.to_string());
        }
    }
    let report = template.operations.validate_source(&snapshot);
    if !report.is_valid() {
        bail!("{}: {}", report.summary, report.violations.join("; "));
    }
    Ok(())
}

async fn run_command(
    cwd: &Path,
    argv: &[&str],
    timeout: Duration,
    runtime_storage_dir: &Path,
    production: bool,
) -> Result<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow!("production build command is empty"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("CI", "1")
        .env("NEXT_TELEMETRY_DISABLED", "1")
        .env("npm_config_cache", runtime_storage_dir.join("npm-cache"))
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if production {
        command.env("NODE_ENV", "production");
    }
    for name in ["PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| anyhow!("production build command timed out: {}", argv.join(" ")))??;
    if !output.status.success() {
        let stdout = diagnostic(&output.stdout);
        let stderr = diagnostic(&output.stderr);
        bail!(
            "production build command failed ({}): stdout={} stderr={}",
            output.status,
            stdout,
            stderr
        );
    }
    Ok(())
}

fn validate_static_export(output_root: &Path) -> Result<ProductionBuildOutput> {
    if !output_root.join("index.html").is_file() {
        bail!("next build did not produce out/index.html");
    }
    let mut stack = vec![output_root.to_path_buf()];
    let mut files = BTreeMap::new();
    let mut total_bytes = 0_u64;
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                bail!("production output contains a symbolic link");
            }
            if file_type.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                bail!("production output contains an unsupported filesystem entry");
            }
            let relative = entry.path().strip_prefix(output_root)?.to_path_buf();
            validate_relative_path(&relative)?;
            let bytes = fs::read(entry.path())?;
            total_bytes = total_bytes.saturating_add(bytes.len() as u64);
            if files.len() >= MAX_OUTPUT_FILES || total_bytes > MAX_OUTPUT_BYTES {
                bail!("production output exceeds artifact limits");
            }
            files.insert(normalized_path(&relative)?, sha256_hex(&bytes));
        }
    }
    let index = fs::read_to_string(output_root.join("index.html"))?;
    if !index.contains("<html") || !index.contains("<title") {
        bail!("production index.html is missing required document metadata");
    }
    let output_hash = canonical_json_hash(&json!({
        "schemaVersion": "production-output@1",
        "files": files,
        "totalBytes": total_bytes,
    }));
    Ok(ProductionBuildOutput {
        output_root: output_root.to_path_buf(),
        output_hash,
    })
}

fn remove_scaffolding_markers(output_root: &Path) -> Result<()> {
    let mut stack = vec![output_root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                bail!("production output contains a symbolic link");
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() && entry.file_name() == ".gitkeep" {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

fn snapshot_id_from_uri<'a>(uri: &'a str, project_id: &str) -> Result<&'a str> {
    let mut parts = uri.split('/');
    if parts.next() != Some("runtime:")
        || parts.next() != Some("")
        || parts.next() != Some("source-snapshots")
        || parts.next() != Some(safe_segment(project_id).as_str())
    {
        bail!("DraftSnapshot sourceSnapshotUri is not a Runtime source snapshot");
    }
    let snapshot_id = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("DraftSnapshot sourceSnapshotUri has no snapshot id"))?;
    if parts.next().is_some() {
        bail!("DraftSnapshot sourceSnapshotUri contains unexpected path segments");
    }
    Ok(snapshot_id)
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("production build rejected unsafe relative path");
    }
    Ok(())
}

fn normalized_path(path: &Path) -> Result<String> {
    validate_relative_path(path)?;
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("production path is not valid UTF-8")),
            Component::CurDir => Ok(String::new()),
            _ => Err(anyhow!("production path is invalid")),
        })
        .filter_map(|part| match part {
            Ok(part) if part.is_empty() => None,
            other => Some(other),
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn diagnostic(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(MAX_DIAGNOSTIC_BYTES);
    String::from_utf8_lossy(&bytes[start..]).replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact_publisher::FileArtifactPublisher,
        templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
        visual_contracts::{DraftSnapshotRetentionState, DRAFT_SNAPSHOT_SCHEMA},
    };
    use chrono::Utc;

    #[test]
    fn cleanup_removes_only_the_selected_workflow_workspace() {
        let root = std::env::temp_dir().join(format!(
            "publish-workflow-cleanup-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let selected = root.join("publish-workflows/workflow-selected/source");
        let retained = root.join("publish-workflows/workflow-retained/source");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir_all(&retained).unwrap();
        std::fs::write(selected.join("package.json"), b"{}").unwrap();
        std::fs::write(retained.join("package.json"), b"{}").unwrap();

        cleanup_build_workspace(&root, "workflow-selected").unwrap();

        assert!(!root.join("publish-workflows/workflow-selected").exists());
        assert!(root.join("publish-workflows/workflow-retained").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "production build canary requires Node.js and npm registry/cache access"]
    async fn next_app_frozen_snapshot_runs_real_production_build() {
        let root = std::env::temp_dir().join(format!(
            "publish-production-build-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let template = BuiltInTemplateRegistry::built_in()
            .current(&TemplateId::parse("next-app").unwrap())
            .unwrap();
        let files = template
            .files
            .iter()
            .map(|file| ArtifactFile {
                path: PathBuf::from(file.path),
                bytes: file.content_for_write().as_bytes().to_vec(),
            })
            .collect::<Vec<_>>();
        let source_hash = source_fingerprint(&files).unwrap();
        let source_snapshot_uri = FileArtifactPublisher::new(&root)
            .publish_source_snapshot("project-canary", "build-canary", files)
            .await
            .unwrap();
        let snapshot = DraftSnapshot {
            schema_version: DRAFT_SNAPSHOT_SCHEMA.to_string(),
            snapshot_id: "draft-canary".to_string(),
            project_id: "project-canary".to_string(),
            source_snapshot_uri,
            source_hash,
            template_id: "next-app".to_string(),
            template_version: "next-app@1".to_string(),
            dependency_policy_version: "runtime-dependency-policy@1".to_string(),
            design_context_hash: "d".repeat(64),
            created_by_run_id: "run-canary".to_string(),
            based_on_snapshot_id: None,
            restored_from_version_id: None,
            created_at: Utc::now(),
            retention_state: DraftSnapshotRetentionState::Active,
            delete_after: None,
        };
        let output = build_frozen_snapshot(&root, "workflow-canary", &snapshot)
            .await
            .unwrap();
        assert!(output.output_root.join("index.html").is_file());
        assert_eq!(output.output_hash.len(), 64);
        std::fs::remove_dir_all(root).unwrap();
    }
}
