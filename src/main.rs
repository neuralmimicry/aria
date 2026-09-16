use aria::{alerts, api, config::Config, engine::Engine, store::Store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aria=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    let store = Store::connect(&config.database_url).await?;
    let engine = Engine::new(config.clone(), store.clone())?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let alert_worker = tokio::spawn(alerts::run(store, engine.config.clone(), shutdown_rx));
    tracing::info!(address=%config.bind,"Aria listening");
    let server = axum::serve(listener, api::router(engine))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        })
        .await;
    // Cancel the worker if serving fails before a shutdown signal arrives.
    if server.is_err() {
        alert_worker.abort();
    } else {
        alert_worker.await??;
    }
    server?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c()=>{}, _ = terminate.recv()=>{} }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
