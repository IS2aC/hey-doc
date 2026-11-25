use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::post,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use hey_doc_api::{
    AppState,
    reducto::{BoxFuture, Image, PrescriptionReader, ReadError, TextBlock, Transcript},
};

const FIXTURE: &str = r#"
INSERT INTO presentations (id, label, ingredient, strength, dosage_form, pack) OVERRIDING SYSTEM VALUE VALUES
  (1, 'DOLIPRANE 1000MG CP B/8', 'PARACETAMOL', '1000MG', 'Comprimé', 'B/8'),
  (2, 'DOLIPRANE 500MG CP B/16', 'PARACETAMOL', '500MG', 'Comprimé', 'B/16');
"#;

struct FakeReader;

impl PrescriptionReader for FakeReader {
    fn read(&self, _image: Image) -> BoxFuture<'_, Result<Transcript, ReadError>> {
        let blocks = ["Dr Kouassi", "Doliprane 1000 mg N°2", "1 cp 3 fois par jour"]
            .into_iter()
            .map(|c| TextBlock { kind: "Text".into(), content: c.into(), bbox: None, confidence: Some(0.9), low_confidence: false })
            .collect();
        Box::pin(async move { Ok(Transcript { blocks, ocr_lines: vec![], pages: 1, provider: "fake", model: "test".into() }) })
    }
}

/// A local stand-in for Ollama: it answers with the number of messages it was given.
async fn fake_llm(calls: Arc<AtomicUsize>) -> hey_doc_api::llm::Llm {
    let fake = Router::new().route(
        "/api/chat",
        post(move |Json(body): Json<Value>| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let messages = body["messages"].as_array().unwrap();
                assert_eq!(messages[0]["role"], "system");
                assert!(messages[0]["content"].as_str().unwrap().contains("Tu parles à Awa"));
                assert!(body.get("format").is_none(), "free answer, no schema");
                Json(json!({ "model": "test-model", "message": { "role": "assistant", "content": format!("Réponse à {} messages", messages.len() - 1) },
                             "prompt_eval_count": 10, "eval_count": 5, "total_duration": 1_000_000u64 }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    hey_doc_api::llm::Llm::new(base, None, "test-model")
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>, cookie: &str) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header(header::COOKIE, cookie);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Creates an account and returns its session cookie, "heydoc_session=…".
async fn sign_up(app: &Router, first_name: &str, number: &str) -> String {
    let body = json!({ "first_name": first_name, "last_name": "Koné", "country": "CI", "city": "Abidjan",
                       "whatsapp": number, "email": null, "password": "mot de passe sûr", "consent": true });
    let req = Request::post("/v1/auth/register").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    res.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned()
}

async fn app(pool: PgPool, llm: Option<hey_doc_api::llm::Llm>, reader: bool) -> Router {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    let reader = reader.then(|| Arc::new(FakeReader) as Arc<dyn PrescriptionReader>);
    hey_doc_api::router_with(AppState { pool, reader, archive: None, llm: llm.map(Arc::new) })
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_conversation_keeps_its_history(pool: PgPool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(pool, Some(fake_llm(calls.clone()).await), false).await;
    let awa = sign_up(&app, "Awa", "+225 07 01 02 03 04").await;

    let (s, v) = call(&app, "POST", "/v1/chat/messages", Some(json!({ "text": "Comment trouver une pharmacie de garde ?" })), &awa).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["conversation"]["title"], "Comment trouver une pharmacie de garde ?");
    assert_eq!(v["messages"][0]["role"], "user");
    assert_eq!(v["messages"][1]["content"], "Réponse à 1 messages");
    let id = v["conversation"]["id"].as_str().unwrap().to_owned();

    let (s, v) = call(&app, "POST", "/v1/chat/messages", Some(json!({ "conversation_id": id, "text": "Et à Cocody ?" })), &awa).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["messages"][1]["content"], "Réponse à 3 messages", "the model sees the previous exchange");

    let (s, list) = call(&app, "GET", "/v1/chat/conversations", None, &awa).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, conv) = call(&app, "GET", &format!("/v1/chat/conversations/{id}"), None, &awa).await;
    assert_eq!(conv["messages"].as_array().unwrap().len(), 4);

    // Another account sees nothing of it.
    let moussa = sign_up(&app, "Moussa", "+225 05 05 05 05 05").await;
    assert_eq!(call(&app, "GET", &format!("/v1/chat/conversations/{id}"), None, &moussa).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", "/v1/chat/messages", Some(json!({ "conversation_id": id, "text": "Bonjour" })), &moussa).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &format!("/v1/chat/conversations/{id}"), None, &moussa).await.0, StatusCode::NOT_FOUND);

    assert_eq!(call(&app, "DELETE", &format!("/v1/chat/conversations/{id}"), None, &awa).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/v1/chat/conversations", None, &awa).await.1, json!([]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_typed_medicine_is_matched_without_the_model(pool: PgPool) {
    let app = app(pool, None, false).await;
    let awa = sign_up(&app, "Awa", "+225 07 01 02 03 04").await;
    let (s, v) = call(&app, "POST", "/v1/chat/messages", Some(json!({ "text": "Doliprane 1000 mg N°2" })), &awa).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let reply = &v["messages"][1];
    assert_eq!(reply["kind"], "prescription");
    assert!(reply["content"].as_str().unwrap().contains("DOLIPRANE 1000MG CP B/8"), "{reply}");
    assert_eq!(reply["data"]["lines"][0]["candidates"][0]["id"], 1);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_photo_is_read_and_not_kept(pool: PgPool) {
    let app = app(pool.clone(), None, true).await;
    let awa = sign_up(&app, "Awa", "+225 07 01 02 03 04").await;
    let boundary = "x-boundary";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"o.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes();
    body.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F']);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::post("/v1/chat/photo")
        .header(header::COOKIE, &awa)
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let v: Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, StatusCode::CREATED, "{v}");
    assert_eq!(v["messages"][0]["kind"], "photo");
    assert_eq!(v["messages"][1]["kind"], "prescription");
    assert_eq!(v["messages"][1]["data"]["lines"][0]["quantity"], 2);
    assert!(v["conversation"]["title"].as_str().unwrap().starts_with("Ordonnance du "));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE content LIKE '%Kouassi%'").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0, "the doctor's name is not kept");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn the_chat_needs_a_session_and_stores_nothing_on_failure(pool: PgPool) {
    let app = app(pool.clone(), None, false).await;
    assert_eq!(call(&app, "POST", "/v1/chat/messages", Some(json!({ "text": "Bonjour" })), "").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/v1/chat/conversations", None, "heydoc_session=forged").await.0, StatusCode::UNAUTHORIZED);
    let awa = sign_up(&app, "Awa", "+225 07 01 02 03 04").await;
    let (s, v) = call(&app, "POST", "/v1/chat/messages", Some(json!({ "text": "Bonjour" })), &awa).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM conversations").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}

const PRICED: &str = r#"
INSERT INTO presentations (id, label, ingredient, strength, dosage_form, pack) OVERRIDING SYSTEM VALUE VALUES
  (10, 'AMOCLAN FORTE CP 500 MG/125 MG B/15', 'AMOXICILLINE+ACIDE CLAVULANIQUE', '500MG/125MG', 'Comprimé', 'B/15'),
  (11, 'TICASSE CP 500 MG/125 MG B/15', 'AMOXICILLINE + ACIDE CLAVULANIQUE', '500MG/125MG', 'Comprimé', 'B/15'),
  (12, 'TRYDROX CV CP PELL 500MG/125MG B/10', 'AMOXICILLINE + ACIDE CLAVULANIQUE', '500MG/125MG', 'Comprimé', 'B/10'),
  (13, 'CO-AMOXICLAV SP ENFANT PDRE POUR SUSP BUVABLE 500MG/125MG B/12', 'AMOXICILLINE + ACIDE CLAVULANIQUE', '500MG/125MG', 'Suspension', 'B/12'),
  (14, 'ACECLAV 1G CP B/12', 'AMOXICILLINE + ACIDE CLAVULANIQUE', '1G/125MG', 'Comprimé', 'B/12');
INSERT INTO sources (id, publisher, reference, published_on) OVERRIDING SYSTEM VALUE VALUES (1, 'MUGEF-CI', 'liste-2026-09', '2026-09-08');
INSERT INTO prices (presentation_id, source_id, kind, amount_fcfa, observed_on) VALUES
  (10, 1, 'mugefci_published', 5955, '2026-09-08'),
  (10, 1, 'mugefci_published', 9999, '2024-12-18'),
  (11, 1, 'mugefci_published', 5070, '2026-09-08'),
  (12, 1, 'mugefci_published', 5250, '2026-09-08'),
  (13, 1, 'mugefci_published', 1000, '2026-09-08'),
  (14, 1, 'mugefci_published', 900, '2026-09-08');
"#;

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn a_prescription_gets_a_price_range_over_equivalent_brands(pool: PgPool) {
    sqlx::raw_sql(PRICED).execute(&pool).await.unwrap();
    let app = app(pool, None, false).await;
    let awa = sign_up(&app, "Awa", "+225 07 01 02 03 04").await;
    let (s, v) = call(&app, "POST", "/v1/chat/messages", Some(json!({ "text": "Amoclan forte 500/125 mg cp\n#21\nSig. Take one with food" })), &awa).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let price = &v["messages"][1]["data"]["lines"][0]["price"];
    assert_eq!(price["basis"], "units", "{price}");
    // 21 tablets: 2 boxes of 15 or 3 boxes of 10. The suspension and the 1 g tablet are not equivalent.
    assert_eq!(price["count"], 3, "{price}");
    assert_eq!(price["min_fcfa"], "10140.00", "TICASSE, 2 boxes");
    assert_eq!(price["max_fcfa"], "15750.00", "TRYDROX, 3 boxes");
    assert_eq!(price["options"][1]["label"], "AMOCLAN FORTE CP 500 MG/125 MG B/15");
    assert_eq!(price["options"][1]["box_price_fcfa"], "5955.00", "latest published price");
    assert_eq!(price["published_on"], "2026-09-08");
    let estimate = &v["messages"][1]["data"]["estimate"];
    assert_eq!(estimate["priced_lines"], 1);
    assert_eq!(estimate["source"], "mugefci_published");
    let content = v["messages"][1]["content"].as_str().unwrap();
    assert!(content.contains("10\u{202f}140 à 15\u{202f}750 FCFA"), "{content}");
}

