//! Accounts for the Hey-Doc chat: create an account, sign in, sign out, read the profile.
//!
//! The account is identified by its WhatsApp number. Passwords are hashed with Argon2id. A
//! session is a random token kept by the browser in an HttpOnly cookie; the database only stores
//! its SHA-256, so a leaked table does not give access to accounts.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::ApiError;

const COOKIE: &str = "heydoc_session";
const SESSION_DAYS: i64 = 30;
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 128;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({"first_name": "Awa", "last_name": "Koné", "country": "CI", "city": "Abidjan",
    "whatsapp": "+225 07 01 02 03 04", "email": null, "password": "un mot de passe long", "consent": true}))]
pub struct RegisterRequest {
    first_name: String,
    last_name: String,
    /// Code pays ISO à deux lettres, par exemple `CI`.
    country: String,
    city: String,
    /// Numéro au format international, avec l’indicatif : `+225 07 01 02 03 04`.
    whatsapp: String,
    email: Option<String>,
    /// De 8 à 128 caractères.
    password: String,
    /// Doit valoir `true` : la personne accepte que Hey-Doc garde ces données et la contacte sur WhatsApp.
    consent: bool,
    /// Champ caché du formulaire, rempli seulement par les robots.
    #[serde(default)]
    website: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({"whatsapp": "+225 07 01 02 03 04", "password": "un mot de passe long"}))]
pub struct LoginRequest {
    whatsapp: String,
    password: String,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct Account {
    first_name: String,
    last_name: String,
    country: String,
    city: String,
    /// Au format E.164 : `+2250701020304`.
    whatsapp: String,
    email: Option<String>,
}

fn bad(field: &'static str, message: &str) -> ApiError {
    ApiError::BadRequest("invalid_account", format!("{field} : {message}"))
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
    let compact: String = raw.trim().chars().filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')')).collect();
    let compact = compact.strip_prefix("00").map(|d| format!("+{d}")).unwrap_or(compact);
    let rest = compact.strip_prefix('+')?;
    (rest.len() >= 8 && rest.len() <= 15 && rest.chars().all(|c| c.is_ascii_digit()) && !rest.starts_with('0'))
        .then(|| format!("+{rest}"))
}

fn check_password(password: &str) -> Result<(), ApiError> {
    match password.chars().count() {
        n if n < MIN_PASSWORD => Err(bad("password", "8 caractères au moins")),
        n if n > MAX_PASSWORD => Err(bad("password", "128 caractères au plus")),
        _ => Ok(()),
    }
}

/// Argon2 is deliberately slow: it runs off the async workers.
async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || Argon2::default().hash_password(password.as_bytes()).map(|h| h.to_string()))
        .await
        .map_err(|e| ApiError::Upstream("internal", e.to_string()))?
        .map_err(|e| ApiError::Upstream("internal", e.to_string()))
}

/// A hash checked when the number is unknown, so that the answer takes as long either way.
fn decoy_hash() -> &'static str {
    static DECOY: OnceLock<String> = OnceLock::new();
    DECOY.get_or_init(|| Argon2::default().hash_password(b"heydoc-decoy-password").expect("hachage").to_string())
}

async fn verify_password(password: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        let known = hash.is_some();
        let stored = hash.unwrap_or_else(|| decoy_hash().to_owned());
        let ok = PasswordHash::new(&stored).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok());
        known && ok
    })
    .await
    .unwrap_or(false)
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `HEYDOC_COOKIE_SECURE=1` in production, where the API is served over HTTPS.
fn secure() -> &'static str {
    static SECURE: OnceLock<bool> = OnceLock::new();
    if *SECURE.get_or_init(|| std::env::var("HEYDOC_COOKIE_SECURE").is_ok_and(|v| v == "1" || v == "true")) { "; Secure" } else { "" }
}

fn session_cookie(token: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", secure()))
        .expect("cookie ASCII")
}

fn token_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('=').map(str::to_owned))
        .filter(|t| !t.is_empty())
}

/// Opens a session and answers with the account and the cookie.
async fn signed_in(pool: &PgPool, signup_id: Uuid, account: Account, status: StatusCode) -> Result<Response, ApiError> {
    // 256 random bits from the system generator, through two UUID v4.
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()").execute(pool).await?;
    sqlx::query("INSERT INTO sessions (token_sha256, signup_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(sha256_hex(&token))
        .bind(signup_id)
        .bind(SESSION_DAYS as i32)
        .execute(pool)
        .await?;
    let mut res = (status, Json(account)).into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(&token, SESSION_DAYS * 86_400));
    Ok(res)
}

/// Création de compte.
///
/// Crée le compte et ouvre une session (cookie `heydoc_session`, HttpOnly, 30 jours). Une personne
/// déjà inscrite sans mot de passe complète son inscription. Un numéro qui a déjà un compte répond 409.
#[utoipa::path(post, path = "/v1/auth/register", tag = "compte", request_body = RegisterRequest,
    responses(
        (status = 201, body = Account, description = "Compte créé, session ouverte"),
        (status = 400, body = crate::ErrorBody),
        (status = 409, body = crate::ErrorBody, description = "Un compte existe déjà pour ce numéro")))]
pub async fn register(State(pool): State<PgPool>, body: Result<Json<RegisterRequest>, JsonRejection>) -> Result<Response, ApiError> {
    let Json(b) = body?;
    if !b.consent {
        return Err(bad("consent", "l’accord est nécessaire pour créer un compte"));
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
    check_password(&b.password)?;
    let account = Account { first_name, last_name, country, city, whatsapp, email };
    // A robot filled the hidden field: same answer, nothing stored, no session.
    if b.website.is_some_and(|w| !w.is_empty()) {
        return Ok((StatusCode::CREATED, Json(account)).into_response());
    }
    let hash = hash_password(b.password).await?;
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO signups (id, first_name, last_name, country, city, whatsapp, email, consent_at, password_hash)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8)
         ON CONFLICT (whatsapp) DO UPDATE SET first_name = $2, last_name = $3, country = $4, city = $5,
             email = $7, consent_at = now(), password_hash = $8, updated_at = now()
             WHERE signups.password_hash IS NULL
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(&account.first_name)
    .bind(&account.last_name)
    .bind(&account.country)
    .bind(&account.city)
    .bind(&account.whatsapp)
    .bind(&account.email)
    .bind(hash)
    .fetch_optional(&pool)
    .await?;
    let Some(id) = id else {
        return Err(ApiError::Conflict("account_exists", "Un compte existe déjà pour ce numéro. Connectez-vous.".into()));
    };
    signed_in(&pool, id, account, StatusCode::CREATED).await
}

#[derive(sqlx::FromRow)]
struct Stored {
    id: Uuid,
    password_hash: Option<String>,
    #[sqlx(flatten)]
    account: Account,
}

/// Connexion.
///
/// Vérifie le numéro WhatsApp et le mot de passe, puis ouvre une session. Un numéro inconnu et un
/// mauvais mot de passe donnent la même réponse 401.
#[utoipa::path(post, path = "/v1/auth/login", tag = "compte", request_body = LoginRequest,
    responses((status = 200, body = Account), (status = 400, body = crate::ErrorBody), (status = 401, body = crate::ErrorBody)))]
pub async fn login(State(pool): State<PgPool>, body: Result<Json<LoginRequest>, JsonRejection>) -> Result<Response, ApiError> {
    let Json(b) = body?;
    let refused = || ApiError::Unauthorized("invalid_credentials", "Numéro ou mot de passe incorrect.".into());
    let whatsapp = whatsapp_number(&b.whatsapp).ok_or_else(|| bad("whatsapp", "numéro international attendu, par exemple +225 07 01 02 03 04"))?;
    if b.password.chars().count() > MAX_PASSWORD {
        return Err(refused());
    }
    let stored: Option<Stored> = sqlx::query_as(
        "SELECT id, password_hash, first_name, last_name, country, city, whatsapp, email FROM signups WHERE whatsapp = $1",
    )
    .bind(&whatsapp)
    .fetch_optional(&pool)
    .await?;
    let hash = stored.as_ref().and_then(|s| s.password_hash.clone());
    if !verify_password(b.password, hash).await {
        return Err(refused());
    }
    let s = stored.ok_or_else(refused)?;
    signed_in(&pool, s.id, s.account, StatusCode::OK).await
}

/// Déconnexion.
///
/// Ferme la session du cookie et l’efface du navigateur. Répond 204 même sans session.
#[utoipa::path(post, path = "/v1/auth/logout", tag = "compte", responses((status = 204)))]
pub async fn logout(State(pool): State<PgPool>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = token_from(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_sha256 = $1").bind(sha256_hex(&token)).execute(&pool).await?;
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie("", 0));
    Ok(res)
}

/// Compte connecté.
///
/// Renvoie le compte de la session en cours, ou 401 sans session valide.
#[utoipa::path(get, path = "/v1/auth/me", tag = "compte", responses((status = 200, body = Account), (status = 401, body = crate::ErrorBody)))]
pub async fn me(State(pool): State<PgPool>, headers: HeaderMap) -> Result<Json<Account>, ApiError> {
    let none = || ApiError::Unauthorized("not_authenticated", "Aucune session ouverte.".into());
    let token = token_from(&headers).ok_or_else(none)?;
    let account: Option<Account> = sqlx::query_as(
        "SELECT s.first_name, s.last_name, s.country, s.city, s.whatsapp, s.email
         FROM sessions x JOIN signups s ON s.id = x.signup_id
         WHERE x.token_sha256 = $1 AND x.expires_at > now()",
    )
    .bind(sha256_hex(&token))
    .fetch_optional(&pool)
    .await?;
    account.map(Json).ok_or_else(none)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whatsapp_numbers_are_normalised_to_e164() {
        assert_eq!(whatsapp_number("+225 07 01 02 03 04").as_deref(), Some("+2250701020304"));
        assert_eq!(whatsapp_number("00225-07.01.02.03.04").as_deref(), Some("+2250701020304"));
        assert_eq!(whatsapp_number("07 01 02 03 04"), None, "country code required");
        assert_eq!(whatsapp_number("+225 07 01 AB"), None);
        assert_eq!(whatsapp_number("+12"), None);
    }

    #[test]
    fn the_session_token_is_read_from_the_cookie_header() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("theme=dark; heydoc_session=abc123; other=1"));
        assert_eq!(token_from(&h).as_deref(), Some("abc123"));
        h.insert(header::COOKIE, HeaderValue::from_static("heydoc_session="));
        assert_eq!(token_from(&h), None);
    }
}
