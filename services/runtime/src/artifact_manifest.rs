use crate::{
    artifact_routes::{ArtifactRouteManifest, ARTIFACT_ROUTE_MANIFEST_FILE},
    types::sha256_hex,
};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

pub const ARTIFACT_MANIFEST_FILE: &str = ".anydesign-artifact-manifest.json";
pub const ARTIFACT_MANIFEST_SCHEMA: &str = "artifact-manifest@1";
pub const ARTIFACT_CONTENT_TYPES: &[&str] = &[
    "text/html; charset=utf-8",
    "text/css; charset=utf-8",
    "text/javascript; charset=utf-8",
    "application/json; charset=utf-8",
    "text/plain; charset=utf-8",
    "application/xml; charset=utf-8",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/avif",
    "image/x-icon",
    "font/woff",
    "font/woff2",
    "font/ttf",
    "font/otf",
    "application/wasm",
    "application/manifest+json",
    "application/octet-stream",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactMountSpec {
    pub url_prefix: &'static str,
    pub artifact_path: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactDeliverySpec {
    pub mounts: &'static [ArtifactMountSpec],
}

const HOST_ROOT_MOUNTS: &[ArtifactMountSpec] = &[ArtifactMountSpec {
    url_prefix: "/",
    artifact_path: "",
}];

impl ArtifactDeliverySpec {
    pub const HOST_ROOT: Self = Self {
        mounts: HOST_ROOT_MOUNTS,
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestMount {
    pub url_prefix: String,
    pub artifact_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema_version: String,
    pub project_id: String,
    pub version_id: String,
    pub candidate_manifest_hash: String,
    pub template_id: String,
    pub template_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_route_manifest_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_route_manifest_hash: Option<String>,
    pub mounts: Vec<ArtifactManifestMount>,
    pub files: Vec<ArtifactManifestFile>,
}

impl ArtifactManifest {
    pub fn build(
        project_id: &str,
        version_id: &str,
        candidate_manifest_hash: &str,
        template_id: &str,
        template_version: &str,
        delivery: ArtifactDeliverySpec,
        mut files: Vec<ArtifactManifestFile>,
    ) -> Result<Self> {
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let mut mounts = delivery
            .mounts
            .iter()
            .map(|mount| ArtifactManifestMount {
                url_prefix: mount.url_prefix.to_string(),
                artifact_path: mount.artifact_path.to_string(),
            })
            .collect::<Vec<_>>();
        mounts.sort_by(|left, right| left.url_prefix.cmp(&right.url_prefix));
        let route_manifest_entry = files
            .iter()
            .find(|file| file.path == ARTIFACT_ROUTE_MANIFEST_FILE);
        let manifest = Self {
            schema_version: ARTIFACT_MANIFEST_SCHEMA.to_string(),
            project_id: project_id.to_string(),
            version_id: version_id.to_string(),
            candidate_manifest_hash: candidate_manifest_hash.to_string(),
            template_id: template_id.to_string(),
            template_version: template_version.to_string(),
            artifact_route_manifest_path: route_manifest_entry
                .map(|_| ARTIFACT_ROUTE_MANIFEST_FILE.to_string()),
            artifact_route_manifest_hash: route_manifest_entry.map(|entry| entry.sha256.clone()),
            mounts,
            files,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ARTIFACT_MANIFEST_SCHEMA {
            return Err(anyhow!("unsupported artifact manifest schema"));
        }
        for (field, value) in [
            ("projectId", self.project_id.as_str()),
            ("versionId", self.version_id.as_str()),
            ("templateId", self.template_id.as_str()),
            ("templateVersion", self.template_version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(anyhow!("artifact manifest {field} must not be empty"));
            }
        }
        validate_sha256(&self.candidate_manifest_hash, "candidate manifest hash")?;
        match (
            self.artifact_route_manifest_path.as_deref(),
            self.artifact_route_manifest_hash.as_deref(),
        ) {
            (Some(path), Some(hash)) => {
                if path != ARTIFACT_ROUTE_MANIFEST_FILE {
                    return Err(anyhow!("artifact route manifest path is invalid"));
                }
                validate_sha256(hash, "artifact route manifest hash")?;
                if !self
                    .files
                    .iter()
                    .any(|file| file.path == path && file.sha256 == hash)
                {
                    return Err(anyhow!("artifact route manifest binding is invalid"));
                }
            }
            (None, None) => {}
            _ => return Err(anyhow!("artifact route manifest binding is incomplete")),
        }
        if self.mounts.is_empty() {
            return Err(anyhow!("artifact manifest requires at least one mount"));
        }
        let mut mount_keys = HashSet::new();
        for mount in &self.mounts {
            validate_url_prefix(&mount.url_prefix)?;
            if !mount.artifact_path.is_empty() {
                validate_artifact_path(&mount.artifact_path)?;
            }
            if !mount_keys.insert(mount.url_prefix.to_ascii_lowercase()) {
                return Err(anyhow!("artifact manifest contains duplicate mounts"));
            }
        }

        if self.files.is_empty() {
            return Err(anyhow!("artifact manifest requires at least one file"));
        }
        let mut paths = HashSet::new();
        let mut previous: Option<&str> = None;
        for file in &self.files {
            validate_artifact_path(&file.path)?;
            validate_sha256(&file.sha256, "file hash")?;
            let expected_content_type = content_type_for_artifact_path(Path::new(&file.path))?;
            if file.content_type != expected_content_type {
                return Err(anyhow!(
                    "artifact content type does not match path: {}",
                    file.path
                ));
            }
            if !paths.insert(file.path.to_ascii_lowercase()) {
                return Err(anyhow!("artifact manifest contains colliding paths"));
            }
            if previous.is_some_and(|previous| previous >= file.path.as_str()) {
                return Err(anyhow!(
                    "artifact manifest files must be canonically sorted"
                ));
            }
            previous = Some(&file.path);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn sha256(&self) -> Result<String> {
        Ok(sha256_hex(&self.canonical_bytes()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtifact {
    pub path: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct ArtifactResolver {
    root: PathBuf,
    manifest: ArtifactManifest,
    route_manifest: Option<ArtifactRouteManifest>,
}

impl ArtifactResolver {
    pub fn load(root: &Path, expected_manifest_hash: &str) -> Result<Option<Self>> {
        let path = root.join(ARTIFACT_MANIFEST_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        validate_sha256(expected_manifest_hash, "expected artifact manifest hash")?;
        let manifest: ArtifactManifest = serde_json::from_slice(&fs::read(path)?)?;
        manifest.validate()?;
        if manifest.sha256()? != expected_manifest_hash {
            return Err(anyhow!("artifact manifest integrity check failed"));
        }
        let route_manifest = load_route_manifest(root, &manifest)?;
        Ok(Some(Self {
            root: root.to_path_buf(),
            manifest,
            route_manifest,
        }))
    }

    pub fn load_for_version(
        root: &Path,
        expected_manifest_hash: &str,
        project_id: &str,
        version_id: &str,
    ) -> Result<Option<Self>> {
        let resolver = Self::load(root, expected_manifest_hash)?;
        if let Some(resolver) = resolver.as_ref() {
            if resolver.manifest.project_id != project_id
                || resolver.manifest.version_id != version_id
            {
                return Err(anyhow!("artifact manifest identity check failed"));
            }
        }
        Ok(resolver)
    }

    pub fn resolve(&self, request_path: &str) -> Result<Option<ResolvedArtifact>> {
        if let Some(route_manifest) = &self.route_manifest {
            return self.resolve_manifest_route(request_path, route_manifest);
        }
        let request_path = normalize_request_path(request_path)?;
        let Some(relative) = self.mount_relative_path(&request_path) else {
            return Ok(None);
        };
        for candidate in resolution_candidates(&relative) {
            let Some(entry) = self
                .manifest
                .files
                .iter()
                .find(|entry| entry.path == candidate)
            else {
                continue;
            };
            return self.read_entry(entry).map(Some);
        }
        Ok(None)
    }

    pub fn verify_all(&self) -> Result<Vec<ResolvedArtifact>> {
        let mut verified = Vec::with_capacity(self.manifest.files.len());
        for entry in &self.manifest.files {
            verified.push(self.read_entry(entry)?);
        }
        Ok(verified)
    }

    pub fn manifest(&self) -> &ArtifactManifest {
        &self.manifest
    }

    fn resolve_manifest_route(
        &self,
        request_path: &str,
        route_manifest: &ArtifactRouteManifest,
    ) -> Result<Option<ResolvedArtifact>> {
        let route = normalize_manifest_request_route(request_path)?;
        if let Some(target) = route_manifest.resolve(&route) {
            let entry = self
                .manifest
                .files
                .iter()
                .find(|entry| entry.path == target.file && entry.sha256 == target.sha256)
                .ok_or_else(|| {
                    anyhow!("artifact route target is not bound to the artifact manifest")
                })?;
            return self.read_entry(entry).map(Some);
        }

        let relative = route.trim_start_matches('/');
        if relative.is_empty() || relative.ends_with('/') {
            return Ok(None);
        }
        validate_artifact_path(relative)?;
        let Some(entry) = self.manifest.files.iter().find(|entry| {
            entry.path == relative
                && entry.path != ARTIFACT_ROUTE_MANIFEST_FILE
                && !entry.content_type.starts_with("text/html")
        }) else {
            return Ok(None);
        };
        self.read_entry(entry).map(Some)
    }

    fn read_entry(&self, entry: &ArtifactManifestFile) -> Result<ResolvedArtifact> {
        let path = self.root.join(&entry.path);
        let canonical_root = fs::canonicalize(&self.root)?;
        let canonical_path = fs::canonicalize(&path)?;
        if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
            return Err(anyhow!("artifact path escapes immutable root"));
        }
        let bytes = fs::read(canonical_path)?;
        if bytes.len() as u64 != entry.bytes || sha256_hex(&bytes) != entry.sha256 {
            return Err(anyhow!(
                "artifact file integrity check failed for {}",
                entry.path
            ));
        }
        Ok(ResolvedArtifact {
            path: entry.path.clone(),
            content_type: entry.content_type.clone(),
            bytes,
        })
    }

    fn mount_relative_path(&self, request_path: &str) -> Option<String> {
        self.manifest
            .mounts
            .iter()
            .filter_map(|mount| {
                let prefix = mount.url_prefix.trim_start_matches('/');
                let suffix = if prefix.is_empty() {
                    request_path
                } else if request_path == prefix {
                    ""
                } else {
                    request_path.strip_prefix(&format!("{prefix}/"))?
                };
                let relative = [mount.artifact_path.as_str(), suffix]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("/");
                Some((mount.url_prefix.len(), relative))
            })
            .max_by_key(|(prefix_len, _)| *prefix_len)
            .map(|(_, relative)| relative)
    }
}

fn load_route_manifest(
    root: &Path,
    manifest: &ArtifactManifest,
) -> Result<Option<ArtifactRouteManifest>> {
    let (Some(path), Some(expected_hash)) = (
        manifest.artifact_route_manifest_path.as_deref(),
        manifest.artifact_route_manifest_hash.as_deref(),
    ) else {
        return Ok(None);
    };
    let bytes = fs::read(root.join(path))?;
    if sha256_hex(&bytes) != expected_hash {
        return Err(anyhow!("artifact route manifest integrity check failed"));
    }
    let route_manifest: ArtifactRouteManifest = serde_json::from_slice(&bytes)?;
    route_manifest
        .validate()
        .map_err(|error| anyhow!(error.to_string()))?;
    for target in route_manifest.routes.values() {
        let bound = manifest.files.iter().any(|entry| {
            entry.path == target.file
                && entry.sha256 == target.sha256
                && entry.content_type == target.content_type
        });
        if !bound {
            return Err(anyhow!(
                "artifact route target is not bound to the artifact manifest: {}",
                target.file
            ));
        }
    }
    Ok(Some(route_manifest))
}

fn normalize_manifest_request_route(path: &str) -> Result<String> {
    let path = path.trim();
    if path.contains('?')
        || path.contains('#')
        || path.contains('\\')
        || path.contains('\0')
        || path.contains('%')
        || path.contains("//")
    {
        return Err(anyhow!("artifact request route is invalid"));
    }
    let route = if path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", path.trim_start_matches('/'))
    };
    if route
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(anyhow!("artifact request route is invalid"));
    }
    Ok(route)
}

pub fn manifest_file(path: &Path, bytes: u64, sha256: String) -> Result<ArtifactManifestFile> {
    let path = normalized_artifact_path(path)?;
    validate_artifact_path(&path)?;
    Ok(ArtifactManifestFile {
        content_type: content_type_for_artifact_path(Path::new(&path))?.to_string(),
        path,
        bytes,
        sha256,
    })
}

pub fn content_type_for_artifact_path(path: &Path) -> Result<&'static str> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => Ok("text/html; charset=utf-8"),
        Some("css") => Ok("text/css; charset=utf-8"),
        Some("js") | Some("mjs") => Ok("text/javascript; charset=utf-8"),
        Some("json") | Some("map") => Ok("application/json; charset=utf-8"),
        Some("txt") => Ok("text/plain; charset=utf-8"),
        Some("xml") => Ok("application/xml; charset=utf-8"),
        Some("svg") => Ok("image/svg+xml"),
        Some("png") => Ok("image/png"),
        Some("jpg") | Some("jpeg") => Ok("image/jpeg"),
        Some("webp") => Ok("image/webp"),
        Some("gif") => Ok("image/gif"),
        Some("avif") => Ok("image/avif"),
        Some("ico") => Ok("image/x-icon"),
        Some("woff") => Ok("font/woff"),
        Some("woff2") => Ok("font/woff2"),
        Some("ttf") => Ok("font/ttf"),
        Some("otf") => Ok("font/otf"),
        Some("wasm") => Ok("application/wasm"),
        Some("webmanifest") => Ok("application/manifest+json"),
        Some("bin") => Ok("application/octet-stream"),
        _ => Err(anyhow!(
            "artifact content type is not allowlisted: {}",
            path.display()
        )),
    }
}

fn resolution_candidates(relative: &str) -> Vec<String> {
    if relative.is_empty() {
        return vec!["index.html".to_string()];
    }
    let mut candidates = vec![relative.to_string(), format!("{relative}/index.html")];
    if Path::new(relative).extension().is_none() {
        candidates.push(format!("{relative}.html"));
    }
    candidates
}

fn normalize_request_path(path: &str) -> Result<String> {
    let path = path.trim().trim_start_matches('/').trim_end_matches('/');
    if path.is_empty() {
        return Ok(String::new());
    }
    validate_artifact_path(path)?;
    Ok(path.to_string())
}

fn validate_url_prefix(prefix: &str) -> Result<()> {
    if !prefix.starts_with('/')
        || (prefix.len() > 1 && prefix.ends_with('/'))
        || prefix.contains(['?', '#', '\\'])
    {
        return Err(anyhow!("artifact mount URL prefix is invalid: {prefix}"));
    }
    let relative = prefix.trim_start_matches('/');
    if !relative.is_empty() {
        validate_artifact_path(relative)?;
    }
    Ok(())
}

fn validate_artifact_path(path: &str) -> Result<()> {
    let normalized = normalized_artifact_path(Path::new(path))?;
    if normalized != path {
        return Err(anyhow!("artifact path is not normalized: {path}"));
    }
    let lower = normalized.to_ascii_lowercase();
    if lower == ARTIFACT_MANIFEST_FILE
        || lower.starts_with(".anydesign/")
        || lower == ".well-known/anydesign"
        || lower.starts_with(".well-known/anydesign/")
    {
        return Err(anyhow!("artifact path is reserved: {path}"));
    }
    Ok(())
}

fn normalized_artifact_path(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(anyhow!("artifact path is invalid: {}", path.display()));
    }
    let normalized = path
        .to_str()
        .ok_or_else(|| anyhow!("artifact path must be valid UTF-8"))?
        .replace('\\', "/");
    if normalized
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(anyhow!("artifact path is not normalized: {normalized}"));
    }
    Ok(normalized)
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(anyhow!("{field} is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_routes::{ArtifactRouteContract, ArtifactRouteFile};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "artifact-manifest-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_manifest(root: &Path) -> ArtifactManifest {
        let index = b"<link href=\"/assets/app.css\"><h1>host root</h1>";
        fs::write(root.join("index.html"), index).unwrap();
        let manifest = ArtifactManifest::build(
            "project-1",
            "version-1",
            &"a".repeat(64),
            "synthetic-static",
            "synthetic-static@1",
            ArtifactDeliverySpec::HOST_ROOT,
            vec![manifest_file(
                Path::new("index.html"),
                index.len() as u64,
                sha256_hex(index),
            )
            .unwrap()],
        )
        .unwrap();
        fs::write(
            root.join(ARTIFACT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn host_root_resolver_verifies_manifest_and_file_bytes() {
        let root = temp_root("integrity");
        let manifest = write_manifest(&root);
        let resolver = ArtifactResolver::load(&root, &manifest.sha256().unwrap())
            .unwrap()
            .unwrap();
        let resolved = resolver.resolve("").unwrap().unwrap();
        assert_eq!(resolved.path, "index.html");
        assert_eq!(resolved.content_type, "text/html; charset=utf-8");
        assert_eq!(
            resolved.bytes,
            b"<link href=\"/assets/app.css\"><h1>host root</h1>"
        );

        fs::write(root.join("index.html"), b"tampered").unwrap();
        assert!(resolver
            .resolve("")
            .unwrap_err()
            .to_string()
            .contains("file integrity check failed"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_hash_and_reserved_paths_fail_closed() {
        let root = temp_root("manifest-hash");
        let manifest = write_manifest(&root);
        let mut tampered = manifest.clone();
        tampered.project_id = "other-project".to_string();
        fs::write(
            root.join(ARTIFACT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&tampered).unwrap(),
        )
        .unwrap();
        assert!(ArtifactResolver::load(&root, &manifest.sha256().unwrap())
            .unwrap_err()
            .to_string()
            .contains("manifest integrity check failed"));

        for reserved in [
            ".anydesign/config.json",
            ".well-known/anydesign/runtime.json",
        ] {
            assert!(manifest_file(Path::new(reserved), 1, "b".repeat(64)).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_type_is_derived_from_an_explicit_allowlist() {
        assert_eq!(
            content_type_for_artifact_path(Path::new("assets/app.js")).unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert!(content_type_for_artifact_path(Path::new("assets/unknown.exe")).is_err());
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../contracts/artifact-manifest-v1.schema.json"
        ))
        .unwrap();
        let schema_types = schema["properties"]["files"]["items"]["properties"]["contentType"]
            ["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(
            schema_types,
            ARTIFACT_CONTENT_TYPES.iter().copied().collect()
        );
    }

    #[test]
    fn published_resolver_uses_the_bound_route_manifest_without_html_fallbacks() {
        let root = temp_root("route-oracle");
        let docs = b"<h1>legacy docs artifact</h1>";
        fs::write(root.join("docs.html"), docs).unwrap();
        let route_manifest = ArtifactRouteManifest::build(
            "build-docs",
            &ArtifactRouteContract::docs(),
            [ArtifactRouteFile {
                path: "docs.html".to_string(),
                sha256: sha256_hex(docs),
            }],
        )
        .unwrap();
        let route_bytes = serde_json::to_vec_pretty(&route_manifest).unwrap();
        fs::write(root.join(ARTIFACT_ROUTE_MANIFEST_FILE), &route_bytes).unwrap();
        let manifest = ArtifactManifest::build(
            "project-1",
            "version-1",
            &"a".repeat(64),
            "fumadocs-docs",
            "fumadocs-docs@runtime-p6",
            ArtifactDeliverySpec::HOST_ROOT,
            vec![
                manifest_file(
                    Path::new(ARTIFACT_ROUTE_MANIFEST_FILE),
                    route_bytes.len() as u64,
                    sha256_hex(&route_bytes),
                )
                .unwrap(),
                manifest_file(Path::new("docs.html"), docs.len() as u64, sha256_hex(docs)).unwrap(),
            ],
        )
        .unwrap();
        fs::write(
            root.join(ARTIFACT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let resolver = ArtifactResolver::load(&root, &manifest.sha256().unwrap())
            .unwrap()
            .unwrap();
        let clean = resolver.resolve("docs").unwrap().unwrap();
        let trailing = resolver.resolve("docs/").unwrap().unwrap();
        assert_eq!(clean.path, "docs.html");
        assert_eq!(clean.bytes, trailing.bytes);
        assert!(resolver.resolve("docs.html").unwrap().is_none());
        assert!(resolver
            .resolve(ARTIFACT_ROUTE_MANIFEST_FILE)
            .unwrap()
            .is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
