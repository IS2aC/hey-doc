//! Sign-ups from the landing page, for access to the Hey-Doc chat.
//!
//! The answer is the same whether the number was new or already registered, so the form
//! cannot be used to find out who signed up.

use axum::{Json, extract::State, extract::rejection::JsonRejection, http::StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::ApiError;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({"first_name": "Awa", "last_name": "Koné", "country": "CI", "city": "Abidjan",
    "whatsapp": "+225 07 01 02 03 04", "email": null, "consent": true}))]
pub struct SignupRequest {
    first_name: String,
    last_name: String,
    /// Code pays ISO à deux lettres, par exemple `CI`.
    country: String,
    city: String,
    /// Numéro au format international, avec l’indicatif : `+225 07 01 02 03 04`.
    whatsapp: String,
    email: Option<String>,
    /// Doit valoir `true` : la personne accepte que Hey-Doc garde ces données et la contacte sur WhatsApp.
    consent: bool,
    /// Champ caché du formulaire, rempli seulement par les robots.
    #[serde(default)]
    website: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SignupResponse {
    /// Toujours `registered`, que le numéro soit nouveau ou déjà inscrit.
    status: &'static str,
}

fn bad(field: &'static str, message: &str) -> ApiError {
    ApiError::BadRequest("invalid_signup", format!("{field} : {message}"))
}

fn text(value: &str, field: &'static str) -> Result<String, ApiError> {
    let v = value.split_whitespace().collect::<Vec<_>>().join(" ");
    match v.chars().count() {
        0 => Err(bad(field, "champ obligatoire")),
        1..=80 => Ok(v),
        _ => Err(bad(field, "80 caractères au plus")),
    }
}

/// "+225 07-01.02 (03) 04" gives "+2250701020304". The number must start with its country code.
pub fn whatsapp_number(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let digits: String = trimmed.chars().filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')')).collect();
    let digits = digits.strip_prefix("00").map(|d| format!("+{d}")).unwrap_or(digits);
    let rest = digits.strip_prefix('+')?;
    (rest.len() >= 8 && rest.len() <= 15 && rest.chars().all(|c| c.is_ascii_digit()) && !rest.starts_with('0'))
        .then(|| format!("+{rest}"))
}

/// Inscription au chat Hey-Doc.
///
/// Enregistre la personne pour l’accès au chat. Une nouvelle inscription avec le même numéro
/// WhatsApp met à jour les données. La réponse est identique dans les deux cas.
#[utoipa::path(post, path = "/v1/signups", tag = "inscription", request_body = SignupRequest,
    responses((status = 201, body = SignupResponse), (status = 400, body = crate::ErrorBody)))]
pub async fn create(
    State(pool): State<PgPool>,
    body: Result<Json<SignupRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SignupResponse>), ApiError> {
    let Json(b) = body?;
    let done = (StatusCode::CREATED, Json(SignupResponse { status: "registered" }));
    if b.website.is_some_and(|w| !w.is_empty()) {
        return Ok(done);
    }
    if !b.consent {
        return Err(bad("consent", "l’accord est nécessaire pour s’inscrire"));
    }
    let first_name = text(&b.first_name, "first_name")?;
    let last_name = text(&b.last_name, "last_name")?;
    let city = text(&b.city, "city")?;
    let country = b.country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(bad("country", "code pays à deux lettres attendu"));
    }
    let whatsapp = whatsapp_number(&b.whatsapp).ok_or_else(|| bad("whatsapp", "numéro international attendu, par exemple +225 07 01 02 03 04"))?;
    let email = b.email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
    if let Some(e) = &email {
        let valid = e.len() <= 254 && e.split_once('@').is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'));
        if !valid {
            return Err(bad("email", "adresse invalide"));
        }
    }
    sqlx::query(
        "INSERT INTO signups (id, first_name, last_name, country, city, whatsapp, email, consent_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         ON CONFLICT (whatsapp) DO UPDATE SET first_name = $2, last_name = $3, country = $4, city = $5,
             email = $7, consent_at = now(), updated_at = now()",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(first_name)
    .bind(last_name)
    .bind(country)
    .bind(city)
    .bind(whatsapp)
    .bind(email)
    .execute(&pool)
    .await?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::whatsapp_number;

    #[test]
    fn whatsapp_numbers_are_normalised_to_e164() {
        assert_eq!(whatsapp_number("+225 07 01 02 03 04").as_deref(), Some("+2250701020304"));
        assert_eq!(whatsapp_number("00225-07.01.02.03.04").as_deref(), Some("+2250701020304"));
        assert_eq!(whatsapp_number("07 01 02 03 04"), None, "country code required");
        assert_eq!(whatsapp_number("+225 07 01 AB"), None);
        assert_eq!(whatsapp_number("+12"), None);
    }
}
