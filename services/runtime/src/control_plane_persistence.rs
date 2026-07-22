use anyhow::{anyhow, Context, Result};
use postgres::{Client, NoTls};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::mpsc,
};

const MIGRATION: &str = include_str!("../migrations/0001_postgres_control_plane_files.sql");

/// PostgreSQL is the authority for control-plane journals and checkpoints.
/// Files under `root` are a local cache retained for the existing domain stores;
/// generated artifacts and source blobs deliberately remain outside this mirror.
pub struct PostgresControlPlaneMirror {
    root: PathBuf,
    worker: mpsc::Sender<ControlPlaneRequest>,
}

struct ControlPlaneRow {
    relative: String,
    bytes: Vec<u8>,
    expected_sha256: String,
}

enum ControlPlaneRequest {
    SyncFile {
        relative: String,
        path: PathBuf,
        response: mpsc::Sender<Result<()>>,
    },
    Load {
        response: mpsc::Sender<Result<Vec<ControlPlaneRow>>>,
    },
}

impl std::fmt::Debug for PostgresControlPlaneMirror {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresControlPlaneMirror")
            .field("root", &self.root)
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
        let (worker, requests) = mpsc::channel();
        let (ready, startup) = mpsc::channel();
        std::thread::Builder::new()
            .name("control-plane-postgres".to_string())
            .spawn(move || control_plane_worker(database_url_owned, requests, ready))
            .context("spawn PostgreSQL control-plane worker")?;
        wait_for_worker(startup)?;
        let mirror = Self { root, worker };
        mirror.restore_or_import()?;
        Ok(Some(mirror))
    }

    pub fn sync_file(&self, path: &Path) -> Result<()> {
        let relative = self.relative_control_plane_path(path)?;
        let (response, result) = mpsc::channel();
        self.worker
            .send(ControlPlaneRequest::SyncFile {
                relative,
                path: path.to_path_buf(),
                response,
            })
            .map_err(|_| anyhow!("PostgreSQL control-plane worker stopped"))?;
        wait_for_worker(result)
    }

    fn restore_or_import(&self) -> Result<()> {
        let (response, result) = mpsc::channel();
        self.worker
            .send(ControlPlaneRequest::Load { response })
            .map_err(|_| anyhow!("PostgreSQL control-plane worker stopped"))?;
        let rows = wait_for_worker(result)?;
        if rows.is_empty() {
            for path in list_control_plane_files(&self.root)? {
                self.sync_file(&path)?;
            }
            return Ok(());
        }

        let mut authoritative = BTreeSet::new();
        let mut contents = BTreeMap::new();
        for row in rows {
            let relative = row.relative;
            let bytes = row.bytes;
            let expected_sha256 = row.expected_sha256;
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
}

fn control_plane_worker(
    database_url: String,
    requests: mpsc::Receiver<ControlPlaneRequest>,
    ready: mpsc::Sender<Result<()>>,
) {
    let mut connection = match connect(&database_url).and_then(|mut client| {
        client.batch_execute(MIGRATION)?;
        Ok(client)
    }) {
        Ok(client) => {
            if ready.send(Ok(())).is_err() {
                return;
            }
            Some(client)
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    for request in requests {
        let response_delivered = match request {
            ControlPlaneRequest::SyncFile {
                relative,
                path,
                response,
            } => {
                let result = with_worker_client(&database_url, &mut connection, |client| {
                    // Read only after this request reaches the serialized worker. This
                    // prevents an older snapshot from overwriting a newer concurrent append.
                    let bytes = fs::read(&path)
                        .with_context(|| format!("read control-plane cache {}", path.display()))?;
                    let digest = sha256_hex(&bytes);
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
                });
                response.send(result).is_ok()
            }
            ControlPlaneRequest::Load { response } => {
                let result = with_worker_client(&database_url, &mut connection, |client| {
                    client
                        .query(
                            "SELECT file_path, content, content_sha256
                             FROM runtime_control_plane_files ORDER BY file_path",
                            &[],
                        )?
                        .into_iter()
                        .map(|row| {
                            Ok(ControlPlaneRow {
                                relative: row.try_get(0)?,
                                bytes: row.try_get(1)?,
                                expected_sha256: row.try_get(2)?,
                            })
                        })
                        .collect()
                });
                response.send(result).is_ok()
            }
        };
        if !response_delivered {
            break;
        }
    }
}

fn with_worker_client<T>(
    database_url: &str,
    connection: &mut Option<Client>,
    operation: impl FnOnce(&mut Client) -> Result<T>,
) -> Result<T> {
    if connection.as_ref().is_some_and(Client::is_closed) {
        *connection = None;
    }
    if connection.is_none() {
        *connection = Some(connect(database_url)?);
    }
    let result = operation(
        connection
            .as_mut()
            .ok_or_else(|| anyhow!("PostgreSQL control-plane connection unavailable"))?,
    );
    if result.is_err() && connection.as_ref().is_some_and(Client::is_closed) {
        *connection = None;
    }
    result
}

fn wait_for_worker<T>(receiver: mpsc::Receiver<Result<T>>) -> Result<T> {
    let wait = || {
        receiver
            .recv()
            .map_err(|_| anyhow!("PostgreSQL control-plane worker stopped before responding"))?
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(wait)
        }
        _ => wait(),
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
