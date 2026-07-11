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
    let app = http_api::recovered_router(config).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
