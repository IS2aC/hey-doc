use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    routing::{delete, post},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use hey_doc_api::reducto::{BoxFuture, Image, PrescriptionReader, ReadError, Reducto, TextBlock, Transcript};

const FIXTURE: &str = r#"
INSERT INTO presentations (id, label, ingredient, strength, dosage_form, pack) OVERRIDING SYSTEM VALUE VALUES
  (1, 'DOLIPRANE 1000MG CP B/8', 'PARACETAMOL', '1000MG', 'Comprimé', 'B/8'),
  (2, 'DOLIPRANE 500MG CP B/16', 'PARACETAMOL', '500MG', 'Comprimé', 'B/16'),
  (3, 'AMOXICILLINE DENK 500MG GELULE B/12', 'AMOXICILLINE', '500MG', 'Gélule', 'B/12'),
  (4, 'AMOXICILLINE CIPHARM 500MG GELULE B/12', 'AMOXICILLINE', '500MG', 'Gélule', 'B/12'),
  (5, 'SPASFON LYOC 80MG B/10', 'PHLOROGLUCINOL', '80MG', 'Lyophilisat', 'B/10');
"#;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn text_request(text: &str) -> Request<Body> {
    Request::post("/v1/prescriptions/match")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "text": text }).to_string()))
        .unwrap()
}

fn photo_request(bytes: &[u8]) -> Request<Body> {
    let mut body = b"--XBOUNDARY\r\nContent-Disposition: form-data; name=\"image\"; filename=\"o.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--XBOUNDARY--\r\n");
    Request::post("/v1/prescriptions/read")
        .header("content-type", "multipart/form-data; boundary=XBOUNDARY")
        .body(Body::from(body))
        .unwrap()
}

struct FakeReader(Vec<(&'static str, &'static str)>);

impl PrescriptionReader for FakeReader {
    fn read(&self, _image: Image) -> BoxFuture<'_, Result<Transcript, ReadError>> {
        let blocks = self
            .0
            .iter()
            .map(|(kind, content)| TextBlock {
                kind: (*kind).into(),
                content: (*content).into(),
                confidence: Some(0.9),
                low_confidence: false,
            })
            .collect();
        Box::pin(async move { Ok(Transcript { blocks, pages: 1, provider: "fake", model: "test".into() }) })
    }
}

async fn app(pool: PgPool, reader: Option<Arc<dyn PrescriptionReader>>) -> Router {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    hey_doc_api::router_with_reader(pool, reader)
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn typed_prescription_is_split_into_matched_lines(pool: PgPool) {
    let app = app(pool, None).await;
    let text = "Dr KOUASSI, médecin généraliste\nDOLIPRANE 1000 MG N°2\n1 cp 3 fois par jour\n2. Amoxicilline 500 mg x1 matin et soir\nXyzzyqrt 50 mg";
    let (s, v) = send(&app, text_request(text)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "read");
    assert_eq!(v["ignored_lines"], 1);
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);

    assert_eq!(lines[0]["text"], "DOLIPRANE 1000 MG N°2");
    assert_eq!(lines[0]["status"], "matched");
    assert_eq!(lines[0]["candidates"][0]["id"], 1);
    assert_eq!(lines[0]["quantity"], 2);
    assert_eq!(lines[0]["instructions"], json!(["1 cp 3 fois par jour"]));

    // Two brands with the same strength and pack: the person chooses.
    assert_eq!(lines[1]["status"], "ambiguous");
    assert_eq!(lines[1]["quantity"], 1);
    assert_eq!(lines[1]["instructions"], json!(["matin et soir"]));
    let ids: Vec<i64> = lines[1]["candidates"].as_array().unwrap().iter().take(2).map(|c| c["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&3) && ids.contains(&4), "{ids:?}");

    assert_eq!(lines[2]["status"], "no_match");
    assert!(lines[2]["quantity"].is_null());
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn handwriting_typos_still_find_the_product(pool: PgPool) {
    let app = app(pool, None).await;
    let (_, v) = send(&app, text_request("Dolipran 500mg")).await;
    assert_eq!(v["lines"][0]["candidates"][0]["id"], 2, "{v}");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn photo_reading_without_a_key_is_unavailable(pool: PgPool) {
    let app = app(pool, None).await;
    let (s, v) = send(&app, photo_request(JPEG)).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"], "reader_unavailable");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn photo_must_be_an_image(pool: PgPool) {
    let app = app(pool, Some(Arc::new(FakeReader(vec![])))).await;
    let (s, v) = send(&app, photo_request(b"%PDF-1.7 not an image")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_image");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn blurry_photo_asks_for_a_new_one(pool: PgPool) {
    let app = app(pool, Some(Arc::new(FakeReader(vec![("Text", "~ ., 1 i")])))).await;
    let (s, v) = send(&app, photo_request(JPEG)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "unreadable");
    assert!(v["message"].as_str().unwrap().contains("Reprenez"));
}

/// A local stand-in for Reducto: the real HTTP client talks to it.
#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn reducto_client_uploads_parses_retries_and_cleans_up(pool: PgPool) {
    let parse_calls = Arc::new(AtomicUsize::new(0));
    let deletes = Arc::new(AtomicUsize::new(0));
    let fake = {
        let parse_calls = parse_calls.clone();
        let deletes = deletes.clone();
        let authorized = |h: &HeaderMap| h.get("authorization").is_some_and(|v| v == "Bearer test-key");
        Router::new()
            .route(
                "/upload",
                post(move |h: HeaderMap| async move {
                    assert!(authorized(&h));
                    Json(json!({ "file_id": "reducto://abc.jpg", "presigned_url": null }))
                }),
            )
            .route(
                "/parse",
                post(move |h: HeaderMap, Json(body): Json<Value>| async move {
                    assert!(authorized(&h));
                    assert_eq!(body["input"], "reducto://abc.jpg");
                    assert_eq!(body["settings"]["model"], "r-1");
                    if parse_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": { "message": "busy" } })));
                    }
                    (StatusCode::OK, Json(json!({
                        "job_id": "job-1", "duration": 2.1,
                        "usage": { "num_pages": 1, "credits": 1.0 },
                        "result": { "type": "full", "chunks": [{ "content": "", "embed": "", "enriched": null, "blocks": [
                            { "type": "Header", "content": "Clinique des Lagunes", "confidence": "high" },
                            { "type": "Text", "content": "Le 20/11/2026\nSpasfon lyoc 80 N°1\n1 cp si douleur",
                              "confidence": "low", "granular_confidence": { "parse_confidence": 0.41, "extract_confidence": null },
                              "bbox": { "left": 0.1, "top": 0.3, "width": 0.6, "height": 0.2, "page": 1, "original_page": 1 } }
                        ]}]}
                    })))
                }),
            )
            .route("/upload/{id}", delete({ let d = deletes.clone(); move || async move { d.fetch_add(1, Ordering::SeqCst); StatusCode::OK } }))
            .route("/job/{id}", delete(move || async move { deletes.fetch_add(1, Ordering::SeqCst); StatusCode::OK }))
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let app = app(pool, Some(Arc::new(Reducto::new(base, "test-key", "r-1")))).await;
    let (s, v) = send(&app, photo_request(JPEG)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(parse_calls.load(Ordering::SeqCst), 2, "one retry after 503");
    assert_eq!(deletes.load(Ordering::SeqCst), 2, "upload and job deleted");
    assert_eq!(v["reader"], json!({ "provider": "reducto", "model": "r-1", "pages": 1 }));
    assert_eq!(v["ignored_lines"], 1);
    let line = &v["lines"][0];
    assert_eq!(line["text"], "Spasfon lyoc 80 N°1");
    assert_eq!(line["status"], "matched");
    assert_eq!(line["candidates"][0]["id"], 5);
    assert_eq!(line["quantity"], 1);
    assert_eq!(line["low_confidence"], true);
    assert_eq!(line["instructions"], json!(["1 cp si douleur"]));
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn product_written_over_two_lines_in_english(pool: PgPool) {
    let app = app(pool, None).await;
    let text = "Name of Patient\nParacetamol\n(Doliprane) 1000 mg/tab #16\nSig. Take one every 6 hours\nMark-Lawrence\nLic. No. 12";
    let (_, v) = send(&app, text_request(text)).await;
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1, "{v}");
    assert_eq!(lines[0]["text"], "Paracetamol (Doliprane) 1000 mg/tab #16");
    assert_eq!(lines[0]["candidates"][0]["id"], 1);
    assert_eq!(lines[0]["units"], 16);
    assert!(lines[0]["quantity"].is_null());
    assert_eq!(lines[0]["instructions"], json!(["Sig. Take one every 6 hours"]));
    assert_eq!(v["ignored_lines"], 3);
}
