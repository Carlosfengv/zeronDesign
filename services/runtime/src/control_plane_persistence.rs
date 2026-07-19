use anyhow::{anyhow, Context, Result};
use postgres::{Client, NoTls};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

const MIGRATION: &str = include_str!("../migrations/0001_postgres_control_plane_files.sql");

/// PostgreSQL is the authority for control-plane journals and checkpoints.
/// Files under `root` are a local cache retained for the existing domain stores;
/// generated artifacts and source blobs deliberately remain outside this mirror.
pub struct PostgresControlPlaneMirror {
    root: PathBuf,
    database_url: String,
    connection: Mutex<Option<Client>>,
}

impl std::fmt::Debug for PostgresControlPlaneMirror {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresControlPlaneMirror")
            .field("root", &self.root)
            .field("database_url", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl PostgresControlPlaneMirror {
    pub fn open(database_url: &str, root: impl Into<PathBuf>) -> Result<Option<Self>> {
        if !is_postgres_url(database_url) {
            return Ok(None);
        }
        let root = root.into();
        fs::create_dir_all(&root)?;
        let database_url_owned = database_url.to_string();
        let client = std::thread::spawn({
            let database_url = database_url_owned.clone();
            move || connect(&database_url)
        })
        .join()
        .map_err(|_| anyhow!("PostgreSQL control-plane startup thread panicked"))??;
        let mirror = Self {
            root,
            database_url: database_url_owned,
            connection: Mutex::new(Some(client)),
        };
        mirror.with_client(|client| {
            client.batch_execute(MIGRATION)?;
            Ok(())
        })?;
        mirror.restore_or_import()?;
        Ok(Some(mirror))
    }

    pub fn sync_file(&self, path: &Path) -> Result<()> {
        let relative = self.relative_control_plane_path(path)?;
        let bytes = fs::read(path)
            .with_context(|| format!("read control-plane cache {}", path.display()))?;
        let digest = sha256_hex(&bytes);
        self.with_client(|client| {
            client.execute(
                "INSERT INTO runtime_control_plane_files
                    (file_path, content, content_sha256, revision)
                 VALUES ($1, $2, $3, 1)
                 ON CONFLICT (file_path) DO UPDATE
                 SET content = EXCLUDED.content,
                     content_sha256 = EXCLUDED.content_sha256,
                     revision = runtime_control_plane_files.revision + 1,
                     updated_at = CURRENT_TIMESTAMP",
                &[&relative, &bytes, &digest],
            )?;
            Ok(())
        })
    }

    fn restore_or_import(&self) -> Result<()> {
        let rows = self.with_client(|client| {
            Ok(client.query(
                "SELECT file_path, content, content_sha256
                 FROM runtime_control_plane_files ORDER BY file_path",
                &[],
            )?)
        })?;
        if rows.is_empty() {
            for path in list_control_plane_files(&self.root)? {
                self.sync_file(&path)?;
            }
            return Ok(());
        }

        let mut authoritative = BTreeSet::new();
        let mut contents = BTreeMap::new();
        for row in rows {
            let relative: String = row.get(0);
            let bytes: Vec<u8> = row.get(1);
            let expected_sha256: String = row.get(2);
            validate_relative_path(&relative)?;
            if sha256_hex(&bytes) != expected_sha256 {
                return Err(anyhow!(
                    "PostgreSQL control-plane file digest mismatch: {relative}"
                ));
            }
            authoritative.insert(relative.clone());
            contents.insert(relative, bytes);
        }

        for cached in list_control_plane_files(&self.root)? {
            let relative = self.relative_control_plane_path(&cached)?;
            if !authoritative.contains(&relative) {
                fs::remove_file(&cached).with_context(|| {
                    format!("remove stale control-plane cache {}", cached.display())
                })?;
            }
        }
        for (relative, bytes) in contents {
            write_atomic(&self.root.join(relative), &bytes)?;
        }
        Ok(())
    }

    fn relative_control_plane_path(&self, path: &Path) -> Result<String> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            anyhow!(
                "control-plane path {} is outside {}",
                path.display(),
                self.root.display()
            )
        })?;
        let relative = relative
            .to_str()
            .ok_or_else(|| anyhow!("control-plane path is not UTF-8: {}", path.display()))?
            .replace('\\', "/");
        validate_relative_path(&relative)?;
        if !is_control_plane_relative_path(Path::new(&relative)) {
            return Err(anyhow!(
                "path is outside the PostgreSQL control-plane boundary: {relative}"
            ));
        }
        Ok(relative)
    }

    fn with_client<T: Send>(
        &self,
        operation: impl FnOnce(&mut Client) -> Result<T> + Send,
    ) -> Result<T> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let mut connection = self
                        .connection
                        .lock()
                        .map_err(|_| anyhow!("PostgreSQL control-plane lock poisoned"))?;
                    if connection
                        .as_mut()
                        .is_some_and(|client| client.simple_query("SELECT 1").is_err())
                    {
                        *connection = None;
                    }
                    if connection.is_none() {
                        *connection = Some(connect(&self.database_url)?);
                    }
                    let result = operation(connection.as_mut().ok_or_else(|| {
                        anyhow!("PostgreSQL control-plane connection unavailable")
                    })?);
                    if result.is_err()
                        && connection.as_ref().is_some_and(postgres::Client::is_closed)
                    {
                        *connection = None;
                    }
                    result
                })
                .join()
                .map_err(|_| anyhow!("PostgreSQL control-plane operation thread panicked"))?
        })
    }
}

pub fn is_postgres_url(database_url: &str) -> bool {
    database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")
}

fn connect(database_url: &str) -> Result<Client> {
    Client::connect(database_url, NoTls).context("connect Runtime control plane to PostgreSQL")
}

fn validate_relative_path(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !is_control_plane_relative_path(path)
    {
        return Err(anyhow!("invalid control-plane file path: {relative}"));
    }
    Ok(())
}

fn is_control_plane_relative_path(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return false;
    };
    if components.next().is_none() {
        return path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "json" | "jsonl"));
    }
    matches!(
        first.to_str(),
        Some("publication" | "work-releases" | "checkpoints" | "run-logs" | "conversation-items")
    ) && path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "json" | "jsonl"))
}

fn list_control_plane_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn visit(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let relative = path.strip_prefix(root)?;
            let first = relative.components().next();
            if matches!(
                first,
                Some(Component::Normal(name))
                    if matches!(name.to_str(), Some("publication" | "work-releases" | "checkpoints" | "run-logs" | "conversation-items"))
            ) {
                visit(root, &path, files)?;
            }
            continue;
        }
        let relative = path.strip_prefix(root)?;
        if is_control_plane_relative_path(relative) {
            files.push(path);
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("restore-{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_includes_control_plane_but_excludes_artifacts_and_source_blobs() {
        assert!(is_control_plane_relative_path(Path::new("runs.jsonl")));
        assert!(is_control_plane_relative_path(Path::new(
            "publication/publication-checkpoint.json"
        )));
        assert!(is_control_plane_relative_path(Path::new(
            "conversation-items/project/conversation-items.jsonl"
        )));
        assert!(!is_control_plane_relative_path(Path::new(
            "artifacts/project/versions/v1/index.html"
        )));
        assert!(!is_control_plane_relative_path(Path::new(
            "design-source-artifacts/source/source.md"
        )));
    }

    #[test]
    fn non_postgres_urls_keep_the_file_backend() {
        assert!(!is_postgres_url("sqlite://runtime.db"));
        assert!(!is_postgres_url("file:///tmp/runtime"));
        assert!(is_postgres_url("postgres://runtime@postgres/runtime"));
        assert!(is_postgres_url("postgresql://runtime@postgres/runtime"));
    }
}
