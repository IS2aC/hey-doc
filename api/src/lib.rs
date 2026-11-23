pub mod archive;
mod catalog;
mod duty;
mod estimates;
mod prescriptions;
pub mod reducto;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRef},
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use sqlx::PgPool;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../database/migrations");

#[derive(OpenApi)]
#[openapi(
    info(
        title = "API de données Hey-Doc",
        description = "Catalogue, prix, couverture, estimation au comptant et pharmacies de garde. \
            Les montants sont des chaînes décimales exactes en FCFA. Les instants sont en UTC, \
            qui est aussi l’heure locale d’Abidjan. La lecture d’ordonnance rapproche chaque ligne \
            du catalogue et laisse la personne trancher les lignes ambiguës."
    ),
    paths(
        health,
        catalog::search,
        catalog::prices,
        catalog::coverage,
        estimates::create,
        duty::search,
        prescriptions::read,
        prescriptions::match_text
    )
)]
pub struct ApiDoc;

/// Shared by the handlers. Most only need the pool, through `FromRef`.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// `None` when no reader is configured: photo reading then answers 503.
    pub reader: Option<Arc<dyn reducto::PrescriptionReader>>,
    /// `None` when object storage is not configured: consent is then ignored, nothing is kept.
    pub archive: Option<Arc<archive::Archive>>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> PgPool {
        state.pool.clone()
    }
}

pub fn router(pool: PgPool) -> Router {
    router_with(AppState { pool, reader: None, archive: None })
}

pub fn router_with_reader(pool: PgPool, reader: Option<Arc<dyn reducto::PrescriptionReader>>) -> Router {
    router_with(AppState { pool, reader, archive: None })
}

pub fn router_with(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/products/search", get(catalog::search))
        .route("/v1/presentations/{id}/prices", get(catalog::prices))
        .route("/v1/presentations/{id}/coverage", get(catalog::coverage))
        .route("/v1/estimates", post(estimates::create))
        .route("/v1/duty-pharmacies", get(duty::search))
        .route(
            "/v1/prescriptions/read",
            // The multipart envelope adds a few bytes around the image.
            post(prescriptions::read).layer(DefaultBodyLimit::max(prescriptions::MAX_IMAGE_BYTES + 64 * 1024)),
        )
        .route("/v1/prescriptions/match", post(prescriptions::match_text))
        .with_state(state)
        .merge(SwaggerUi::new("/docs").url("/docs/openapi.json", ApiDoc::openapi()))
}

/// Vérifie l’accès à la base.
#[utoipa::path(get, path = "/health", tag = "service",
    responses((status = 200, body = String, example = "ok"), (status = 500, body = ErrorBody)))]
async fn health(
    axum::extract::State(pool): axum::extract::State<PgPool>,
) -> Result<&'static str, ApiError> {
    sqlx::query("SELECT 1").execute(&pool).await?;
    Ok("ok")
}

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str, String),
    NotFound(&'static str),
    /// A dependency is not configured or is down (503).
    Unavailable(&'static str, String),
    /// A dependency answered with an error (502).
    Upstream(&'static str, String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Db(e)
    }
}

impl From<QueryRejection> for ApiError {
    fn from(e: QueryRejection) -> Self {
        ApiError::BadRequest("invalid_query", e.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(e: PathRejection) -> Self {
        ApiError::BadRequest("invalid_path", e.body_text())
    }
}

impl From<JsonRejection> for ApiError {
    fn from(e: JsonRejection) -> Self {
        ApiError::BadRequest("invalid_body", e.body_text())
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct ErrorBody {
    /// Code stable, par exemple `sector_not_found`.
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            ApiError::BadRequest(code, msg) => (StatusCode::BAD_REQUEST, code, msg),
            ApiError::NotFound(code) => {
                (StatusCode::NOT_FOUND, code, "Ressource introuvable".into())
            }
            ApiError::Unavailable(code, msg) => (StatusCode::SERVICE_UNAVAILABLE, code, msg),
            ApiError::Upstream(code, msg) => (StatusCode::BAD_GATEWAY, code, msg),
            ApiError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "Erreur interne".into(),
                )
            }
        };
        (status, Json(ErrorBody { error, message })).into_response()
    }
}

#[derive(Serialize, Clone, sqlx::FromRow, utoipa::ToSchema)]
pub(crate) struct SourceRef {
    pub publisher: String,
    pub reference: String,
    pub published_on: Option<chrono::NaiveDate>,
}
