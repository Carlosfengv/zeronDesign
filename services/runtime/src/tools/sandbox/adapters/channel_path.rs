use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(super) fn workspace_channel_path(path: &Path, workspace_root: &Path) -> io::Result<String> {
    let workspace_root = normalize_path(workspace_root);
    let relative = if path.starts_with("/workspace") {
        path.strip_prefix("/workspace")
            .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "path outside workspace"))?
            .to_path_buf()
    } else if path.is_absolute() {
        let normalized = normalize_path(path);
        let comparable = if normalized.starts_with(&workspace_root) {
            normalized
        } else {
            canonicalize_existing_prefix(path)?
        };
        comparable
            .strip_prefix(&workspace_root)
            .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "path outside workspace"))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path outside workspace",
        ));
    }
    let relative = normalize_path(&relative);
    if relative.as_os_str().is_empty() {
        return Ok("/workspace".to_string());
    }
    Ok(format!(
        "/workspace/{}",
        relative.to_string_lossy().replace('\\', "/")
    ))
}

// Local macOS temp paths may be presented through /tmp while canonical paths use /private/tmp.
// Remote Kubernetes paths take the lexical branch above and never depend on host existence.
// remote-fs-boundary: allow-begin channel-local-path-alias-fallback
fn canonicalize_existing_prefix(path: &Path) -> io::Result<PathBuf> {
    if let Ok(real) = fs::canonicalize(path) {
        return Ok(real);
    }
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::<OsString>::new();
    loop {
        let Some(file_name) = ancestor.file_name() else {
            return Ok(normalize_path(path));
        };
        suffix.push(file_name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return Ok(normalize_path(path));
        };
        ancestor = parent.to_path_buf();
        if let Ok(mut real_parent) = fs::canonicalize(&ancestor) {
            for part in suffix.iter().rev() {
                real_parent.push(part);
            }
            return Ok(normalize_path(&real_parent));
        }
    }
}
// remote-fs-boundary: allow-end channel-local-path-alias-fallback

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
