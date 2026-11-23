use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};

use crate::{ApiError, SourceRef};

#[derive(Serialize, Clone, sqlx::FromRow, ToSchema)]
pub(crate) struct Presentation {
    pub id: i64,
    pub label: String,
    pub ingredient: Option<String>,
    pub strength: Option<String>,
    pub dosage_form: Option<String>,
    pub pack: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchParams {
    /// Texte recherché, au moins 2 caractères.
    #[param(example = "amoxi")]
    q: String,
    /// Nombre de résultats, de 1 à 50. 20 par défaut.
    limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct SearchResponse {
    query: String,
    results: Vec<Presentation>,
}

/// Recherche catalogue.
///
/// Les correspondances exactes passent d’abord, puis les noms proches.
#[utoipa::path(get, path = "/v1/products/search", tag = "catalogue", params(SearchParams),
    responses((status = 200, body = SearchResponse), (status = 400, description = "Paramètre invalide", body = crate::ErrorBody)))]
pub async fn search(
    State(pool): State<PgPool>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<SearchResponse>, ApiError> {
    let Query(params) = params?;
    let q = params.q.trim().to_lowercase();
    if q.chars().count() < 2 {
        return Err(ApiError::BadRequest(
            "query_too_short",
            "q doit contenir au moins 2 caractères".into(),
        ));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let results = sqlx::query_as::<_, Presentation>(
        "SELECT id, label, ingredient, strength, dosage_form, pack
         FROM presentations
         WHERE strpos(search_text, $1) > 0 OR $1 <% search_text
         ORDER BY strpos(search_text, $1) > 0 DESC, word_similarity($1, search_text) DESC, label, id
         LIMIT $2",
    )
    .bind(&q)
    .bind(limit)
    .fetch_all(&pool)
    .await?;
    Ok(Json(SearchResponse { query: q, results }))
}

pub(crate) async fn ensure_presentation(pool: &PgPool, id: i64) -> Result<(), ApiError> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM presentations WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if found {
        Ok(())
    } else {
        Err(ApiError::NotFound("presentation_not_found"))
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PriceParams {
    /// `cash` (par défaut), `mugefci_published` ou `cmu_contracted`.
    kind: Option<String>,
    /// Limite les prix à un secteur.
    sector: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct PriceRow {
    id: i64,
    #[schema(value_type = String, example = "2600.00")]
    amount_fcfa: Decimal,
    observed_on: NaiveDate,
    sector_id: Option<i64>,
    sector_city: Option<String>,
    sector_district: Option<String>,
    pharmacy_id: Option<i64>,
    pharmacy_name: Option<String>,
    #[sqlx(flatten)]
    source: SourceRef,
}

#[derive(Serialize, ToSchema)]
pub struct PricesResponse {
    presentation_id: i64,
    kind: String,
    currency: &'static str,
    prices: Vec<PriceRow>,
}

const PRICE_KINDS: [&str; 3] = ["cash", "mugefci_published", "cmu_contracted"];

/// Prix d’une présentation.
///
/// Montants du type demandé, les relevés les plus récents d’abord.
#[utoipa::path(get, path = "/v1/presentations/{id}/prices", tag = "catalogue",
    params(("id" = i64, Path, description = "Identifiant interne de la présentation"), PriceParams),
    responses((status = 200, body = PricesResponse), (status = 400, description = "Paramètre invalide", body = crate::ErrorBody), (status = 404, description = "Présentation introuvable", body = crate::ErrorBody)))]
pub async fn prices(
    State(pool): State<PgPool>,
    id: Result<Path<i64>, PathRejection>,
    params: Result<Query<PriceParams>, QueryRejection>,
) -> Result<Json<PricesResponse>, ApiError> {
    let Path(id) = id?;
    let Query(params) = params?;
    let kind = params.kind.unwrap_or_else(|| "cash".into());
    if !PRICE_KINDS.contains(&kind.as_str()) {
        return Err(ApiError::BadRequest(
            "invalid_kind",
            format!("kind doit valoir {}", PRICE_KINDS.join(", ")),
        ));
    }
    ensure_presentation(&pool, id).await?;
    let prices = sqlx::query_as::<_, PriceRow>(
        "SELECT p.id, p.amount_fcfa, p.observed_on,
                p.sector_id, sec.city AS sector_city, sec.district AS sector_district,
                p.pharmacy_id, ph.name AS pharmacy_name,
                s.publisher, s.reference, s.published_on
         FROM prices p
         JOIN sources s ON s.id = p.source_id
         LEFT JOIN sectors sec ON sec.id = p.sector_id
         LEFT JOIN pharmacies ph ON ph.id = p.pharmacy_id
         WHERE p.presentation_id = $1 AND p.kind = $2
           AND ($3::bigint IS NULL OR p.sector_id = $3)
         ORDER BY p.observed_on DESC, sec.city, sec.district, ph.name, p.id",
    )
    .bind(id)
    .bind(&kind)
    .bind(params.sector)
    .fetch_all(&pool)
    .await?;
    Ok(Json(PricesResponse {
        presentation_id: id,
        kind,
        currency: "XOF",
        prices,
    }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CoverageParams {
    /// Régime, par exemple `cmu`. Sans valeur, toutes les listes sont consultées.
    #[param(example = "cmu")]
    scheme: Option<String>,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct Listing {
    listing_id: i64,
    insurer: String,
    scheme: String,
    listed: bool,
    #[sqlx(flatten)]
    source: SourceRef,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Listed,
    NotListed,
    ListUnavailable,
}

#[derive(Serialize, ToSchema)]
pub struct CoverageResponse {
    presentation_id: i64,
    scheme: Option<String>,
    /// Answer from the most recent edition; `listings` keeps older editions for context.
    status: CoverageStatus,
    listings: Vec<Listing>,
}

/// Couverture d’une présentation.
///
/// `status` répond pour l’édition la plus récente : `listed`, `not_listed` ou `list_unavailable`.
#[utoipa::path(get, path = "/v1/presentations/{id}/coverage", tag = "catalogue",
    params(("id" = i64, Path, description = "Identifiant interne de la présentation"), CoverageParams),
    responses((status = 200, body = CoverageResponse), (status = 400, description = "Paramètre invalide", body = crate::ErrorBody), (status = 404, description = "Présentation introuvable", body = crate::ErrorBody)))]
pub async fn coverage(
    State(pool): State<PgPool>,
    id: Result<Path<i64>, PathRejection>,
    params: Result<Query<CoverageParams>, QueryRejection>,
) -> Result<Json<CoverageResponse>, ApiError> {
    let Path(id) = id?;
    let Query(params) = params?;
    let scheme = params
        .scheme
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    ensure_presentation(&pool, id).await?;
    let listings = sqlx::query_as::<_, Listing>(
        "SELECT l.id AS listing_id, l.insurer, l.scheme,
                EXISTS (SELECT FROM coverage_entries e
                        WHERE e.listing_id = l.id AND e.presentation_id = $1) AS listed,
                s.publisher, s.reference, s.published_on
         FROM coverage_listings l
         JOIN sources s ON s.id = l.source_id
         WHERE $2::text IS NULL OR lower(l.scheme) = lower($2)
         ORDER BY s.published_on DESC NULLS LAST, l.id DESC",
    )
    .bind(id)
    .bind(&scheme)
    .fetch_all(&pool)
    .await?;
    let status = match listings.first() {
        None => CoverageStatus::ListUnavailable,
        Some(l) if l.listed => CoverageStatus::Listed,
        Some(_) => CoverageStatus::NotListed,
    };
    Ok(Json(CoverageResponse {
        presentation_id: id,
        scheme,
        status,
        listings,
    }))
}
