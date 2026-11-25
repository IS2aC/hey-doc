use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn post(app: &Router, body: Value) -> (StatusCode, Value) {
    let req = Request::post("/v1/signups").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn awa() -> Value {
    json!({ "first_name": "  Awa ", "last_name": "Koné", "country": "ci", "city": "Abidjan",
            "whatsapp": "+225 07 01 02 03 04", "email": "Awa@Exemple.ci", "consent": true, "website": "" })
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_signup_is_stored_once_per_whatsapp_number(pool: PgPool) {
    let app = hey_doc_api::router(pool.clone());
    let (s, v) = post(&app, awa()).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["status"], "registered");

    let mut again = awa();
    again["city"] = json!("Bouaké");
    again["whatsapp"] = json!("00225 0701020304");
    let (s, v) = post(&app, again).await;
    assert_eq!(s, StatusCode::CREATED, "same answer for a number already registered: {v}");

    let rows: Vec<(String, String, String, String, Option<String>)> =
        sqlx::query_as("SELECT first_name, country, city, whatsapp, email FROM signups").fetch_all(&pool).await.unwrap();
    assert_eq!(rows, vec![("Awa".into(), "CI".into(), "Bouaké".into(), "+2250701020304".into(), Some("awa@exemple.ci".into()))]);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn invalid_signups_are_refused_and_bots_are_ignored(pool: PgPool) {
    let app = hey_doc_api::router(pool.clone());
    for (field, value) in [("consent", json!(false)), ("whatsapp", json!("07 01 02 03 04")), ("first_name", json!("  ")), ("email", json!("awa@"))] {
        let mut b = awa();
        b[field] = value;
        let (s, v) = post(&app, b).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{field}: {v}");
        assert_eq!(v["error"], "invalid_signup");
    }
    let mut bot = awa();
    bot["website"] = json!("https://spam.example");
    let (s, _) = post(&app, bot).await;
    assert_eq!(s, StatusCode::CREATED);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM signups").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}
