use super::super::{AppState, HealthResponse, VersionResponse};
use axum::{extract::State, http::StatusCode, response::Html, routing::get, Json, Router};

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/version", get(version))
}

async fn root(State(state): State<AppState>) -> Html<String> {
    let base = format!("http://{}:{}", state.config.host, state.config.port);
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>AnyDesign Runtime</title>
  <style>
    :root {{ color-scheme: light dark; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; padding: 40px; background: #0f172a; color: #e5e7eb; }}
    main {{ max-width: 880px; margin: 0 auto; }}
    h1 {{ margin: 0 0 8px; font-size: 32px; }}
    p {{ color: #a5b4fc; line-height: 1.6; }}
    a {{ color: #67e8f9; }}
    .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin-top: 24px; }}
    .card {{ border: 1px solid #334155; border-radius: 8px; padding: 16px; background: #111827; }}
    code {{ background: #1f2937; border-radius: 4px; padding: 2px 5px; color: #f8fafc; }}
  </style>
</head>
<body>
  <main>
    <h1>AnyDesign Runtime</h1>
    <p>Status: <code>ready</code>. This root page is a local runtime index for browser checks.</p>
    <div class="grid">
      <div class="card"><strong>Health</strong><p><a href="{base}/health">{base}/health</a></p></div>
      <div class="card"><strong>Website artifact</strong><p><a href="{base}/artifacts/zeron-real-website-1783303319260/current">{base}/artifacts/zeron-real-website-1783303319260/current</a></p></div>
      <div class="card"><strong>Docs artifact</strong><p><a href="{base}/artifacts/zeron-real-docs-1783303417188/current/docs">{base}/artifacts/zeron-real-docs-1783303417188/current/docs</a></p></div>
      <div class="card"><strong>Run stream example</strong><p><code>{base}/runs/&lt;runId&gt;/events</code></p></div>
    </div>
  </main>
</body>
</html>"#
    ))
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    if state.supervisor.readiness().is_ready() {
        (StatusCode::OK, Json(HealthResponse { status: "ready" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not_ready",
            }),
        )
    }
}

async fn version(State(state): State<AppState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "anydesign-runtime",
        repository_commit: state.config.repository_commit.clone(),
        repository_dirty: state.config.repository_dirty,
        image_ref: state.config.runtime_image_ref.clone(),
    })
}
