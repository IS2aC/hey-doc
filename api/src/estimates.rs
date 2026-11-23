use std::collections::HashMap;

use axum::{Json, extract::State, extract::rejection::JsonRejection};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::{ApiError, SourceRef, catalog::Presentation};

const MAX_LINES: usize = 50;
const MAX_QUANTITY: i32 = 100;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({"sector_id": 1, "pharmacy_id": null, "lines": [
    {"presentation_id": 1, "quantity": 2}, {"presentation_id": 2, "quantity": null}]}))]
pub struct EstimateRequest {
    sector_id: i64,
    /// Limite l’estimation aux prix relevés dans cette officine du secteur.
    pharmacy_id: Option<i64>,
    /// De 1 à 50 lignes.
    lines: Vec<LineRequest>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LineRequest {
    presentation_id: i64,
    /// Nombre de boîtes, de 1 à 100. `null` si la quantité reste à confirmer.
    quantity: Option<i32>,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
struct Scope {
    sector_id: i64,
    city: String,
    district: String,
    pharmacy_id: Option<i64>,
    pharmacy_name: Option<String>,
}

#[derive(Serialize, Clone, sqlx::FromRow, ToSchema)]
struct PriceRef {
    #[serde(skip)]
    presentation_id: i64,
    price_id: i64,
    #[schema(value_type = String, example = "2600.00")]
    amount_fcfa: Decimal,
    observed_on: NaiveDate,
    pharmacy_id: Option<i64>,
    pharmacy_name: Option<String>,
    #[sqlx(flatten)]
    source: SourceRef,
}

#[derive(Serialize, Clone, Copy, ToSchema)]
#[serde(rename_all = "snake_case")]
enum Exclusion {
    UnknownPresentation,
    QuantityUnconfirmed,
    NoCashPrice,
}

#[derive(Serialize, ToSchema)]
struct Line {
    index: usize,
    presentation_id: i64,
    presentation: Option<Presentation>,
    quantity: Option<i32>,
    #[schema(value_type = Option<String>, example = "2600.00")]
    unit_price_fcfa: Option<Decimal>,
    #[schema(value_type = Option<String>, example = "5200.00")]
    line_total_fcfa: Option<Decimal>,
    price: Option<PriceRef>,
    exclusion: Option<Exclusion>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum AmountKind {
    /// Every line is priced.
    Total,
    /// Only some lines are priced: never present this as the full cost.
    Subtotal,
    /// No line is priced: no amount is shown.
    None,
}

#[derive(Serialize, ToSchema)]
pub struct EstimateResponse {
    currency: &'static str,
    scope: Scope,
    amount_kind: AmountKind,
    /// `null` quand aucune ligne n’est chiffrée.
    #[schema(value_type = Option<String>, example = "6700.00")]
    amount_fcfa: Option<Decimal>,
    priced_lines: usize,
    excluded_lines: usize,
    lines: Vec<Line>,
}

/// Estimation au comptant.
///
/// Chiffre chaque ligne avec le dernier prix comptant relevé dans le secteur ou l’officine.
/// `amount_kind` indique s’il s’agit d’un total, d’un sous-total ou d’aucun montant.
#[utoipa::path(post, path = "/v1/estimates", tag = "estimation", request_body = EstimateRequest,
    responses((status = 200, body = EstimateResponse), (status = 400, description = "Requête invalide", body = crate::ErrorBody),
        (status = 404, description = "Secteur introuvable", body = crate::ErrorBody)))]
pub async fn create(
    State(pool): State<PgPool>,
    body: Result<Json<EstimateRequest>, JsonRejection>,
) -> Result<Json<EstimateResponse>, ApiError> {
    let Json(req) = body?;
    if req.lines.is_empty() || req.lines.len() > MAX_LINES {
        return Err(ApiError::BadRequest(
            "invalid_lines",
            format!("lines doit contenir de 1 à {MAX_LINES} lignes"),
        ));
    }
    if let Some(i) = req
        .lines
        .iter()
        .position(|l| l.quantity.is_some_and(|q| !(1..=MAX_QUANTITY).contains(&q)))
    {
        return Err(ApiError::BadRequest(
            "invalid_quantity",
            format!("lines[{i}].quantity doit être compris entre 1 et {MAX_QUANTITY}"),
        ));
    }

    let scope = sqlx::query_as::<_, Scope>(
        "SELECT sec.id AS sector_id, sec.city, sec.district, ph.id AS pharmacy_id, ph.name AS pharmacy_name
         FROM sectors sec
         LEFT JOIN pharmacies ph ON ph.id = $2 AND ph.sector_id = sec.id
         WHERE sec.id = $1",
    )
    .bind(req.sector_id)
    .bind(req.pharmacy_id)
    .fetch_optional(&pool)
    .await?
    .ok_or(ApiError::NotFound("sector_not_found"))?;
    if req.pharmacy_id.is_some() && scope.pharmacy_id.is_none() {
        return Err(ApiError::BadRequest(
            "pharmacy_not_in_sector",
            "Officine inconnue dans ce secteur".into(),
        ));
    }

    let ids: Vec<i64> = req.lines.iter().map(|l| l.presentation_id).collect();
    let presentations: HashMap<i64, Presentation> = sqlx::query_as::<_, Presentation>(
        "SELECT id, label, ingredient, strength, dosage_form, pack FROM presentations WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&pool)
    .await?
    .into_iter()
    .map(|p| (p.id, p))
    .collect();

    // Latest cash observation in scope; on a tie, the higher amount, so the budget is not underestimated.
    let prices: HashMap<i64, PriceRef> = sqlx::query_as::<_, PriceRef>(
        "SELECT DISTINCT ON (p.presentation_id)
                p.presentation_id, p.id AS price_id, p.amount_fcfa, p.observed_on,
                p.pharmacy_id, ph.name AS pharmacy_name,
                s.publisher, s.reference, s.published_on
         FROM prices p
         JOIN sources s ON s.id = p.source_id
         LEFT JOIN pharmacies ph ON ph.id = p.pharmacy_id
         WHERE p.kind = 'cash' AND p.sector_id = $1 AND p.presentation_id = ANY($2)
           AND ($3::bigint IS NULL OR p.pharmacy_id = $3)
         ORDER BY p.presentation_id, p.observed_on DESC, p.amount_fcfa DESC, p.id",
    )
    .bind(req.sector_id)
    .bind(&ids)
    .bind(req.pharmacy_id)
    .fetch_all(&pool)
    .await?
    .into_iter()
    .map(|p| (p.presentation_id, p))
    .collect();

    let lines: Vec<Line> = req
        .lines
        .iter()
        .enumerate()
        .map(|(index, l)| {
            let presentation = presentations.get(&l.presentation_id);
            let price = prices.get(&l.presentation_id);
            let exclusion = match (presentation, l.quantity, price) {
                (None, _, _) => Some(Exclusion::UnknownPresentation),
                (_, None, _) => Some(Exclusion::QuantityUnconfirmed),
                (_, _, None) => Some(Exclusion::NoCashPrice),
                _ => None,
            };
            let priced = exclusion.is_none();
            Line {
                index,
                presentation_id: l.presentation_id,
                presentation: presentation.cloned(),
                quantity: l.quantity,
                unit_price_fcfa: price
                    .filter(|_| presentation.is_some())
                    .map(|p| p.amount_fcfa),
                line_total_fcfa: priced
                    .then(|| price.unwrap().amount_fcfa * Decimal::from(l.quantity.unwrap())),
                price: price.filter(|_| presentation.is_some()).cloned(),
                exclusion,
            }
        })
        .collect();

    let priced: Vec<Decimal> = lines.iter().filter_map(|l| l.line_total_fcfa).collect();
    let (amount_kind, amount) = match priced.len() {
        0 => (AmountKind::None, None),
        n if n == lines.len() => (AmountKind::Total, Some(priced.iter().sum::<Decimal>())),
        _ => (AmountKind::Subtotal, Some(priced.iter().sum::<Decimal>())),
    };

    Ok(Json(EstimateResponse {
        currency: "XOF",
        scope,
        amount_kind,
        amount_fcfa: amount,
        priced_lines: priced.len(),
        excluded_lines: lines.len() - priced.len(),
        lines,
    }))
}
