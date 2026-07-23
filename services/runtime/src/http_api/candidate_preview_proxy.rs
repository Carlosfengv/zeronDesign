use super::*;
use crate::types::PreviewLeaseMode;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use futures::{SinkExt, StreamExt};
use regex::Regex;
use tokio_tungstenite::tungstenite::Message as UpstreamWsMessage;

pub(in crate::http_api) async fn proxy_candidate_preview(
    preview_access: PreviewAccessService,
    lease_id: String,
    preview_path: String,
    requested_prefix: Option<String>,
    prefix_required: bool,
    context: PreviewAccessContext<'_>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let access = preview_access
        .resolve_candidate(&lease_id, &preview_path, context)
        .await
        .map_err(preview_access_error)?;
    let preview_prefix = match context {
        PreviewAccessContext::Public(_) => validated_preview_prefix(
            prefix_required,
            requested_prefix.as_deref(),
            &access.project_id,
            &access.lease_id,
        )?,
        PreviewAccessContext::InternalCapture => format!("/preview-captures/{lease_id}"),
        PreviewAccessContext::InternalCaptureHost => String::new(),
    };
    let mut upstream = reqwest::Url::parse(&access.upstream_endpoint)
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    match access.mode {
        PreviewLeaseMode::Static => {
            upstream.set_path(&format!("/candidates/{}/{}", access.build_id, preview_path));
        }
        PreviewLeaseMode::Dev => {
            let base = format!("/previews/{lease_id}");
            let upstream_path = if preview_path.is_empty() {
                format!("{base}/")
            } else {
                format!("{base}/{preview_path}")
            };
            upstream.set_path(&upstream_path);
        }
    }
    let upstream_response = reqwest::Client::new()
        .get(upstream)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| not_found(format!("candidate preview upstream unavailable: {error}")))?;
    if !upstream_response.status().is_success() {
        return Err(not_found(format!(
            "candidate preview file not found: {preview_path}"
        )));
    }
    if access.mode == PreviewLeaseMode::Static {
        let manifest_hash = upstream_response
            .headers()
            .get("x-anydesign-candidate-manifest-hash")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| not_found("candidate preview manifest evidence missing".to_string()))?;
        if manifest_hash != access.candidate_manifest_hash {
            return Err(conflict_error(anyhow::anyhow!(
                "candidate preview manifest hash mismatch"
            )));
        }
    }
    let resolved_artifact_path = upstream_response
        .headers()
        .get("x-anydesign-artifact-path")
        .cloned();
    let resolved_artifact_sha256 = upstream_response
        .headers()
        .get("x-anydesign-artifact-sha256")
        .cloned();
    let content_type = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let mut bytes = upstream_response
        .bytes()
        .await
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?
        .to_vec();
    if content_type
        .to_str()
        .ok()
        .is_some_and(|value| value.starts_with("text/html"))
    {
        if let Ok(html) = String::from_utf8(bytes.clone()) {
            bytes = rewrite_preview_html(&html, &preview_prefix, &lease_id).into_bytes();
        }
    }
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("x-anydesign-preview-lease", &access.lease_id);
    if access.mode == PreviewLeaseMode::Static {
        response = response.header(
            "x-anydesign-candidate-manifest-hash",
            &access.candidate_manifest_hash,
        );
    }
    if let Some(value) = resolved_artifact_path {
        response = response.header("x-anydesign-artifact-path", value);
    }
    if let Some(value) = resolved_artifact_sha256 {
        response = response.header("x-anydesign-artifact-sha256", value);
    }
    response
        .body(Body::from(bytes))
        .map_err(|error| internal_error(anyhow::anyhow!(error)))
}

fn rewrite_preview_html(html: &str, preview_prefix: &str, lease_id: &str) -> String {
    if preview_prefix.is_empty() {
        return html.to_string();
    }
    let upstream_prefix = format!("/previews/{lease_id}");
    let root_asset = Regex::new(r#"(href|src)="(/[^"]*)""#).unwrap();
    root_asset
        .replace_all(html, |captures: &regex::Captures<'_>| {
            let path = &captures[2];
            let suffix = path
                .strip_prefix(&upstream_prefix)
                .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                .unwrap_or(path);
            format!(
                "{}=\"{}{}\"",
                &captures[1],
                preview_prefix.trim_end_matches('/'),
                suffix
            )
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::rewrite_preview_html;

    #[test]
    fn preview_html_replaces_next_base_path_instead_of_duplicating_it() {
        let html = r#"<link rel="icon" href="/previews/lease-1/icon.svg"><script src="/previews/lease-1/_next/app.js"></script><img src="/logo.svg">"#;

        let rewritten =
            rewrite_preview_html(html, "/projects/project-1/previews/lease-1", "lease-1");

        assert!(rewritten.contains(r#"href="/projects/project-1/previews/lease-1/icon.svg""#));
        assert!(rewritten.contains(r#"src="/projects/project-1/previews/lease-1/_next/app.js""#));
        assert!(rewritten.contains(r#"src="/projects/project-1/previews/lease-1/logo.svg""#));
        assert!(!rewritten.contains("previews/lease-1/previews/lease-1"));
    }

    #[test]
    fn capture_host_keeps_upstream_paths_when_no_prefix_is_required() {
        let html = r#"<script src="/previews/lease-1/_next/app.js"></script>"#;

        assert_eq!(rewrite_preview_html(html, "", "lease-1"), html);
    }
}

pub(in crate::http_api) async fn proxy_candidate_preview_websocket(
    preview_access: PreviewAccessService,
    lease_id: String,
    preview_path: String,
    context: PreviewAccessContext<'_>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let access = preview_access
        .resolve_candidate(&lease_id, &preview_path, context)
        .await
        .map_err(preview_access_error)?;
    if access.mode != PreviewLeaseMode::Dev {
        return Err(not_found(
            "static candidate previews do not accept WebSocket upgrades".to_string(),
        ));
    }
    let mut upstream = reqwest::Url::parse(&access.upstream_endpoint)
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    upstream
        .set_scheme(if upstream.scheme() == "https" {
            "wss"
        } else {
            "ws"
        })
        .map_err(|_| internal_error(anyhow::anyhow!("invalid preview WebSocket scheme")))?;
    let upstream_path = if preview_path.is_empty() {
        format!("/previews/{lease_id}/")
    } else {
        format!("/previews/{lease_id}/{preview_path}")
    };
    upstream.set_path(&upstream_path);
    let upstream_url = upstream.to_string();
    Ok(upgrade.on_upgrade(move |socket| async move {
        if let Ok((upstream, _response)) = tokio_tungstenite::connect_async(&upstream_url).await {
            bridge_websockets(socket, upstream).await;
        }
    }))
}

async fn bridge_websockets(
    client: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut client_tx, mut client_rx) = client.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    loop {
        tokio::select! {
            message = client_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let Some(message) = axum_to_upstream(message) else { break };
                if upstream_tx.send(message).await.is_err() { break; }
            }
            message = upstream_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let Some(message) = upstream_to_axum(message) else { continue };
                if client_tx.send(message).await.is_err() { break; }
            }
        }
    }
    let _ = upstream_tx.close().await;
    let _ = client_tx.close().await;
}

fn axum_to_upstream(message: AxumWsMessage) -> Option<UpstreamWsMessage> {
    match message {
        AxumWsMessage::Text(text) => Some(UpstreamWsMessage::Text(text.to_string().into())),
        AxumWsMessage::Binary(bytes) => Some(UpstreamWsMessage::Binary(bytes.to_vec().into())),
        AxumWsMessage::Ping(bytes) => Some(UpstreamWsMessage::Ping(bytes.to_vec().into())),
        AxumWsMessage::Pong(bytes) => Some(UpstreamWsMessage::Pong(bytes.to_vec().into())),
        AxumWsMessage::Close(_) => None,
    }
}

fn upstream_to_axum(message: UpstreamWsMessage) -> Option<AxumWsMessage> {
    match message {
        UpstreamWsMessage::Text(text) => Some(AxumWsMessage::Text(text.to_string().into())),
        UpstreamWsMessage::Binary(bytes) => Some(AxumWsMessage::Binary(bytes.to_vec().into())),
        UpstreamWsMessage::Ping(bytes) => Some(AxumWsMessage::Ping(bytes.to_vec().into())),
        UpstreamWsMessage::Pong(bytes) => Some(AxumWsMessage::Pong(bytes.to_vec().into())),
        UpstreamWsMessage::Close(_) => Some(AxumWsMessage::Close(None)),
        UpstreamWsMessage::Frame(_) => None,
    }
}

fn preview_access_error(error: PreviewAccessError) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        PreviewAccessError::NotFound(message) => not_found(message),
        PreviewAccessError::Forbidden(message) => forbidden(message),
        PreviewAccessError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        PreviewAccessError::Internal(message) => internal_error(anyhow::anyhow!(message)),
    }
}
