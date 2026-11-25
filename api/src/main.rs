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
    let reader = hey_doc_api::reducto::Reducto::from_env()
        .map(|r| std::sync::Arc::new(r) as std::sync::Arc<dyn hey_doc_api::reducto::PrescriptionReader>);
    if reader.is_none() {
        tracing::warn!("REDUCTO_API_KEY absent : la lecture de photo répondra 503, la saisie texte reste disponible");
    }
    let archive = match hey_doc_api::archive::Archive::from_env(pool.clone()) {
        Some(Ok(a)) => {
            let a = std::sync::Arc::new(a);
            a.clone().spawn_purge();
            Some(a)
        }
        Some(Err(e)) => return Err(format!("stockage objet mal configuré : {e}").into()),
        None => {
            tracing::warn!("S3_ENDPOINT absent : aucune ordonnance ne sera archivée, même avec accord");
            None
        }
    };
    let llm = hey_doc_api::llm::Llm::from_env().map(std::sync::Arc::new);
    match &llm {
        Some(l) => tracing::info!(model = l.model(), "modèle de langage configuré"),
        None => tracing::warn!("LLM_MODEL absent : la structuration par modèle de langage répondra 503"),
    }
    let state = hey_doc_api::AppState { pool, reader, archive, llm };
    axum::serve(listener, hey_doc_api::router_with(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
