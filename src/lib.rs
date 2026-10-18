pub mod inference;
pub mod models;

pub async fn serve(config: std::sync::Arc<models::config::Config>) -> anyhow::Result<()> {
    use models::{lifecycle::Manager, process::ProcessRuntime};
    use std::{future::IntoFuture, time::Duration};
    let listener = tokio::net::TcpListener::bind(config.listen()).await?;
    let runtime = std::sync::Arc::new(ProcessRuntime::new(&config)?);
    let (manager, task) = Manager::spawn(runtime, config.idle_timeout, config.queue_capacity);
    let app = inference::router(config.clone(), manager.clone())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(manager.cancellation().cancelled_owned())
        .into_future();
    tokio::pin!(server);
    tracing::info!(listen = %config.listen(), "lllmmm listening");
    let result = tokio::select! {
        result = &mut server => result,
        _ = async { tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} } } => {
            manager.shutdown();
            // A client that stops reading must not hold shutdown open indefinitely.
            match tokio::time::timeout(config.shutdown_timeout + Duration::from_secs(5), &mut server).await {
                Ok(result) => result,
                Err(_) => { tracing::warn!("HTTP shutdown deadline exceeded"); Ok(()) }
            }
        }
    };
    manager.shutdown();
    task.await?;
    result?;
    Ok(())
}
