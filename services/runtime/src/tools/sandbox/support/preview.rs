use super::*;

pub(super) async fn verify_screenshot_artifact(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    screenshot_id: &str,
) -> Result<(), ToolError> {
    if screenshot_id.trim().is_empty()
        || screenshot_id.contains('/')
        || screenshot_id.contains('\\')
        || screenshot_id.contains("..")
    {
        return Err(typed_recoverable(
            "preview.report_candidate screenshotId must be a simple browser.screenshot artifact id"
                .to_string(),
            "preview.screenshot_invalid",
            json!({
                "screenshotId": screenshot_id,
                "suggestedAction": "Call browser.screenshot and pass its screenshotId."
            }),
        ));
    }
    let path = format!("outputs/screenshots/{screenshot_id}.json");
    let artifact = read_workspace_json(workspace, ctx, &path).await.ok_or_else(|| {
        typed_recoverable(
            format!(
                "preview.report_candidate requires existing screenshot artifact {screenshot_id}; call browser.screenshot first"
            ),
            "preview.screenshot_missing",
            json!({
                "screenshotId": screenshot_id,
                "expectedPath": format!("/workspace/{path}"),
                "suggestedAction": "Call browser.screenshot before preview.report_candidate."
            }),
        )
    })?;
    if artifact
        .get("blank")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        return Err(typed_recoverable(
            format!("preview.report_candidate rejected blank screenshot artifact {screenshot_id}"),
            "preview.screenshot_blank",
            json!({
                "screenshotId": screenshot_id,
                "path": format!("/workspace/{path}"),
                "suggestedAction": "Fix the preview and capture a non-blank screenshot."
            }),
        ));
    }
    if ctx.remote_workspace {
        let png_hash = artifact.get("pngSha256").and_then(Value::as_str);
        let document_hash = artifact.get("documentSha256").and_then(Value::as_str);
        let screenshot_uri = artifact.get("runtimeScreenshotUri").and_then(Value::as_str);
        let nonblank_ratio = artifact
            .get("nonblankPixelRatio")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if png_hash.is_none_or(|hash| hash.len() != 64)
            || document_hash.is_none_or(|hash| hash.len() != 64)
            || !screenshot_uri.is_some_and(|uri| {
                uri.starts_with("runtime://screenshots/")
                    || uri.starts_with("runtime://preview-captures/")
            })
            || nonblank_ratio < 0.0005
        {
            return Err(typed_recoverable(
                "preview.report_candidate requires Runtime-owned bitmap evidence".to_string(),
                "preview.screenshot_evidence_invalid",
                json!({
                    "screenshotId": screenshot_id,
                    "nonblankPixelRatio": nonblank_ratio,
                    "suggestedAction": "Capture the Runtime proxy URL with browser.screenshot."
                }),
            ));
        }
    }
    Ok(())
}

pub(super) fn is_internal_preview_url(url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return false;
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local")
}

pub(super) fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next()?
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

pub(super) fn url_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.starts_with('[') {
        let after_bracket = host_port.split_once(']')?.1;
        return after_bracket
            .strip_prefix(':')
            .and_then(|port| port.parse().ok())
            .or_else(|| default_port_for_scheme(scheme));
    }
    let mut parts = host_port.rsplitn(2, ':');
    let maybe_port = parts.next()?;
    if parts.next().is_some() {
        maybe_port.parse().ok()
    } else {
        default_port_for_scheme(scheme)
    }
}

pub(super) fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

pub(super) async fn verify_preview_accessible(url: &str) -> Result<(), ToolError> {
    let host = url_host(url)
        .ok_or_else(|| ToolError::Recoverable(format!("preview.start invalid url: {url}")))?;
    let port = url_port(url)
        .ok_or_else(|| ToolError::Recoverable(format!("preview.start missing port: {url}")))?;
    time::timeout(
        Duration::from_millis(750),
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| ToolError::Recoverable(format!("preview.start timed out connecting to {url}")))?
    .map_err(|error| {
        ToolError::Recoverable(format!("preview.start could not reach {url}: {error}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod browser_worker_tests {
    use super::*;
    use crate::RuntimeStore;
    use tokio::io::AsyncReadExt;
    use tokio::sync::Mutex;

    static BROWSER_ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn runtime_browser_worker_writes_real_nonblank_png_evidence() {
        let executable = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        if !executable.is_file() {
            return;
        }
        let _guard = BROWSER_ENV_LOCK.lock().await;
        unsafe {
            std::env::set_var("RUNTIME_BROWSER_EXECUTABLE", executable);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
                    let html = "<!doctype html><style>html,body{margin:0;height:100%}body{background:linear-gradient(90deg,#f00 50%,#00f 50%)}</style>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        html.len(),
                        html
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                "browser-project".to_string(),
                AgentPhase::Review,
                "review".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let storage = std::env::temp_dir().join(format!(
            "runtime-browser-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut ctx = ToolContext::new(store, run, storage.join("workspace"));
        ctx.runtime_storage_dir = storage;
        let capture = browser::capture_runtime_screenshot(
            &ctx,
            "real-browser",
            &format!("http://{address}/"),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        unsafe {
            std::env::remove_var("RUNTIME_BROWSER_EXECUTABLE");
        }
        server.abort();
        assert_eq!((capture.width, capture.height), (1440, 900));
        assert_eq!(capture.png_sha256.len(), 64);
        assert!(capture.nonblank_pixel_ratio > 0.25);
        assert!(capture.uri.starts_with("runtime://screenshots/"));
    }
}
