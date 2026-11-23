use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let database_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL manquant")?;
    let addr = std::env::var("HEYDOC_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    hey_doc_api::MIGRATOR.run(&pool).await?;

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Hey-Doc API sur http://{addr}");
    axum::serve(listener, hey_doc_api::router(pool))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
