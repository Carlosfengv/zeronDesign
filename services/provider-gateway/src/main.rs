use anyhow::Result;
use provider_gateway::{router, GatewayConfig, GatewayService};
use std::net::SocketAddr;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "provider_gateway=info,tower_http=info".into()),
        )
        .init();

    let config = GatewayConfig::from_env()?;
    let listen: SocketAddr = config.listen.parse()?;
    let service = GatewayService::new(config)?;
    service.start_configuration_refresh_task(Duration::from_secs(2));
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, router(service.clone()))
        .with_graceful_shutdown(shutdown_signal(service))
        .await?;
    Ok(())
}

async fn shutdown_signal(service: GatewayService) {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("installing SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("installing Ctrl-C handler");

    service.begin_shutdown();
    // Give Kubernetes readiness propagation a short window before the server
    // stops accepting sockets. Existing handlers continue under the pod grace
    // period rather than being force-cancelled here.
    tokio::time::sleep(Duration::from_secs(5)).await;
}
