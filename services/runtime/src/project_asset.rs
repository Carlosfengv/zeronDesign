use crate::visual_contracts::{ProjectAsset, PROJECT_ASSET_SCHEMA};
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug)]
pub enum ProjectAssetError {
    Invalid(String),
    NotFound(String),
    Conflict(String),
    Storage(String),
}

impl std::fmt::Display for ProjectAssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid ProjectAsset: {message}"),
            Self::NotFound(message) => write!(formatter, "ProjectAsset not found: {message}"),
            Self::Conflict(message) => write!(formatter, "ProjectAsset conflict: {message}"),
            Self::Storage(message) => write!(formatter, "ProjectAsset storage failure: {message}"),
        }
    }
}

impl std::error::Error for ProjectAssetError {}

#[derive(Debug)]
pub struct ProjectAssetStore {
    log_path: PathBuf,
    assets: Mutex<HashMap<String, ProjectAsset>>,
}

impl ProjectAssetStore {
    pub fn open(runtime_storage_dir: impl AsRef<Path>) -> Result<Self, ProjectAssetError> {
        let root = runtime_storage_dir.as_ref().join("project-assets");
        fs::create_dir_all(&root).map_err(storage)?;
        let log_path = root.join("assets.jsonl");
        let assets = read_jsonl::<ProjectAsset>(&log_path)?
            .into_iter()
            .map(|asset| (asset.asset_id.clone(), asset))
            .collect();
        Ok(Self {
            log_path,
            assets: Mutex::new(assets),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        project_id: &str,
        source_artifact_id: String,
        source: crate::visual_contracts::ProjectAssetSource,
        target_path: String,
        content_hash: String,
        license: String,
        provenance: Value,
        width: u32,
        height: u32,
        alt_text: String,
        created_by_run_id: Option<String>,
    ) -> Result<ProjectAsset, ProjectAssetError> {
        if content_hash.len() != 64 || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProjectAssetError::Invalid(
                "contentHash must be SHA-256 hex".to_string(),
            ));
        }
        let existing = self
            .assets
            .lock()
            .unwrap()
            .values()
            .find(|asset| {
                asset.project_id == project_id
                    && asset.content_hash == content_hash
                    && asset.target_path == target_path
            })
            .cloned();
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let asset = ProjectAsset {
            schema_version: PROJECT_ASSET_SCHEMA.to_string(),
            asset_id: format!(
                "project-asset-{}-{:016x}",
                &content_hash[..16],
                rand::random::<u64>()
            ),
            project_id: project_id.to_string(),
            source_artifact_id,
            source,
            target_path,
            content_hash,
            license,
            provenance,
            width,
            height,
            alt_text,
            created_by_run_id,
            created_at: Utc::now(),
        };
        asset.validate().map_err(ProjectAssetError::Invalid)?;
        append_jsonl(&self.log_path, &asset)?;
        self.assets
            .lock()
            .unwrap()
            .insert(asset.asset_id.clone(), asset.clone());
        Ok(asset)
    }

    pub fn get(&self, asset_id: &str) -> Option<ProjectAsset> {
        self.assets.lock().unwrap().get(asset_id).cloned()
    }

    pub fn list_project(&self, project_id: &str) -> Vec<ProjectAsset> {
        let mut assets = self
            .assets
            .lock()
            .unwrap()
            .values()
            .filter(|asset| asset.project_id == project_id)
            .cloned()
            .collect::<Vec<_>>();
        assets.sort_by_key(|asset| asset.created_at);
        assets
    }
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<(), ProjectAssetError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(storage)?;
    serde_json::to_writer(&mut file, value).map_err(storage)?;
    file.write_all(b"\n").map_err(storage)?;
    file.sync_data().map_err(storage)
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, ProjectAssetError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(storage(error)),
    };
    BufReader::new(file)
        .lines()
        .filter_map(|line| match line {
            Ok(line) if line.trim().is_empty() => None,
            other => Some(other),
        })
        .map(|line| {
            let line = line.map_err(storage)?;
            serde_json::from_str(&line).map_err(storage)
        })
        .collect()
}

fn storage(error: impl std::fmt::Display) -> ProjectAssetError {
    ProjectAssetError::Storage(error.to_string())
}
