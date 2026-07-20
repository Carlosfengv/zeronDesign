use crate::{
    artifact_manifest::{
        manifest_file, ArtifactDeliverySpec, ArtifactManifest, ArtifactManifestFile,
        ARTIFACT_MANIFEST_FILE,
    },
    object_storage::{delete_object_tree, sync_object_tree},
    templates::TemplateSpec,
    types::{sha256_hex, ArtifactPublishRecord, ArtifactPublishStatus},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct ArtifactFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

pub fn source_snapshot_fingerprint(files: &[ArtifactFile]) -> Result<String> {
    let mut framed = Vec::new();
    for file in files {
        validate_relative_path(&file.path)?;
        let path = normalized_relative_path(&file.path)?;
        if path == ".snapshot.json" {
            continue;
        }
        let content = std::str::from_utf8(&file.bytes)
            .map_err(|_| anyhow!("source snapshot contains a non-UTF-8 file: {path}"))?;
        framed.push((path, content));
    }
    framed.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = sha2::Sha256::new();
    for (path, content) in framed {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((content.len() as u64).to_be_bytes());
        digest.update(content.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StagedArtifact {
    pub project_id: String,
    pub version_id: String,
    pub candidate_manifest_hash: String,
    pub artifact_manifest_hash: String,
    pub staged_uri: String,
    pub file_count: usize,
}

#[async_trait]
pub trait ArtifactPublisher: Send + Sync {
    async fn stage(
        &self,
        project_id: &str,
        version_id: &str,
        candidate_manifest_hash: &str,
        files: Vec<ArtifactFile>,
    ) -> Result<StagedArtifact>;
    async fn promote(&self, staged: &StagedArtifact) -> Result<String>;
    async fn abort(&self, staged: &StagedArtifact) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct FileArtifactPublisher {
    runtime_storage_dir: PathBuf,
}

const SOURCE_SNAPSHOT_MANIFEST: &str = ".anydesign-source-snapshot-manifest.json";

struct StagedManifestContext<'a> {
    project_id: &'a str,
    version_id: &'a str,
    candidate_manifest_hash: &'a str,
    template_id: &'a str,
    template_version: &'a str,
    delivery: ArtifactDeliverySpec,
}

impl FileArtifactPublisher {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        let runtime_storage_dir = runtime_storage_dir.into();
        Self {
            runtime_storage_dir,
        }
    }

    pub fn version_root(runtime_storage_dir: &Path, project_id: &str, version_id: &str) -> PathBuf {
        runtime_storage_dir
            .join("artifacts")
            .join(safe_segment(project_id))
            .join("versions")
            .join(safe_segment(version_id))
    }

    pub fn staged_version_root(
        runtime_storage_dir: &Path,
        project_id: &str,
        version_id: &str,
    ) -> PathBuf {
        runtime_storage_dir
            .join("artifacts")
            .join(safe_segment(project_id))
            .join("staged")
            .join(safe_segment(version_id))
    }

    fn staged_root(&self, project_id: &str, version_id: &str) -> PathBuf {
        Self::staged_version_root(&self.runtime_storage_dir, project_id, version_id)
    }

    pub fn source_snapshot_root(
        runtime_storage_dir: &Path,
        project_id: &str,
        snapshot_id: &str,
    ) -> PathBuf {
        runtime_storage_dir
            .join("source-snapshots")
            .join(safe_segment(project_id))
            .join(safe_segment(snapshot_id))
    }

    pub async fn publish_source_snapshot(
        &self,
        project_id: &str,
        snapshot_id: &str,
        files: Vec<ArtifactFile>,
    ) -> Result<String> {
        let target = Self::source_snapshot_root(&self.runtime_storage_dir, project_id, snapshot_id);
        let temporary = target.with_extension("tmp");
        if temporary.exists() {
            fs::remove_dir_all(&temporary)?;
        }
        fs::create_dir_all(&temporary)?;
        let mut manifest_files = Vec::with_capacity(files.len());
        let mut normalized_paths = HashSet::new();
        for file in files {
            validate_relative_path(&file.path)?;
            let path = normalized_relative_path(&file.path)?;
            if path == SOURCE_SNAPSHOT_MANIFEST
                || !normalized_paths.insert(path.to_ascii_lowercase())
            {
                return Err(anyhow!(
                    "source snapshot contains a reserved or colliding path: {path}"
                ));
            }
            let output = temporary.join(&file.path);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(output, &file.bytes)?;
            manifest_files.push(serde_json::json!({
                "path": path,
                "bytes": file.bytes.len(),
                "sha256": sha256_hex(&file.bytes),
            }));
        }
        manifest_files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        let manifest = serde_json::json!({
            "schemaVersion": "source-snapshot-manifest@1",
            "projectId": project_id,
            "snapshotId": snapshot_id,
            "files": manifest_files,
        });
        let canonical_manifest = serde_json::to_vec(&manifest)?;
        fs::write(
            temporary.join(SOURCE_SNAPSHOT_MANIFEST),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        if target.exists() {
            let existing = read_canonical_json(&target.join(SOURCE_SNAPSHOT_MANIFEST))?;
            if existing != canonical_manifest {
                fs::remove_dir_all(&temporary).ok();
                return Err(anyhow!(
                    "immutable source snapshot already exists with different content"
                ));
            }
            fs::remove_dir_all(&temporary)?;
            sync_object_tree(&self.runtime_storage_dir, &target)?;
            return Ok(format!(
                "runtime://source-snapshots/{}/{}",
                safe_segment(project_id),
                safe_segment(snapshot_id)
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(temporary, &target)?;
        sync_object_tree(&self.runtime_storage_dir, &target)?;
        Ok(format!(
            "runtime://source-snapshots/{}/{}",
            safe_segment(project_id),
            safe_segment(snapshot_id)
        ))
    }

    pub fn read_source_snapshot(
        runtime_storage_dir: &Path,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<ArtifactFile>> {
        let root = Self::source_snapshot_root(runtime_storage_dir, project_id, snapshot_id);
        let manifest_path = root.join(SOURCE_SNAPSHOT_MANIFEST);
        if manifest_path.exists() {
            let manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
            let entries = manifest
                .get("files")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| anyhow!("source snapshot manifest has no files"))?;
            let mut files = Vec::with_capacity(entries.len());
            for entry in entries {
                let relative = entry
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow!("source snapshot manifest path is invalid"))?;
                let relative = PathBuf::from(relative);
                validate_relative_path(&relative)?;
                let bytes = fs::read(root.join(&relative))?;
                let expected_size = entry
                    .get("bytes")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| anyhow!("source snapshot manifest size is invalid"))?;
                let expected_hash = entry
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow!("source snapshot manifest hash is invalid"))?;
                if bytes.len() as u64 != expected_size || sha256_hex(&bytes) != expected_hash {
                    return Err(anyhow!(
                        "source snapshot integrity check failed for {}",
                        relative.display()
                    ));
                }
                files.push(ArtifactFile {
                    path: relative,
                    bytes,
                });
            }
            return Ok(files);
        }

        // Read-only compatibility for snapshots created before manifest@1.
        let mut files = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(directory) = stack.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    if path.file_name().and_then(|name| name.to_str())
                        == Some(SOURCE_SNAPSHOT_MANIFEST)
                    {
                        continue;
                    }
                    files.push(ArtifactFile {
                        path: path.strip_prefix(&root)?.to_path_buf(),
                        bytes: fs::read(path)?,
                    });
                }
            }
        }
        Ok(files)
    }

    pub async fn stage_directory(
        &self,
        project_id: &str,
        version_id: &str,
        candidate_manifest_hash: &str,
        source_root: &Path,
        template: &TemplateSpec,
    ) -> Result<StagedArtifact> {
        validate_manifest_hash(candidate_manifest_hash)?;
        let staged_root = self.staged_root(project_id, version_id);
        let temporary_root = staged_root.with_extension("tmp");
        if temporary_root.exists() {
            fs::remove_dir_all(&temporary_root)?;
        }
        fs::create_dir_all(&temporary_root)?;
        let mut source_files = Vec::new();
        let mut stack = vec![source_root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_symlink() {
                    return Err(anyhow!("artifact export rejects symbolic links"));
                }
                if file_type.is_dir() {
                    stack.push(entry.path());
                } else if file_type.is_file() {
                    source_files.push(entry.path());
                }
            }
        }
        source_files.sort();
        let mut manifest_files = Vec::with_capacity(source_files.len());
        for source in source_files {
            let relative = source.strip_prefix(source_root)?.to_path_buf();
            validate_relative_path(&relative)?;
            let output = temporary_root.join(&relative);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &output)?;
            let bytes = fs::metadata(&source)?.len();
            manifest_files.push(manifest_file(&relative, bytes, sha256_file(&source)?)?);
        }
        let staged = write_staged_manifest(
            &temporary_root,
            &staged_root,
            StagedManifestContext {
                project_id,
                version_id,
                candidate_manifest_hash,
                template_id: template.id.as_str(),
                template_version: template.version.as_str(),
                delivery: template.artifact_delivery,
            },
            manifest_files,
        )?;
        sync_object_tree(&self.runtime_storage_dir, &staged_root)?;
        Ok(staged)
    }

    pub fn garbage_collect(&self, publish: &ArtifactPublishRecord) -> Result<()> {
        if publish.status != ArtifactPublishStatus::GarbageCollectable {
            return Err(anyhow!("artifact publish is not garbage collectable"));
        }
        let staged = self.staged_root(&publish.project_id, &publish.version_id);
        delete_object_tree(&self.runtime_storage_dir, &staged)?;
        if staged.exists() {
            fs::remove_dir_all(&staged)?;
        }
        let immutable = Self::version_root(
            &self.runtime_storage_dir,
            &publish.project_id,
            &publish.version_id,
        );
        delete_object_tree(&self.runtime_storage_dir, &immutable)?;
        if immutable.exists() {
            fs::remove_dir_all(&immutable)?;
        }
        Ok(())
    }
}

#[async_trait]
impl ArtifactPublisher for FileArtifactPublisher {
    async fn stage(
        &self,
        project_id: &str,
        version_id: &str,
        candidate_manifest_hash: &str,
        files: Vec<ArtifactFile>,
    ) -> Result<StagedArtifact> {
        validate_manifest_hash(candidate_manifest_hash)?;
        let staged_root = self.staged_root(project_id, version_id);
        let temporary_root = staged_root.with_extension("tmp");
        if temporary_root.exists() {
            fs::remove_dir_all(&temporary_root)?;
        }
        fs::create_dir_all(&temporary_root)?;
        let mut manifest_files = Vec::with_capacity(files.len());
        for file in files {
            validate_relative_path(&file.path)?;
            let output = temporary_root.join(&file.path);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&output, &file.bytes)?;
            manifest_files.push(manifest_file(
                &file.path,
                file.bytes.len() as u64,
                sha256_hex(&file.bytes),
            )?);
        }
        let staged = write_staged_manifest(
            &temporary_root,
            &staged_root,
            StagedManifestContext {
                project_id,
                version_id,
                candidate_manifest_hash,
                template_id: "generic-static",
                template_version: "generic-static@1",
                delivery: ArtifactDeliverySpec::HOST_ROOT,
            },
            manifest_files,
        )?;
        sync_object_tree(&self.runtime_storage_dir, &staged_root)?;
        Ok(staged)
    }

    async fn promote(&self, staged: &StagedArtifact) -> Result<String> {
        let source = self.staged_root(&staged.project_id, &staged.version_id);
        let target = Self::version_root(
            &self.runtime_storage_dir,
            &staged.project_id,
            &staged.version_id,
        );
        if target.exists() {
            let existing_hash = artifact_manifest_hash_at(&target)?;
            if existing_hash != staged.artifact_manifest_hash {
                return Err(anyhow!(
                    "immutable artifact version already exists with different content"
                ));
            }
            sync_object_tree(&self.runtime_storage_dir, &target)?;
            delete_object_tree(&self.runtime_storage_dir, &source)?;
            if source.exists() {
                fs::remove_dir_all(&source)?;
            }
            return Ok(format!(
                "runtime://artifacts/{}/versions/{}",
                safe_segment(&staged.project_id),
                safe_segment(&staged.version_id)
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&source, &target)?;
        sync_object_tree(&self.runtime_storage_dir, &target)?;
        delete_object_tree(&self.runtime_storage_dir, &source)?;
        Ok(format!(
            "runtime://artifacts/{}/versions/{}",
            safe_segment(&staged.project_id),
            safe_segment(&staged.version_id)
        ))
    }

    async fn abort(&self, staged: &StagedArtifact) -> Result<()> {
        let source = self.staged_root(&staged.project_id, &staged.version_id);
        delete_object_tree(&self.runtime_storage_dir, &source)?;
        if source.exists() {
            fs::remove_dir_all(&source)?;
        }
        Ok(())
    }
}

fn validate_manifest_hash(candidate_manifest_hash: &str) -> Result<()> {
    if candidate_manifest_hash.len() != 64
        || !candidate_manifest_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(anyhow!("candidate manifest hash is invalid"));
    }
    Ok(())
}

fn write_staged_manifest(
    temporary_root: &Path,
    staged_root: &Path,
    context: StagedManifestContext<'_>,
    manifest_files: Vec<ArtifactManifestFile>,
) -> Result<StagedArtifact> {
    let manifest = ArtifactManifest::build(
        context.project_id,
        context.version_id,
        context.candidate_manifest_hash,
        context.template_id,
        context.template_version,
        context.delivery,
        manifest_files,
    )?;
    let artifact_manifest_hash = manifest.sha256()?;
    fs::write(
        temporary_root.join(ARTIFACT_MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    if staged_root.exists() {
        fs::remove_dir_all(staged_root)?;
    }
    fs::rename(temporary_root, staged_root)?;
    Ok(StagedArtifact {
        project_id: context.project_id.to_string(),
        version_id: context.version_id.to_string(),
        candidate_manifest_hash: context.candidate_manifest_hash.to_string(),
        artifact_manifest_hash,
        staged_uri: format!(
            "runtime://artifacts/{}/staged/{}",
            safe_segment(context.project_id),
            safe_segment(context.version_id)
        ),
        file_count: manifest.files.len(),
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(anyhow!("artifact file path is invalid: {}", path.display()));
    }
    Ok(())
}

fn normalized_relative_path(path: &Path) -> Result<String> {
    validate_relative_path(path)?;
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("artifact path must be valid UTF-8"))?
        .replace('\\', "/");
    if path.starts_with("./")
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == ".")
    {
        return Err(anyhow!("artifact path is not normalized: {path}"));
    }
    Ok(path)
}

fn read_canonical_json(path: &Path) -> Result<Vec<u8>> {
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    Ok(serde_json::to_vec(&value)?)
}

fn artifact_manifest_hash_at(root: &Path) -> Result<String> {
    let manifest: ArtifactManifest =
        serde_json::from_slice(&fs::read(root.join(ARTIFACT_MANIFEST_FILE))?)?;
    manifest.sha256()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "artifact-publisher-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn source_snapshot_fingerprint_uses_frozen_utf8_files_and_ignores_snapshot_metadata() {
        let files = vec![
            ArtifactFile {
                path: PathBuf::from("app/page.tsx"),
                bytes: b"export default function Page() {}".to_vec(),
            },
            ArtifactFile {
                path: PathBuf::from(".snapshot.json"),
                bytes: b"{\"createdAt\":\"first\"}".to_vec(),
            },
            ArtifactFile {
                path: PathBuf::from("app/icon.svg"),
                bytes: b"<svg/>".to_vec(),
            },
        ];
        let expected = source_snapshot_fingerprint(&files).unwrap();
        let mut reordered = files.into_iter().rev().collect::<Vec<_>>();
        reordered[1].bytes = b"{\"createdAt\":\"second\"}".to_vec();

        assert_eq!(source_snapshot_fingerprint(&reordered).unwrap(), expected);
        reordered[0].bytes = b"<svg><path/></svg>".to_vec();
        assert_ne!(source_snapshot_fingerprint(&reordered).unwrap(), expected);
    }

    #[tokio::test]
    async fn source_snapshot_is_immutable_and_preserves_binary_files() {
        let storage = temp_dir("source-snapshot");
        let publisher = FileArtifactPublisher::new(&storage);
        let files = vec![
            ArtifactFile {
                path: PathBuf::from("src/index.ts"),
                bytes: b"export const value = 1;".to_vec(),
            },
            ArtifactFile {
                path: PathBuf::from("public/logo.bin"),
                bytes: vec![0, 1, 2, 127, 128, 254, 255],
            },
        ];
        let uri = publisher
            .publish_source_snapshot("project/unsafe", "build-1", files.clone())
            .await
            .unwrap();
        assert_eq!(uri, "runtime://source-snapshots/project-unsafe/build-1");
        assert_eq!(
            FileArtifactPublisher::read_source_snapshot(&storage, "project/unsafe", "build-1")
                .unwrap()
                .into_iter()
                .map(|file| (file.path, file.bytes))
                .collect::<std::collections::BTreeMap<_, _>>(),
            files
                .clone()
                .into_iter()
                .map(|file| (file.path, file.bytes))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
        publisher
            .publish_source_snapshot("project/unsafe", "build-1", files)
            .await
            .unwrap();
        let different = publisher
            .publish_source_snapshot(
                "project/unsafe",
                "build-1",
                vec![ArtifactFile {
                    path: PathBuf::from("src/index.ts"),
                    bytes: b"changed".to_vec(),
                }],
            )
            .await
            .unwrap_err();
        assert!(different.to_string().contains("immutable source snapshot"));
    }

    #[tokio::test]
    async fn source_snapshot_read_rejects_tampered_bytes() {
        let storage = temp_dir("source-tamper");
        let publisher = FileArtifactPublisher::new(&storage);
        publisher
            .publish_source_snapshot(
                "project-1",
                "build-1",
                vec![ArtifactFile {
                    path: PathBuf::from("public/image.bin"),
                    bytes: vec![1, 2, 3, 4],
                }],
            )
            .await
            .unwrap();
        fs::write(
            FileArtifactPublisher::source_snapshot_root(&storage, "project-1", "build-1")
                .join("public/image.bin"),
            [9, 9, 9, 9],
        )
        .unwrap();
        let error = FileArtifactPublisher::read_source_snapshot(&storage, "project-1", "build-1")
            .unwrap_err();
        assert!(error.to_string().contains("integrity check failed"));
    }

    #[tokio::test]
    async fn garbage_collection_removes_staged_and_immutable_non_current_bytes() {
        let storage = temp_dir("garbage-collection");
        let publisher = FileArtifactPublisher::new(&storage);
        let staged = publisher
            .stage(
                "project-1",
                "version-1",
                &"a".repeat(64),
                vec![ArtifactFile {
                    path: PathBuf::from("index.html"),
                    bytes: b"artifact".to_vec(),
                }],
            )
            .await
            .unwrap();
        publisher.promote(&staged).await.unwrap();
        let now = Utc::now();
        let publish = ArtifactPublishRecord {
            id: "publish-1".to_string(),
            idempotency_key: "project-1/run-1/build-1".to_string(),
            project_id: "project-1".to_string(),
            run_id: "run-1".to_string(),
            build_id: "build-1".to_string(),
            version_id: "version-1".to_string(),
            sandbox_binding_id: None,
            pod_uid: None,
            candidate_manifest_hash: "a".repeat(64),
            artifact_manifest_hash: Some(staged.artifact_manifest_hash),
            source_snapshot_uri: "runtime://source-snapshots/project-1/build-1".to_string(),
            expected_current_version_id: None,
            status: ArtifactPublishStatus::GarbageCollectable,
            revision: 4,
            staged_uri: Some(staged.staged_uri),
            immutable_artifact_uri: Some(
                "runtime://artifacts/project-1/versions/version-1".to_string(),
            ),
            last_error: Some("CAS conflict".to_string()),
            created_at: now,
            updated_at: now,
            gc_after: Some(now),
        };
        publisher.garbage_collect(&publish).unwrap();
        assert!(!FileArtifactPublisher::version_root(&storage, "project-1", "version-1").exists());
    }
}
