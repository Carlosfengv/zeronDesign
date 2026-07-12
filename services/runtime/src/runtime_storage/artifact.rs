use crate::{artifact_manifest::ArtifactResolver, artifact_publisher::FileArtifactPublisher};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactReadRequest<'a> {
    pub project_id: &'a str,
    pub version_id: &'a str,
    pub artifact_path: &'a str,
    pub expected_manifest_hash: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactContent {
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub legacy_html_rewrite: bool,
    pub nosniff: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactReadError {
    NotFound(String),
    Conflict(String),
}

pub trait ArtifactStore: Send + Sync {
    fn read(&self, request: ArtifactReadRequest<'_>) -> Result<ArtifactContent, ArtifactReadError>;
}

#[derive(Debug, Clone)]
pub struct FileArtifactStore {
    runtime_storage_dir: PathBuf,
}

impl FileArtifactStore {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_storage_dir: runtime_storage_dir.into(),
        }
    }
}

impl ArtifactStore for FileArtifactStore {
    fn read(&self, request: ArtifactReadRequest<'_>) -> Result<ArtifactContent, ArtifactReadError> {
        let output_root = FileArtifactPublisher::version_root(
            &self.runtime_storage_dir,
            request.project_id,
            request.version_id,
        );
        if !output_root.is_dir() {
            return Err(ArtifactReadError::NotFound(format!(
                "immutable artifact output not found for version: {}",
                request.version_id
            )));
        }

        if let Some(expected_hash) = request.expected_manifest_hash {
            let resolver = ArtifactResolver::load_for_version(
                &output_root,
                expected_hash,
                request.project_id,
                request.version_id,
            )
            .map_err(conflict)?
            .ok_or_else(|| {
                ArtifactReadError::Conflict(format!(
                    "promoted artifact manifest is missing for version {}",
                    request.version_id
                ))
            })?;
            let resolved = resolver
                .resolve(request.artifact_path)
                .map_err(conflict)?
                .ok_or_else(|| {
                    ArtifactReadError::NotFound(format!(
                        "artifact not found: {}",
                        request.artifact_path
                    ))
                })?;
            return Ok(ArtifactContent {
                content_type: resolved.content_type,
                bytes: resolved.bytes,
                legacy_html_rewrite: false,
                nosniff: true,
            });
        }

        read_legacy_artifact(&output_root, request.artifact_path)
    }
}

fn read_legacy_artifact(
    output_root: &Path,
    artifact_path: &str,
) -> Result<ArtifactContent, ArtifactReadError> {
    let path = resolve_artifact_file(output_root, artifact_path)?;
    let content_type = content_type_for_path(&path).to_string();
    let bytes = fs::read(&path)
        .map_err(|_| ArtifactReadError::NotFound(format!("artifact not found: {artifact_path}")))?;
    Ok(ArtifactContent {
        legacy_html_rewrite: content_type.starts_with("text/html"),
        content_type,
        bytes,
        nosniff: false,
    })
}

fn resolve_artifact_file(
    output_root: &Path,
    artifact_path: &str,
) -> Result<PathBuf, ArtifactReadError> {
    let relative = artifact_path.trim().trim_start_matches('/');
    if relative.is_empty() {
        return static_artifact_path(output_root, &output_root.join("index.html"));
    }
    let requested = static_artifact_path(output_root, &output_root.join(relative))?;
    if requested.is_file() {
        return Ok(requested);
    }
    if requested.is_dir() {
        let index = requested.join("index.html");
        if index.is_file() {
            return Ok(index);
        }
    }
    if Path::new(relative).extension().is_none() {
        let html =
            static_artifact_path(output_root, &output_root.join(format!("{relative}.html")))?;
        if html.is_file() {
            return Ok(html);
        }
    }
    Err(ArtifactReadError::NotFound(format!(
        "artifact not found: {artifact_path}"
    )))
}

fn static_artifact_path(
    output_root: &Path,
    requested: &Path,
) -> Result<PathBuf, ArtifactReadError> {
    let root = fs::canonicalize(output_root).map_err(|_| {
        ArtifactReadError::NotFound("artifact output root is not readable".to_string())
    })?;
    let path = if requested.exists() {
        fs::canonicalize(requested)
            .map_err(|_| ArtifactReadError::NotFound("artifact path is not readable".to_string()))?
    } else {
        let parent = requested
            .parent()
            .ok_or_else(|| ArtifactReadError::NotFound("artifact path is invalid".to_string()))?;
        let parent = fs::canonicalize(parent).map_err(|_| {
            ArtifactReadError::NotFound("artifact parent path is not readable".to_string())
        })?;
        parent.join(
            requested.file_name().ok_or_else(|| {
                ArtifactReadError::NotFound("artifact path is invalid".to_string())
            })?,
        )
    };
    if !path.starts_with(&root) {
        return Err(ArtifactReadError::Conflict(
            "artifact path escapes project output".to_string(),
        ));
    }
    Ok(path)
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "text/html; charset=utf-8",
    }
}

fn conflict(error: anyhow::Error) -> ArtifactReadError {
    ArtifactReadError::Conflict(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, FileArtifactStore) {
        let root = std::env::temp_dir().join(format!(
            "runtime-artifact-store-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let version_root = FileArtifactPublisher::version_root(&root, "project-1", "version-1");
        fs::create_dir_all(&version_root).unwrap();
        fs::write(version_root.join("index.html"), b"<h1>artifact</h1>").unwrap();
        let store = FileArtifactStore::new(&root);
        (root, store)
    }

    #[test]
    fn legacy_artifact_is_read_through_the_file_adapter() {
        let (root, store) = fixture();
        let content = store
            .read(ArtifactReadRequest {
                project_id: "project-1",
                version_id: "version-1",
                artifact_path: "",
                expected_manifest_hash: None,
            })
            .unwrap();
        assert_eq!(content.bytes, b"<h1>artifact</h1>");
        assert!(content.legacy_html_rewrite);
        assert!(!content.nosniff);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_artifact_path_escape_fails_closed() {
        let (root, store) = fixture();
        fs::write(root.join("outside.html"), b"outside").unwrap();
        let error = store
            .read(ArtifactReadRequest {
                project_id: "project-1",
                version_id: "version-1",
                artifact_path: "../../../outside.html",
                expected_manifest_hash: None,
            })
            .unwrap_err();
        assert!(matches!(error, ArtifactReadError::Conflict(_)));
        let _ = fs::remove_dir_all(root);
    }
}
