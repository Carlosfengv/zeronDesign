use super::*;

type LegacyArtifactResult = Result<(&'static str, Vec<u8>), (StatusCode, Json<ErrorResponse>)>;

// Read-only compatibility for artifacts promoted before artifact-manifest@1.
// New artifacts must resolve through crate::artifact_manifest::ArtifactResolver.
// remote-fs-boundary: allow-begin legacy-runtime-storage-artifact-serving
pub(in crate::http_api) fn read_legacy_artifact(
    output_root: &FsPath,
    artifact_path: &str,
    project_id: &str,
) -> LegacyArtifactResult {
    let path = resolve_artifact_file(output_root, artifact_path)?;
    let content_type = content_type_for_path(&path);
    let bytes =
        fs::read(&path).map_err(|_| not_found(format!("artifact not found: {artifact_path}")))?;
    let bytes = if content_type.starts_with("text/html") {
        String::from_utf8(bytes)
            .map(|html| rewrite_artifact_html(&html, project_id).into_bytes())
            .unwrap_or_else(|error| error.into_bytes())
    } else {
        bytes
    };
    Ok((content_type, bytes))
}

fn resolve_artifact_file(
    output_root: &FsPath,
    artifact_path: &str,
) -> Result<PathBuf, (StatusCode, Json<ErrorResponse>)> {
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
    if FsPath::new(relative).extension().is_none() {
        let html =
            static_artifact_path(output_root, &output_root.join(format!("{relative}.html")))?;
        if html.is_file() {
            return Ok(html);
        }
    }

    Err(not_found(format!("artifact not found: {artifact_path}")))
}

fn static_artifact_path(
    output_root: &FsPath,
    requested: &FsPath,
) -> Result<PathBuf, (StatusCode, Json<ErrorResponse>)> {
    let root = fs::canonicalize(output_root)
        .map_err(|_| not_found("artifact output root is not readable".to_string()))?;
    let path = if requested.exists() {
        fs::canonicalize(requested)
            .map_err(|_| not_found("artifact path is not readable".to_string()))?
    } else {
        let parent = requested
            .parent()
            .ok_or_else(|| not_found("artifact path is invalid".to_string()))?;
        let parent = fs::canonicalize(parent)
            .map_err(|_| not_found("artifact parent path is not readable".to_string()))?;
        parent.join(
            requested
                .file_name()
                .ok_or_else(|| not_found("artifact path is invalid".to_string()))?,
        )
    };
    if !path.starts_with(&root) {
        return Err(conflict_error(anyhow::anyhow!(
            "artifact path escapes project output"
        )));
    }
    Ok(path)
}

fn rewrite_artifact_html(html: &str, project_id: &str) -> String {
    let prefix = format!("/artifacts/{project_id}/current");
    html.replace("href=\"/_next/", &format!("href=\"{prefix}/_next/"))
        .replace("src=\"/_next/", &format!("src=\"{prefix}/_next/"))
        .replace("href=\"/_astro/", &format!("href=\"{prefix}/_astro/"))
        .replace("src=\"/_astro/", &format!("src=\"{prefix}/_astro/"))
        .replace(
            "href=\"/favicon.svg\"",
            &format!("href=\"{prefix}/favicon.svg\""),
        )
        .replace("href=\"/docs", &format!("href=\"{prefix}/docs"))
        .replace("href=\"/\"", &format!("href=\"{prefix}/\""))
        .replace("\\\"/_next/", &format!("\\\"{prefix}/_next/"))
        .replace("\\\"/_astro/", &format!("\\\"{prefix}/_astro/"))
        .replace("\\\"/docs", &format!("\\\"{prefix}/docs"))
        .replace("\\\"/\\\"", &format!("\\\"{prefix}/\\\""))
}

fn content_type_for_path(path: &FsPath) -> &'static str {
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
// remote-fs-boundary: allow-end legacy-runtime-storage-artifact-serving
