use super::*;

// Chromium is not reliably re-entrant in the constrained Runtime container:
// concurrent headless processes can leave crashpad descendants holding stdio
// open after the browser exits. Share one process slot across screenshots and
// computed-style collectors so Website/Docs runs cannot wedge each other.
pub(super) static RUNTIME_BROWSER_PROCESS_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

pub(super) fn browser_open_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(BrowserOpenTool { workspace })
}

pub(super) fn browser_screenshot_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(BrowserScreenshotTool { workspace })
}

pub(super) fn browser_inspect_tool(workspace: Arc<dyn WorkspaceBackend>) -> Arc<dyn Tool> {
    Arc::new(BrowserInspectTool { workspace })
}

pub(super) struct BrowserOpenTool {
    pub(super) workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserOpenTool {
    fn name(&self) -> &'static str {
        "browser.open"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({ "url": string_schema("URL to inspect") }), &["url"])
    }

    async fn validate_input(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<Value, ValidationError> {
        require_string(&input, "url", self.name())?;
        Ok(input)
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
        if !is_internal_preview_url(url) {
            return PermissionResult::Deny {
                message: "browser.open public internet access is not allowed".to_string(),
                reason: PermissionReason::Rule {
                    source: RuleSource::Runtime,
                    rule_content: "public internet egress denied".to_string(),
                },
            };
        }
        allow_with_input(input, "browser open internal preview allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let requested_url = required_str(&input, "url")?.to_string();
        let url = if ctx.remote_workspace {
            let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
                .await
                .ok_or_else(|| {
                    typed_recoverable(
                        "browser.open requires an active Runtime preview lease".to_string(),
                        "browser.preview_missing",
                        json!({ "suggestedAction": "Call preview.start before browser.open." }),
                    )
                })?;
            let proxy_url = preview
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    typed_recoverable(
                        "browser.open preview lease has no Runtime proxy URL".to_string(),
                        "browser.preview_invalid",
                        json!({ "preview": preview }),
                    )
                })?
                .to_string();
            if !proxy_url.starts_with(&ctx.runtime_public_base_url) {
                return Err(ToolError::Terminal(
                    "browser.open refused a preview URL outside the Runtime proxy".to_string(),
                ));
            }
            proxy_url
        } else {
            requested_url
        };
        let state = json!({
            "url": url,
            "consoleErrors": [],
            "opened": true,
        });
        write_workspace_json(&*self.workspace, &ctx, "state/browser.json", &state).await?;
        Ok(ToolResult::ok(state))
    }
}

pub(super) struct BrowserScreenshotTool {
    pub(super) workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &'static str {
        "browser.screenshot"
    }

    fn input_schema(&self) -> Value {
        object_schema(
            json!({
                "screenshotId": string_schema("Screenshot artifact id"),
                "blank": { "type": "boolean" }
            }),
            &[],
        )
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "browser screenshot allowed")
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let screenshot_id = input
            .get("screenshotId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| ctx.store.next_id("screenshot"));
        let browser_state = read_workspace_json(&*self.workspace, &ctx, "state/browser.json")
            .await
            .ok_or_else(|| {
                typed_recoverable(
                    "browser.screenshot requires browser.open first".to_string(),
                    "browser.not_open",
                    json!({ "suggestedAction": "Call browser.open before browser.screenshot." }),
                )
            })?;
        let url = browser_state
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Terminal("browser state has no URL".to_string()))?;
        let (capture_url, capture_lease_id) = if ctx.remote_workspace {
            let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
                .await
                .ok_or_else(|| {
                    typed_recoverable(
                        "browser.screenshot requires an active Runtime preview lease".to_string(),
                        "preview.lease_missing",
                        json!({}),
                    )
                })?;
            let lease_id = preview
                .get("leaseId")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Terminal("preview state has no leaseId".to_string()))?
                .to_string();
            (
                format!(
                    "{}/preview-captures/{lease_id}/",
                    ctx.runtime_browser_proxy_base_url
                ),
                Some(lease_id),
            )
        } else {
            (url.to_string(), None)
        };
        let capture = capture_runtime_screenshot(
            &ctx,
            &screenshot_id,
            &capture_url,
            capture_lease_id.as_deref(),
        )
        .await?;
        let fixture_blank = input.get("blank").and_then(Value::as_bool).unwrap_or(false);
        if ctx.remote_workspace && capture.is_none() {
            return Err(typed_recoverable(
                "Runtime browser worker is not configured".to_string(),
                "browser.worker_unavailable",
                json!({ "requiredEnv": "RUNTIME_BROWSER_EXECUTABLE" }),
            ));
        }
        let is_blank = capture
            .as_ref()
            .map(|capture| capture.nonblank_pixel_ratio < 0.0005)
            .unwrap_or(fixture_blank);
        let path = ctx
            .workspace_root
            .join("outputs/screenshots")
            .join(format!("{screenshot_id}.json"));
        let artifact = json!({
            "screenshotId": screenshot_id,
            "blank": is_blank,
            "url": url,
            "runtimeScreenshotUri": capture.as_ref().map(|capture| capture.uri.clone()),
            "pngSha256": capture.as_ref().map(|capture| capture.png_sha256.clone()),
            "documentSha256": capture.as_ref().map(|capture| capture.document_sha256.clone()),
            "width": capture.as_ref().map(|capture| capture.width),
            "height": capture.as_ref().map(|capture| capture.height),
            "nonblankPixelRatio": capture.as_ref().map(|capture| capture.nonblank_pixel_ratio),
        });
        self.workspace
            .write_string(
                &ctx,
                &path,
                &serde_json::to_string_pretty(&artifact)
                    .map_err(|error| ToolError::Recoverable(error.to_string()))?,
            )
            .await
            .map_err(|error| ToolError::Recoverable(error.to_string()))?;
        Ok(ToolResult::ok(json!({
            "screenshotId": artifact["screenshotId"],
            "path": format!("/workspace/outputs/screenshots/{}.json", artifact["screenshotId"].as_str().unwrap_or("unknown")),
            "blank": is_blank,
            "runtimeScreenshotUri": artifact["runtimeScreenshotUri"],
            "pngSha256": artifact["pngSha256"],
            "documentSha256": artifact["documentSha256"],
            "width": artifact["width"],
            "height": artifact["height"],
            "nonblankPixelRatio": artifact["nonblankPixelRatio"],
        })))
    }
}

pub(super) struct RuntimeScreenshotCapture {
    pub(super) uri: String,
    pub(super) png_sha256: String,
    pub(super) document_sha256: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) nonblank_pixel_ratio: f64,
}

pub(super) async fn capture_runtime_screenshot(
    ctx: &ToolContext,
    screenshot_id: &str,
    url: &str,
    preview_lease_id: Option<&str>,
) -> Result<Option<RuntimeScreenshotCapture>, ToolError> {
    let Some(executable) = std::env::var("RUNTIME_BROWSER_EXECUTABLE")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let screenshot_dir = ctx
        .runtime_storage_dir
        .join("screenshots")
        .join(safe_segment(&ctx.run.project_id))
        .join(safe_segment(&ctx.run.id));
    // remote-fs-boundary: allow-begin runtime-browser-screenshot-artifact
    fs::create_dir_all(&screenshot_dir)
        .map_err(|error| ToolError::Terminal(format!("create screenshot directory: {error}")))?;
    let screenshot_path = screenshot_dir.join(format!("{}.png", safe_segment(screenshot_id)));
    // remote-fs-boundary: allow-end runtime-browser-screenshot-artifact
    let document_bytes = wait_for_runtime_proxy_document(url, Duration::from_secs(15)).await?;
    let document_sha256 = sha256_hex(&document_bytes);
    let _browser_process_guard = RUNTIME_BROWSER_PROCESS_LOCK.lock().await;
    let mut command = TokioCommand::new(executable);
    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--hide-scrollbars")
        .arg("--window-size=1440,900")
        .arg(format!("--screenshot={}", screenshot_path.display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(typed_recoverable(
                format!("Runtime browser worker failed to start: {error}"),
                "browser.worker_failed",
                json!({}),
            ));
        }
        Err(_) => {
            return Err(typed_recoverable(
                "Runtime browser worker timed out after 30 seconds".to_string(),
                "browser.capture_timeout",
                json!({ "url": url, "deadlineMs": 30_000 }),
            ));
        }
    };
    if !output.status.success() {
        return Err(typed_recoverable(
            format!(
                "Runtime browser worker failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            "browser.capture_failed",
            json!({ "url": url }),
        ));
    }
    // remote-fs-boundary: allow-begin runtime-browser-screenshot-artifact
    let png_bytes = fs::read(&screenshot_path)
        .map_err(|error| ToolError::Terminal(format!("read screenshot PNG: {error}")))?;
    // remote-fs-boundary: allow-end runtime-browser-screenshot-artifact
    let decoder = png::Decoder::new(png_bytes.as_slice());
    let mut reader = decoder
        .read_info()
        .map_err(|error| ToolError::Terminal(format!("decode screenshot PNG: {error}")))?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|error| ToolError::Terminal(format!("decode screenshot pixels: {error}")))?;
    let bytes = &pixels[..frame.buffer_size()];
    let channels = frame.color_type.samples();
    let mut colors = std::collections::HashMap::<[u8; 4], usize>::new();
    for pixel in bytes.chunks_exact(channels) {
        let rgba = match frame.color_type {
            png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
            png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
            png::ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
            png::ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 255],
            png::ColorType::Indexed => {
                return Err(ToolError::Terminal(
                    "indexed screenshot PNG is unsupported".to_string(),
                ));
            }
        };
        *colors.entry(rgba).or_default() += 1;
    }
    let total = colors.values().sum::<usize>();
    let dominant = colors.values().copied().max().unwrap_or(total);
    let nonblank_pixel_ratio = if total == 0 {
        0.0
    } else {
        1.0 - dominant as f64 / total as f64
    };
    let capture = RuntimeScreenshotCapture {
        uri: preview_lease_id.map_or_else(
            || {
                format!(
                    "runtime://screenshots/{}/{}/{}.png",
                    safe_segment(&ctx.run.project_id),
                    safe_segment(&ctx.run.id),
                    safe_segment(screenshot_id)
                )
            },
            |lease_id| {
                format!(
                    "runtime://preview-captures/{}/{}/{}",
                    safe_segment(&ctx.run.project_id),
                    safe_segment(&ctx.run.id),
                    safe_segment(lease_id)
                )
            },
        ),
        png_sha256: sha256_hex(&png_bytes),
        document_sha256,
        width: frame.width,
        height: frame.height,
        nonblank_pixel_ratio,
    };
    let metadata_path = screenshot_path.with_extension("json");
    // remote-fs-boundary: allow-begin runtime-browser-screenshot-artifact
    fs::write(
        metadata_path,
        serde_json::to_vec_pretty(&json!({
            "uri": capture.uri,
            "pngSha256": capture.png_sha256,
            "documentSha256": capture.document_sha256,
            "width": capture.width,
            "height": capture.height,
            "nonblankPixelRatio": capture.nonblank_pixel_ratio,
        }))
        .map_err(|error| ToolError::Terminal(error.to_string()))?,
    )
    .map_err(|error| ToolError::Terminal(format!("write screenshot metadata: {error}")))?;
    // remote-fs-boundary: allow-end runtime-browser-screenshot-artifact
    Ok(Some(capture))
}

async fn wait_for_runtime_proxy_document(
    url: &str,
    timeout: Duration,
) -> Result<Vec<u8>, ToolError> {
    let deadline = time::Instant::now() + timeout;
    let client = reqwest::Client::new();
    loop {
        let last_error = match client.get(url).timeout(Duration::from_secs(3)).send().await {
            Ok(response) if response.status().is_success() => {
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                match response.bytes().await {
                    Ok(bytes)
                        if !bytes.is_empty()
                            && bytes.len() <= 5 * 1024 * 1024
                            && content_type.contains("text/html") =>
                    {
                        return Ok(bytes.to_vec());
                    }
                    Ok(bytes) => format!(
                        "preview proxy returned invalid document: content-type={content_type} bytes={}",
                        bytes.len()
                    ),
                    Err(error) => error.to_string(),
                }
            }
            Ok(response) => format!("preview proxy returned {}", response.status()),
            Err(error) => error.to_string(),
        };
        if time::Instant::now() >= deadline {
            return Err(typed_recoverable(
                format!("Runtime preview proxy is not ready for screenshot: {last_error}"),
                "browser.preview_unavailable",
                json!({ "url": url }),
            ));
        }
        time::sleep(Duration::from_millis(200)).await;
    }
}

struct BrowserInspectTool {
    workspace: Arc<dyn WorkspaceBackend>,
}

#[async_trait]
impl Tool for BrowserInspectTool {
    fn name(&self) -> &'static str {
        "browser.inspect"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "browser inspect allowed")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        let browser = read_workspace_json(&*self.workspace, &ctx, "state/browser.json")
            .await
            .unwrap_or_else(|| {
                json!({
                    "url": Value::Null,
                    "consoleErrors": [],
                    "opened": false,
                })
            });
        let preview = read_workspace_json(&*self.workspace, &ctx, "state/preview.json")
            .await
            .unwrap_or_else(|| {
                json!({
                    "status": "stopped",
                    "accessible": false,
                })
            });
        Ok(ToolResult::ok(json!({
            "url": browser.get("url").cloned().unwrap_or(Value::Null),
            "opened": browser.get("opened").cloned().unwrap_or(json!(false)),
            "consoleErrors": browser.get("consoleErrors").cloned().unwrap_or_else(|| json!([])),
            "preview": preview,
        })))
    }
}
