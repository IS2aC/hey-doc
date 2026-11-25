use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

struct Reply {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>, cookie: Option<&str>) -> Reply {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let cookie = res.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_owned());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie }
}

/// "heydoc_session=abc; Path=/; …" gives "heydoc_session=abc".
fn pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_owned()
}

fn awa() -> Value {
    json!({ "first_name": " Awa ", "last_name": "Koné", "country": "ci", "city": "Abidjan",
            "whatsapp": "+225 07 01 02 03 04", "email": "Awa@Exemple.ci", "password": "mot de passe sûr", "consent": true })
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn register_sign_out_and_sign_in_again(pool: PgPool) {
    let app = hey_doc_api::router(pool.clone());
    let r = call(&app, "POST", "/v1/auth/register", Some(awa()), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["first_name"], "Awa");
    assert_eq!(r.body["whatsapp"], "+2250701020304");
    let set = r.cookie.expect("session cookie");
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("Max-Age=2592000"), "{set}");
    let session = pair(&set);

    let me = call(&app, "GET", "/v1/auth/me", None, Some(&session)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["email"], "awa@exemple.ci");

    let out = call(&app, "POST", "/v1/auth/logout", None, Some(&session)).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert!(out.cookie.unwrap().contains("Max-Age=0"));
    assert_eq!(call(&app, "GET", "/v1/auth/me", None, Some(&session)).await.status, StatusCode::UNAUTHORIZED);

    let login = call(&app, "POST", "/v1/auth/login", Some(json!({ "whatsapp": "00225 0701020304", "password": "mot de passe sûr" })), None).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.body);
    assert_eq!(login.body["city"], "Abidjan");
    let again = pair(&login.cookie.unwrap());
    assert_eq!(call(&app, "GET", "/v1/auth/me", None, Some(&again)).await.status, StatusCode::OK);

    let hash: String = sqlx::query_scalar("SELECT password_hash FROM signups").fetch_one(&pool).await.unwrap();
    assert!(hash.starts_with("$argon2id$"), "password is hashed");
    let stored: String = sqlx::query_scalar("SELECT token_sha256 FROM sessions LIMIT 1").fetch_one(&pool).await.unwrap();
    assert!(!again.ends_with(&stored), "only the token's hash is stored");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn wrong_credentials_and_duplicate_accounts_are_refused(pool: PgPool) {
    let app = hey_doc_api::router(pool.clone());
    assert_eq!(call(&app, "POST", "/v1/auth/register", Some(awa()), None).await.status, StatusCode::CREATED);

    let dup = call(&app, "POST", "/v1/auth/register", Some(awa()), None).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(dup.body["error"], "account_exists");

    for (number, password) in [("+225 07 01 02 03 04", "pas le bon mot"), ("+225 05 55 55 55 55", "mot de passe sûr")] {
        let r = call(&app, "POST", "/v1/auth/login", Some(json!({ "whatsapp": number, "password": password })), None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{number}");
        assert_eq!(r.body["error"], "invalid_credentials", "same answer for an unknown number");
        assert!(r.cookie.is_none());
    }
    assert_eq!(call(&app, "GET", "/v1/auth/me", None, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/v1/auth/me", None, Some("heydoc_session=forged")).await.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_signup_without_password_can_complete_its_account(pool: PgPool) {
    sqlx::query("INSERT INTO signups (id, first_name, last_name, country, city, whatsapp, consent_at) VALUES (gen_random_uuid(), 'Awa', 'Koné', 'CI', 'Bouaké', '+2250701020304', now())")
        .execute(&pool)
        .await
        .unwrap();
    let app = hey_doc_api::router(pool.clone());
    let r = call(&app, "POST", "/v1/auth/register", Some(awa()), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["city"], "Abidjan");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM signups").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn invalid_accounts_are_refused_and_bots_are_ignored(pool: PgPool) {
    let app = hey_doc_api::router(pool.clone());
    for (field, value) in [("consent", json!(false)), ("whatsapp", json!("07 01 02 03 04")), ("first_name", json!("  ")),
                           ("email", json!("awa@")), ("password", json!("court"))] {
        let mut b = awa();
        b[field] = value;
        let r = call(&app, "POST", "/v1/auth/register", Some(b), None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{field}: {}", r.body);
        assert_eq!(r.body["error"], "invalid_account");
    }
    let mut bot = awa();
    bot["website"] = json!("https://spam.example");
    let r = call(&app, "POST", "/v1/auth/register", Some(bot), None).await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(r.cookie.is_none());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM signups").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}
