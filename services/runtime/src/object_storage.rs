use anyhow::{anyhow, Context, Result};
use futures::TryStreamExt;
use object_store::{aws::AmazonS3Builder, path::Path as ObjectPath, ObjectStore, ObjectStoreExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    future::Future,
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};

static REGISTERED_MIRRORS: OnceLock<RwLock<BTreeMap<PathBuf, Arc<S3ObjectStorage>>>> =
    OnceLock::new();
const INITIALIZED_MARKER: &str = ".anydesign-object-store-v1";

pub struct S3ObjectStorage {
    root: PathBuf,
    prefix: String,
    store: Arc<dyn ObjectStore>,
}

impl std::fmt::Debug for S3ObjectStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3ObjectStorage")
            .field("root", &self.root)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct S3ObjectStorageConfig<'a> {
    pub url: &'a str,
    pub endpoint: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
    pub allow_http: bool,
}

impl S3ObjectStorage {
    pub fn open(
        config: S3ObjectStorageConfig<'_>,
        root: impl Into<PathBuf>,
    ) -> Result<Option<Arc<Self>>> {
        if !config.url.starts_with("s3://") {
            return Ok(None);
        }
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            rustls::crypto::ring::default_provider()
                .install_default()
                .map_err(|_| anyhow!("install Runtime rustls crypto provider"))?;
        }
        let (bucket, prefix) = parse_s3_url(config.url)?;
        let root = root.into();
        fs::create_dir_all(&root)?;
        let store = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(config.region)
            .with_access_key_id(config.access_key)
            .with_secret_access_key(config.secret_key)
            .with_endpoint(config.endpoint)
            .with_allow_http(config.allow_http)
            .with_virtual_hosted_style_request(false)
            .with_disable_tagging(true)
            .build()
            .context("configure S3-compatible Runtime object storage")?;
        let storage = Arc::new(Self {
            root: root.clone(),
            prefix,
            store: Arc::new(store),
        });
        storage.restore_or_import()?;
        REGISTERED_MIRRORS
            .get_or_init(Default::default)
            .write()
            .map_err(|_| anyhow!("object storage registry lock poisoned"))?
            .insert(root, Arc::clone(&storage));
        Ok(Some(storage))
    }

    pub fn sync_file(&self, path: &Path) -> Result<()> {
        let relative = self.relative_object_path(path)?;
        let bytes =
            fs::read(path).with_context(|| format!("read object cache {}", path.display()))?;
        let location = self.location(&relative)?;
        self.run(async move |store| {
            store.put(&location, bytes.into()).await?;
            Ok(())
        })
    }

    pub fn sync_tree(&self, directory: &Path) -> Result<()> {
        if !directory.exists() {
            return Ok(());
        }
        let files = list_files(directory)?;
        let mut objects = Vec::with_capacity(files.len());
        for file in files {
            let relative = self.relative_object_path(&file)?;
            objects.push((self.location(&relative)?, fs::read(file)?));
        }
        self.run(async move |store| {
            for (location, bytes) in objects {
                store.put(&location, bytes.into()).await?;
            }
            Ok(())
        })
    }

    pub fn delete_tree(&self, directory: &Path) -> Result<()> {
        let relative = directory
            .strip_prefix(&self.root)
            .map_err(|_| anyhow!("object cache path is outside Runtime storage"))?;
        validate_object_relative_path(relative, true)?;
        let prefix = self.location(&path_string(relative))?;
        self.run(async move |store| {
            let mut objects = store.list(Some(&prefix));
            let directory_prefix = format!("{}/", prefix.as_ref().trim_end_matches('/'));
            while let Some(object) = objects.try_next().await? {
                if object.location.as_ref().starts_with(&directory_prefix) {
                    store.delete(&object.location).await?;
                }
            }
            Ok(())
        })
    }

    fn restore_or_import(&self) -> Result<()> {
        let prefix = self.prefix_path()?;
        let remote = self.run(async move |store| {
            let mut objects = store.list(prefix.as_ref());
            let mut entries = Vec::new();
            while let Some(object) = objects.try_next().await? {
                let bytes = store.get(&object.location).await?.bytes().await?;
                entries.push((object.location.to_string(), bytes.to_vec()));
            }
            Ok(entries)
        })?;
        if remote.is_empty() {
            for directory in object_directories(&self.root) {
                self.sync_tree(&directory)?;
            }
            self.ensure_initialized_marker()?;
            return Ok(());
        }

        let prefix = normalized_prefix(&self.prefix);
        let marker = self.location(INITIALIZED_MARKER)?.to_string();
        let mut authoritative = BTreeSet::new();
        let mut contents = Vec::new();
        for (location, bytes) in remote {
            if location == marker {
                continue;
            }
            let relative = location
                .strip_prefix(&prefix)
                .ok_or_else(|| anyhow!("object key is outside configured prefix: {location}"))?
                .trim_start_matches('/');
            let relative_path = PathBuf::from(relative);
            validate_object_relative_path(&relative_path, false)?;
            authoritative.insert(path_string(&relative_path));
            contents.push((relative_path, bytes));
        }
        for directory in object_directories(&self.root) {
            for cached in list_files(&directory)? {
                let relative = cached.strip_prefix(&self.root)?;
                if !authoritative.contains(&path_string(relative)) {
                    fs::remove_file(cached)?;
                }
            }
        }
        for (relative, bytes) in contents {
            write_atomic(&self.root.join(relative), &bytes)?;
        }
        self.ensure_initialized_marker()?;
        Ok(())
    }

    fn ensure_initialized_marker(&self) -> Result<()> {
        let marker = self.location(INITIALIZED_MARKER)?;
        self.run(async move |store| {
            store
                .put(
                    &marker,
                    b"anydesign-runtime-object-storage@1\n".as_slice().into(),
                )
                .await?;
            Ok(())
        })
    }

    fn relative_object_path(&self, path: &Path) -> Result<String> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            anyhow!(
                "object cache path {} is outside {}",
                path.display(),
                self.root.display()
            )
        })?;
        validate_object_relative_path(relative, false)?;
        Ok(path_string(relative))
    }

    fn location(&self, relative: &str) -> Result<ObjectPath> {
        let key = if self.prefix.is_empty() {
            relative.to_string()
        } else {
            format!("{}/{}", self.prefix.trim_matches('/'), relative)
        };
        ObjectPath::parse(key).map_err(anyhow::Error::new)
    }

    fn prefix_path(&self) -> Result<Option<ObjectPath>> {
        if self.prefix.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ObjectPath::parse(self.prefix.trim_matches('/'))?))
        }
    }

    fn run<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        T: Send,
        F: FnOnce(Arc<dyn ObjectStore>) -> Fut + Send,
        Fut: Future<Output = Result<T>> + Send,
    {
        let store = Arc::clone(&self.store);
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?
                        .block_on(operation(store))
                })
                .join()
                .map_err(|_| anyhow!("object storage operation thread panicked"))?
        })
    }
}

pub fn sync_object_file(runtime_storage_dir: &Path, path: &Path) -> Result<()> {
    if let Some(storage) = registered(runtime_storage_dir)? {
        storage.sync_file(path)?;
    }
    Ok(())
}

pub fn sync_object_tree(runtime_storage_dir: &Path, directory: &Path) -> Result<()> {
    if let Some(storage) = registered(runtime_storage_dir)? {
        storage.sync_tree(directory)?;
    }
    Ok(())
}

pub fn delete_object_tree(runtime_storage_dir: &Path, directory: &Path) -> Result<()> {
    if let Some(storage) = registered(runtime_storage_dir)? {
        storage.delete_tree(directory)?;
    }
    Ok(())
}

fn registered(root: &Path) -> Result<Option<Arc<S3ObjectStorage>>> {
    let Some(registry) = REGISTERED_MIRRORS.get() else {
        return Ok(None);
    };
    Ok(registry
        .read()
        .map_err(|_| anyhow!("object storage registry lock poisoned"))?
        .get(root)
        .cloned())
}

fn parse_s3_url(url: &str) -> Result<(String, String)> {
    let remainder = url
        .strip_prefix("s3://")
        .ok_or_else(|| anyhow!("object storage URL must use s3://"))?;
    let (bucket, prefix) = remainder.split_once('/').unwrap_or((remainder, ""));
    if bucket.is_empty()
        || !bucket
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(anyhow!("object storage bucket is invalid"));
    }
    let prefix = prefix.trim_matches('/').to_string();
    if !prefix.is_empty() {
        validate_safe_components(Path::new(&prefix))?;
    }
    Ok((bucket.to_string(), prefix))
}

fn validate_object_relative_path(path: &Path, allow_directory: bool) -> Result<()> {
    validate_safe_components(path)?;
    let Some(Component::Normal(first)) = path.components().next() else {
        return Err(anyhow!("object path is empty"));
    };
    if !matches!(
        first.to_str(),
        Some(
            "artifacts"
                | "source-snapshots"
                | "validation-reports"
                | "acceptance-reports"
                | "screenshots"
        )
    ) {
        return Err(anyhow!("path is outside the Runtime object boundary"));
    }
    if !allow_directory && path.components().count() < 2 {
        return Err(anyhow!(
            "object path must identify a file below an object directory"
        ));
    }
    Ok(())
}

fn validate_safe_components(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(anyhow!("object path contains unsafe components"));
    }
    Ok(())
}

fn object_directories(root: &Path) -> Vec<PathBuf> {
    [
        "artifacts",
        "source-snapshots",
        "validation-reports",
        "acceptance-reports",
        "screenshots",
    ]
    .into_iter()
    .map(|directory| root.join(directory))
    .collect()
}

fn list_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !directory.exists() {
        return Ok(files);
    }
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(anyhow!("object cache rejects symbolic links"));
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("object-restore-{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalized_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_url_and_object_boundary_are_fail_closed() {
        assert_eq!(
            parse_s3_url("s3://runtime-artifacts/greenfield").unwrap(),
            ("runtime-artifacts".to_string(), "greenfield".to_string())
        );
        assert!(parse_s3_url("s3:///missing").is_err());
        assert!(validate_object_relative_path(
            Path::new("artifacts/project/versions/v1/index.html"),
            false
        )
        .is_ok());
        assert!(validate_object_relative_path(
            Path::new("artifacts/project/versions/v1/routes/account"),
            false
        )
        .is_ok());
        assert!(validate_object_relative_path(Path::new("artifacts"), false).is_err());
        assert!(validate_object_relative_path(Path::new("runs.jsonl"), false).is_err());
        assert!(
            validate_object_relative_path(Path::new("artifacts/project/../../secret"), false)
                .is_err()
        );
    }
}
