use crate::{
    types::sha256_hex,
    visual_contracts::{
        DraftSnapshotRetentionState, VisualArtifact, VisualArtifactOrigin, VisualMediaType,
        VISUAL_ARTIFACT_SCHEMA,
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use image::{GenericImageView, ImageFormat, ImageReader};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

pub const MAX_VISUAL_ARTIFACT_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_VISUAL_ARTIFACT_DIMENSION: u32 = 8_192;
pub const MAX_VISUAL_ARTIFACT_PIXELS: u64 = 40_000_000;

#[derive(Debug, Clone)]
pub struct NormalizedVisualImage {
    pub bytes: Vec<u8>,
    pub media_type: VisualMediaType,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct VisualArtifactStore {
    root: PathBuf,
}

impl VisualArtifactStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("metadata"))?;
        fs::create_dir_all(root.join("blobs"))?;
        Ok(Self { root })
    }

    pub fn create_upload(
        &self,
        project_id: &str,
        input: &[u8],
        origin_metadata: BTreeMap<String, Value>,
    ) -> Result<VisualArtifact> {
        self.create(
            project_id,
            input,
            VisualArtifactOrigin::Upload,
            origin_metadata,
        )
    }

    pub fn create_browser_capture(
        &self,
        project_id: &str,
        input: &[u8],
        origin_metadata: BTreeMap<String, Value>,
    ) -> Result<VisualArtifact> {
        self.create(
            project_id,
            input,
            VisualArtifactOrigin::Browser,
            origin_metadata,
        )
    }

    pub fn create_generated(
        &self,
        project_id: &str,
        input: &[u8],
        origin_metadata: BTreeMap<String, Value>,
    ) -> Result<VisualArtifact> {
        self.create(
            project_id,
            input,
            VisualArtifactOrigin::Generated,
            origin_metadata,
        )
    }

    fn create(
        &self,
        project_id: &str,
        input: &[u8],
        origin: VisualArtifactOrigin,
        origin_metadata: BTreeMap<String, Value>,
    ) -> Result<VisualArtifact> {
        if project_id.trim().is_empty() {
            return Err(anyhow!("projectId is required"));
        }
        let normalized = normalize_visual_image(input)?;
        let now = Utc::now();
        let id = format!(
            "visual-{}-{}-{:016x}",
            &normalized.sha256[..16],
            now.timestamp_micros(),
            rand::random::<u64>()
        );
        let blob_path = self.blob_path(&normalized.sha256);
        if !blob_path.exists() {
            atomic_write(&blob_path, &normalized.bytes)?;
        }
        let artifact = VisualArtifact {
            schema_version: VISUAL_ARTIFACT_SCHEMA.to_string(),
            id: id.clone(),
            project_id: project_id.to_string(),
            media_type: normalized.media_type,
            size_bytes: normalized.bytes.len() as u64,
            width: normalized.width,
            height: normalized.height,
            sha256: normalized.sha256,
            storage_uri: format!("runtime://visual-artifacts/{id}/content"),
            origin,
            origin_metadata,
            created_at: now,
            retention_state: DraftSnapshotRetentionState::Active,
            delete_after: None,
        };
        artifact
            .validate()
            .map_err(|error| anyhow!("invalid VisualArtifact: {error}"))?;
        let metadata = serde_json::to_vec_pretty(&artifact)?;
        atomic_write(&self.metadata_path(&id)?, &metadata)?;
        Ok(artifact)
    }

    pub fn get(&self, artifact_id: &str) -> Result<Option<VisualArtifact>> {
        let path = self.metadata_path(artifact_id)?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let artifact: VisualArtifact = serde_json::from_slice(&bytes)?;
        artifact
            .validate()
            .map_err(|error| anyhow!("invalid persisted VisualArtifact: {error}"))?;
        Ok(Some(artifact))
    }

    pub fn list(&self) -> Result<Vec<VisualArtifact>> {
        let mut artifacts = Vec::new();
        for entry in fs::read_dir(self.root.join("metadata"))? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let artifact: VisualArtifact = serde_json::from_slice(&fs::read(path)?)?;
            artifact
                .validate()
                .map_err(|error| anyhow!("invalid persisted VisualArtifact: {error}"))?;
            artifacts.push(artifact);
        }
        artifacts.sort_by_key(|artifact| artifact.created_at);
        Ok(artifacts)
    }

    pub fn request_delete(&self, artifact_id: &str, protected: bool) -> Result<VisualArtifact> {
        let mut artifact = self
            .get(artifact_id)?
            .ok_or_else(|| anyhow!("VisualArtifact not found: {artifact_id}"))?;
        if protected {
            artifact.retention_state = DraftSnapshotRetentionState::Protected;
            artifact.delete_after = None;
        } else {
            artifact.retention_state = DraftSnapshotRetentionState::DeletionPending;
            artifact.delete_after = Some(Utc::now());
        }
        artifact
            .validate()
            .map_err(|error| anyhow!("invalid VisualArtifact deletion state: {error}"))?;
        atomic_write(
            &self.metadata_path(artifact_id)?,
            &serde_json::to_vec_pretty(&artifact)?,
        )?;
        Ok(artifact)
    }

    pub fn purge_deletion_pending(&self, now: chrono::DateTime<Utc>) -> Result<Vec<String>> {
        let artifacts = self.list()?;
        let mut purged = Vec::new();
        for artifact in artifacts.iter().filter(|artifact| {
            artifact.retention_state == DraftSnapshotRetentionState::DeletionPending
                && artifact
                    .delete_after
                    .is_some_and(|delete_after| delete_after <= now)
        }) {
            fs::remove_file(self.metadata_path(&artifact.id)?)?;
            let shared_blob_is_referenced = artifacts.iter().any(|candidate| {
                candidate.id != artifact.id
                    && candidate.sha256 == artifact.sha256
                    && !purged.iter().any(|id| id == &candidate.id)
            });
            if !shared_blob_is_referenced {
                match fs::remove_file(self.blob_path(&artifact.sha256)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            purged.push(artifact.id.clone());
        }
        Ok(purged)
    }

    pub fn read_content(&self, artifact_id: &str) -> Result<Vec<u8>> {
        let artifact = self
            .get(artifact_id)?
            .ok_or_else(|| anyhow!("VisualArtifact not found: {artifact_id}"))?;
        let bytes = fs::read(self.blob_path(&artifact.sha256))?;
        if bytes.len() as u64 != artifact.size_bytes || sha256_hex(&bytes) != artifact.sha256 {
            return Err(anyhow!(
                "VisualArtifact integrity check failed: {artifact_id}"
            ));
        }
        let normalized = normalize_visual_image(&bytes)?;
        if normalized.media_type != artifact.media_type
            || normalized.width != artifact.width
            || normalized.height != artifact.height
            || normalized.sha256 != artifact.sha256
        {
            return Err(anyhow!(
                "VisualArtifact decoded metadata mismatch: {artifact_id}"
            ));
        }
        Ok(bytes)
    }

    fn metadata_path(&self, artifact_id: &str) -> Result<PathBuf> {
        if artifact_id.is_empty()
            || !artifact_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(anyhow!("invalid VisualArtifact id"));
        }
        Ok(self
            .root
            .join("metadata")
            .join(format!("{artifact_id}.json")))
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        self.root.join("blobs").join(format!("{sha256}.png"))
    }
}

pub fn normalize_visual_image(input: &[u8]) -> Result<NormalizedVisualImage> {
    if input.is_empty() || input.len() > MAX_VISUAL_ARTIFACT_INPUT_BYTES {
        return Err(anyhow!(
            "visual image must contain 1..={MAX_VISUAL_ARTIFACT_INPUT_BYTES} compressed bytes"
        ));
    }
    let format = ImageReader::new(Cursor::new(input))
        .with_guessed_format()?
        .format()
        .ok_or_else(|| anyhow!("unable to detect visual image format"))?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(anyhow!("visual image format must be PNG, JPEG, or WebP"));
    }
    let dimensions = ImageReader::with_format(Cursor::new(input), format).into_dimensions()?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_VISUAL_ARTIFACT_DIMENSION
        || dimensions.1 > MAX_VISUAL_ARTIFACT_DIMENSION
        || u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_VISUAL_ARTIFACT_PIXELS
    {
        return Err(anyhow!("visual image dimensions exceed Runtime limits"));
    }
    let image = ImageReader::with_format(Cursor::new(input), format).decode()?;
    if image.dimensions() != dimensions {
        return Err(anyhow!("visual image dimensions changed during decode"));
    }
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png)?;
    let bytes = output.into_inner();
    Ok(NormalizedVisualImage {
        sha256: sha256_hex(&bytes),
        bytes,
        media_type: VisualMediaType::Png,
        width: dimensions.0,
        height: dimensions.1,
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_micros()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_pixel_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0, 255]).unwrap();
        drop(writer);
        bytes
    }

    #[test]
    fn normalized_upload_is_content_addressed_and_revalidated_on_read() {
        let root = std::env::temp_dir().join(format!(
            "zerondesign-visual-store-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let store = VisualArtifactStore::open(&root).unwrap();
        let artifact = store
            .create_upload("project-1", &one_pixel_png(), BTreeMap::new())
            .unwrap();
        assert_eq!(artifact.media_type, VisualMediaType::Png);
        assert_eq!((artifact.width, artifact.height), (1, 1));
        assert_eq!(store.get(&artifact.id).unwrap(), Some(artifact.clone()));
        assert_eq!(
            sha256_hex(&store.read_content(&artifact.id).unwrap()),
            artifact.sha256
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_bytes_are_rejected() {
        assert!(normalize_visual_image(b"not-an-image").is_err());
    }
}
