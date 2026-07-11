use anydesign_runtime::{config::RuntimeConfig, http_api};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "anydesign_runtime=info,tower_http=info".into()),
        )
        .init();

    let config = RuntimeConfig::from_env();
    config.validate_startup().map_err(anyhow::Error::msg)?;
    let listener = TcpListener::bind(config.bind_addr()).await?;
    let capture_listener = TcpListener::bind(config.runtime_browser_proxy_bind).await?;
    let state = http_api::recover_startup_runs(http_api::app_state(config)).await?;
    let capture_app = http_api::capture_router_with_state(state.clone());
    let capture_server =
        tokio::spawn(async move { axum::serve(capture_listener, capture_app).await });
    let result = axum::serve(listener, http_api::router_with_state(state)).await;
    capture_server.abort();
    result?;
    Ok(())
}
