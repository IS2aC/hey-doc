use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

// Fictitious data. Sector 1 has a published night schedule from Sunday 22 Nov 2026 19:00
// to Monday 08:00, then a day schedule with no pharmacy listed. Sector 2 has no schedule.
const FIXTURE: &str = r#"
INSERT INTO sources (id, publisher, reference, published_on) OVERRIDING SYSTEM VALUE VALUES
  (1, 'Relevé officine', 'releve-2026-11', NULL),
  (2, 'MUGEF-CI', 'Liste septembre 2026', '2026-09-11'),
  (3, 'CNAM', 'Décision HTA diabète 2025', '2025-09-29'),
  (4, 'Planning de garde', 'cocody-s47', '2026-11-16');
INSERT INTO presentations (id, label, ingredient, strength, dosage_form, pack) OVERRIDING SYSTEM VALUE VALUES
  (1, 'Amoxicilline Démo', 'Amoxicilline', '500 mg', 'Gélule', 'Boîte de 12'),
  (2, 'Paracétamol Démo', 'Paracétamol', '1000 mg', 'Comprimé', 'Boîte de 8'),
  (3, 'Metformine Démo', 'Metformine', '850 mg', 'Comprimé', 'Boîte de 30');
INSERT INTO sectors (id, city, district) OVERRIDING SYSTEM VALUE VALUES
  (1, 'Abidjan', 'Cocody'), (2, 'Abidjan', 'Yopougon');
INSERT INTO pharmacies (id, sector_id, name, phone, address, latitude, longitude) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, 'Pharmacie des Lagunes', '+225 00 00 00 00', 'Près du carrefour', 5.359952, -3.986160),
  (2, 1, 'Pharmacie Riviera', NULL, NULL, NULL, NULL);
INSERT INTO prices (presentation_id, source_id, sector_id, pharmacy_id, kind, amount_fcfa, observed_on) VALUES
  (1, 1, 1, 1, 'cash', 2400, '2026-11-20'),
  (1, 1, 1, 2, 'cash', 2600, '2026-11-20'),
  (1, 1, 1, 1, 'cash', 2000, '2026-10-01'),
  (2, 1, 1, 1, 'cash', 1500, '2026-11-18');
INSERT INTO prices (presentation_id, source_id, kind, amount_fcfa, observed_on) VALUES
  (1, 2, 'mugefci_published', 2100, '2026-09-11');
INSERT INTO coverage_listings (id, insurer, scheme, source_id) OVERRIDING SYSTEM VALUE VALUES (1, 'CNAM', 'CMU', 3);
INSERT INTO coverage_entries (listing_id, presentation_id) VALUES (1, 3);
INSERT INTO duty_schedules (id, sector_id, source_id, period) OVERRIDING SYSTEM VALUE VALUES
  (1, 1, 4, '[2026-11-22 19:00+00, 2026-11-23 08:00+00)'),
  (2, 1, 4, '[2026-11-23 08:00+00, 2026-11-23 19:00+00)');
INSERT INTO pharmacy_duties (schedule_id, sector_id, schedule_period, pharmacy_id, period) VALUES
  (1, 1, '[2026-11-22 19:00+00, 2026-11-23 08:00+00)', 1, '[2026-11-22 19:00+00, 2026-11-23 08:00+00)');
"#;

async fn app(pool: PgPool) -> axum::Router {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    hey_doc_api::router(pool)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn search_tolerates_partial_names_and_typos(pool: PgPool) {
    let app = app(pool).await;
    let (s, v) = call(&app, "GET", "/v1/products/search?q=amoxi", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["results"][0]["id"], 1);
    assert_eq!(v["results"][0]["strength"], "500 mg");

    let (_, v) = call(&app, "GET", "/v1/products/search?q=Amoxiciline", None).await;
    assert_eq!(v["results"][0]["id"], 1);

    let (s, v) = call(&app, "GET", "/v1/products/search?q=a", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "query_too_short");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn prices_are_filtered_by_kind(pool: PgPool) {
    let app = app(pool).await;
    let (s, v) = call(&app, "GET", "/v1/presentations/1/prices", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["kind"], "cash");
    assert_eq!(v["prices"].as_array().unwrap().len(), 3);
    assert_eq!(v["prices"][0]["observed_on"], "2026-11-20");

    let (_, v) = call(
        &app,
        "GET",
        "/v1/presentations/1/prices?kind=mugefci_published",
        None,
    )
    .await;
    assert_eq!(v["prices"][0]["amount_fcfa"], "2100.00");
    assert_eq!(v["prices"][0]["sector_id"], Value::Null);

    let (s, _) = call(&app, "GET", "/v1/presentations/1/prices?kind=free", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(&app, "GET", "/v1/presentations/999/prices", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn coverage_distinguishes_absent_from_unavailable(pool: PgPool) {
    let app = app(pool).await;
    let (_, v) = call(&app, "GET", "/v1/presentations/3/coverage?scheme=cmu", None).await;
    assert_eq!(v["status"], "listed");
    assert_eq!(v["listings"][0]["source"]["published_on"], "2025-09-29");

    let (_, v) = call(&app, "GET", "/v1/presentations/1/coverage?scheme=cmu", None).await;
    assert_eq!(v["status"], "not_listed");

    let (_, v) = call(
        &app,
        "GET",
        "/v1/presentations/1/coverage?scheme=mugef",
        None,
    )
    .await;
    assert_eq!(v["status"], "list_unavailable");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn coverage_counts_every_listing_of_the_latest_edition(pool: PgPool) {
    let app = app(pool.clone()).await;
    // MUGEF-CI 2026 publishes two schemes in the same edition: listed in either one is enough.
    sqlx::raw_sql(
        "INSERT INTO coverage_listings (id, insurer, scheme, source_id) OVERRIDING SYSTEM VALUE VALUES
           (2, 'MUGEF-CI', 'IVSPL', 2), (3, 'MUGEF-CI', 'RCO', 2);
         INSERT INTO coverage_entries (listing_id, presentation_id) VALUES (3, 2);
         UPDATE sources SET published_on = '2026-12-01' WHERE id = 2;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, v) = call(&app, "GET", "/v1/presentations/2/coverage", None).await;
    assert_eq!(v["status"], "listed");
    let (_, v) = call(&app, "GET", "/v1/presentations/1/coverage", None).await;
    assert_eq!(v["status"], "not_listed");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn manual_prescription_gives_a_total(pool: PgPool) {
    let app = app(pool).await;
    let body = json!({ "sector_id": 1, "lines": [
        { "presentation_id": 1, "quantity": 2 },
        { "presentation_id": 2, "quantity": 1 }
    ]});
    let (s, v) = call(&app, "POST", "/v1/estimates", Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    // Same-day tie in the sector: the higher price (2600) is used.
    assert_eq!(v["lines"][0]["unit_price_fcfa"], "2600.00");
    assert_eq!(v["lines"][0]["line_total_fcfa"], "5200.00");
    assert_eq!(v["lines"][0]["price"]["pharmacy_name"], "Pharmacie Riviera");
    assert_eq!(v["amount_kind"], "total");
    assert_eq!(v["amount_fcfa"], "6700.00");

    let body = json!({ "sector_id": 1, "pharmacy_id": 1, "lines": [{ "presentation_id": 1, "quantity": 1 }]});
    let (_, v) = call(&app, "POST", "/v1/estimates", Some(body)).await;
    assert_eq!(v["amount_fcfa"], "2400.00");
    assert_eq!(v["scope"]["pharmacy_name"], "Pharmacie des Lagunes");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn partial_estimate_is_a_subtotal_with_reasons(pool: PgPool) {
    let app = app(pool).await;
    let body = json!({ "sector_id": 1, "lines": [
        { "presentation_id": 1, "quantity": 2 },
        { "presentation_id": 3, "quantity": 1 },
        { "presentation_id": 2, "quantity": null },
        { "presentation_id": 999, "quantity": 1 }
    ]});
    let (_, v) = call(&app, "POST", "/v1/estimates", Some(body)).await;
    assert_eq!(v["amount_kind"], "subtotal");
    assert_eq!(v["amount_fcfa"], "5200.00");
    assert_eq!(v["priced_lines"], 1);
    assert_eq!(v["lines"][1]["exclusion"], "no_cash_price");
    assert_eq!(v["lines"][2]["exclusion"], "quantity_unconfirmed");
    assert_eq!(v["lines"][2]["unit_price_fcfa"], "1500.00");
    assert_eq!(v["lines"][2]["line_total_fcfa"], Value::Null);
    assert_eq!(v["lines"][3]["exclusion"], "unknown_presentation");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn estimate_without_any_cash_price_shows_no_amount(pool: PgPool) {
    let app = app(pool).await;
    let body = json!({ "sector_id": 2, "lines": [{ "presentation_id": 1, "quantity": 1 }]});
    let (s, v) = call(&app, "POST", "/v1/estimates", Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["amount_kind"], "none");
    assert_eq!(v["amount_fcfa"], Value::Null);

    let bad = [
        (
            json!({ "sector_id": 1, "lines": [] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "sector_id": 1, "lines": [{ "presentation_id": 1, "quantity": 0 }] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "sector_id": 2, "pharmacy_id": 1, "lines": [{ "presentation_id": 1, "quantity": 1 }] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "sector_id": 9, "lines": [{ "presentation_id": 1, "quantity": 1 }] }),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (body, expected) in bad {
        let (s, _) = call(&app, "POST", "/v1/estimates", Some(body)).await;
        assert_eq!(s, expected);
    }
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn duty_on_sunday_night_and_after_midnight(pool: PgPool) {
    let app = app(pool).await;
    for at in [
        "2026-11-22T21:00",
        "2026-11-23T02:30",
        "2026-11-23T07:59:59Z",
    ] {
        let (s, v) = call(
            &app,
            "GET",
            &format!("/v1/duty-pharmacies?sector=1&at={at}"),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{at}");
        assert_eq!(v["status"], "on_duty", "{at}");
        assert_eq!(v["pharmacies"][0]["name"], "Pharmacie des Lagunes");
        assert_eq!(v["pharmacies"][0]["duty_ends_at"], "2026-11-23T08:00:00Z");
        assert!(
            v["pharmacies"][0]["directions_url"]
                .as_str()
                .unwrap()
                .contains("5.359952,-3.986160")
        );
    }
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn duty_distinguishes_empty_schedule_from_missing_schedule(pool: PgPool) {
    let app = app(pool).await;
    // End of the night duty is excluded; the day schedule lists nobody. +01:00 offset = 08:00 Abidjan.
    let (_, v) = call(
        &app,
        "GET",
        "/v1/duty-pharmacies?sector=1&at=2026-11-23T09:00:00%2B01:00",
        None,
    )
    .await;
    assert_eq!(v["status"], "none_in_schedule");
    assert_eq!(v["pharmacies"].as_array().unwrap().len(), 0);

    let (_, v) = call(
        &app,
        "GET",
        "/v1/duty-pharmacies?sector=2&at=2026-11-22T21:00",
        None,
    )
    .await;
    assert_eq!(v["status"], "no_schedule");

    let (s, _) = call(&app, "GET", "/v1/duty-pharmacies?sector=1&at=demain", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(&app, "GET", "/v1/duty-pharmacies?sector=42", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn docs_serve_swagger_and_the_openapi_spec(pool: PgPool) {
    let app = hey_doc_api::router(pool);
    let (s, spec) = call(&app, "GET", "/docs/openapi.json", None).await;
    assert_eq!(s, StatusCode::OK);
    for path in [
        "/health",
        "/v1/products/search",
        "/v1/presentations/{id}/prices",
        "/v1/presentations/{id}/coverage",
        "/v1/estimates",
        "/v1/duty-pharmacies",
    ] {
        assert!(spec["paths"][path].is_object(), "{path} absent de la spec");
    }
    let res = app
        .clone()
        .oneshot(Request::get("/docs/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
