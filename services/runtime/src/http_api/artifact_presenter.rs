use super::*;
use crate::runtime_storage::{ArtifactContent, ArtifactReadError};

pub(in crate::http_api) type ArtifactHttpResponse =
    Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)>;

pub(in crate::http_api) fn present_artifact(
    content: ArtifactContent,
    route_prefix: &str,
) -> ArtifactHttpResponse {
    // Modern manifest-backed artifacts are served below /artifacts/<project>/..., while
    // Next.js and Next emit root-relative asset URLs. Rewrite every HTML artifact, not only
    // the legacy fallback path, so those assets remain reachable from the stable URL.
    let rewrite_html = content.content_type.starts_with("text/html");
    let bytes = if rewrite_html {
        String::from_utf8(content.bytes)
            .map(|html| rewrite_legacy_artifact_html(&html, route_prefix).into_bytes())
            .unwrap_or_else(|error| error.into_bytes())
    } else {
        content.bytes
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content.content_type)
            .map_err(|error| conflict_error(anyhow::Error::new(error)))?,
    );
    if content.nosniff {
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    Ok((headers, bytes))
}

pub(in crate::http_api) fn artifact_read_error(
    error: ArtifactReadError,
) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        ArtifactReadError::NotFound(message) => not_found(message),
        ArtifactReadError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
    }
}

fn rewrite_legacy_artifact_html(html: &str, prefix: &str) -> String {
    html.replace("href=\"/_next/", &format!("href=\"{prefix}/_next/"))
        .replace("src=\"/_next/", &format!("src=\"{prefix}/_next/"))
        .replace(
            "href=\"/favicon.svg\"",
            &format!("href=\"{prefix}/favicon.svg\""),
        )
        .replace("href=\"/docs", &format!("href=\"{prefix}/docs"))
        .replace("href=\"/\"", &format!("href=\"{prefix}/\""))
        .replace("\\\"/_next/", &format!("\\\"{prefix}/_next/"))
        .replace("\\\"/docs", &format!("\\\"{prefix}/docs"))
        .replace("\\\"/\\\"", &format!("\\\"{prefix}/\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_root_relative_next_assets_for_manifest_backed_html() {
        let (_, body) = present_artifact(
            ArtifactContent {
                content_type: "text/html; charset=utf-8".to_string(),
                bytes: br#"<link rel="stylesheet" href="/_next/app.css">"#.to_vec(),
                legacy_html_rewrite: false,
                nosniff: true,
            },
            "/artifacts/project-1/current",
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(body).unwrap(),
            r#"<link rel="stylesheet" href="/artifacts/project-1/current/_next/app.css">"#
        );
    }
}
