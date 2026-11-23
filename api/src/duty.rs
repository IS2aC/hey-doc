use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};
use chrono::{DateTime, NaiveDateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};

use crate::{ApiError, SourceRef};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DutyParams {
    /// Identifiant du secteur.
    sector: i64,
    /// Date RFC 3339, ou heure locale d’Abidjan sans décalage. Maintenant par défaut.
    #[param(example = "2026-11-22T23:30")]
    at: Option<String>,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
struct Sector {
    id: i64,
    city: String,
    district: String,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
struct OnDuty {
    pharmacy_id: i64,
    name: String,
    phone: Option<String>,
    address: Option<String>,
    #[schema(value_type = Option<String>, example = "5.359952")]
    latitude: Option<Decimal>,
    #[schema(value_type = Option<String>, example = "-3.986160")]
    longitude: Option<Decimal>,
    duty_starts_at: DateTime<Utc>,
    duty_ends_at: DateTime<Utc>,
    #[sqlx(flatten)]
    source: SourceRef,
    #[sqlx(skip)]
    directions_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum DutyStatus {
    OnDuty,
    /// A verified schedule covers the instant but lists no pharmacy for it.
    NoneInSchedule,
    /// No verified schedule covers this sector at that instant.
    NoSchedule,
}

#[derive(Serialize, ToSchema)]
pub struct DutyResponse {
    sector: Sector,
    at: DateTime<Utc>,
    status: DutyStatus,
    pharmacies: Vec<OnDuty>,
}

/// Abidjan is UTC+0 all year, so a local time without offset is read as UTC.
fn parse_at(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Some(t.with_timezone(&Utc));
    }
    [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
    .map(|t| t.and_utc())
}

/// Pharmacies de garde.
///
/// Officines dont la garde couvre l’instant demandé. `status` vaut `on_duty`, `none_in_schedule` ou `no_schedule`.
#[utoipa::path(get, path = "/v1/duty-pharmacies", tag = "garde", params(DutyParams),
    responses((status = 200, body = DutyResponse), (status = 400, description = "Paramètre invalide", body = crate::ErrorBody),
        (status = 404, description = "Secteur introuvable", body = crate::ErrorBody)))]
pub async fn search(
    State(pool): State<PgPool>,
    params: Result<Query<DutyParams>, QueryRejection>,
) -> Result<Json<DutyResponse>, ApiError> {
    let Query(params) = params?;
    let at = match params.at.as_deref().map(str::trim) {
        None | Some("") => Utc::now(),
        Some(raw) => parse_at(raw).ok_or_else(|| {
            ApiError::BadRequest(
                "invalid_at",
                "at doit être une date ISO 8601, par exemple 2026-11-22T23:30".into(),
            )
        })?,
    };

    let sector =
        sqlx::query_as::<_, Sector>("SELECT id, city, district FROM sectors WHERE id = $1")
            .bind(params.sector)
            .fetch_optional(&pool)
            .await?
            .ok_or(ApiError::NotFound("sector_not_found"))?;

    let mut pharmacies = sqlx::query_as::<_, OnDuty>(
        "SELECT ph.id AS pharmacy_id, ph.name, ph.phone, ph.address, ph.latitude, ph.longitude,
                lower(d.period) AS duty_starts_at, upper(d.period) AS duty_ends_at,
                s.publisher, s.reference, s.published_on
         FROM pharmacy_duties d
         JOIN pharmacies ph ON ph.id = d.pharmacy_id
         JOIN duty_schedules sc ON sc.id = d.schedule_id
         JOIN sources s ON s.id = sc.source_id
         WHERE d.sector_id = $1 AND d.period @> $2::timestamptz
         ORDER BY ph.name, ph.id, lower(d.period)",
    )
    .bind(sector.id)
    .bind(at)
    .fetch_all(&pool)
    .await?;
    // Two overlapping schedules may list the same pharmacy: keep its first duty only.
    pharmacies.dedup_by_key(|p| p.pharmacy_id);
    for p in &mut pharmacies {
        if let (Some(lat), Some(lng)) = (p.latitude, p.longitude) {
            p.directions_url = Some(format!(
                "https://www.google.com/maps/dir/?api=1&destination={lat},{lng}"
            ));
        }
    }

    let status = if !pharmacies.is_empty() {
        DutyStatus::OnDuty
    } else {
        let covered: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT FROM duty_schedules WHERE sector_id = $1 AND period @> $2::timestamptz)",
        )
        .bind(sector.id)
        .bind(at)
        .fetch_one(&pool)
        .await?;
        if covered {
            DutyStatus::NoneInSchedule
        } else {
            DutyStatus::NoSchedule
        }
    };

    Ok(Json(DutyResponse {
        sector,
        at,
        status,
        pharmacies,
    }))
}
