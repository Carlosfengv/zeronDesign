use anydesign_runtime::{config::RuntimeConfig, runtime::RuntimeBootstrap};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "anydesign_runtime=info,tower_http=info".into()),
        )
        .init();

    let config = RuntimeConfig::from_env();
    let _shutdown = RuntimeBootstrap::new(config).run().await?;
    Ok(())
}
