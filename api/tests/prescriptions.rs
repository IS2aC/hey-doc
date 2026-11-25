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

use futures::StreamExt;
use hey_doc_api::{
    AppState,
    archive::Archive,
    reducto::{BBox, BoxFuture, Image, OcrLine, PrescriptionReader, ReadError, Reducto, TextBlock, Transcript},
};
use object_store::{ObjectStore, ObjectStoreExt, memory::InMemory, path::Path};

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
    photo_request_with(bytes, &[])
}

fn photo_request_with(bytes: &[u8], fields: &[(&str, &str)]) -> Request<Body> {
    let mut body = vec![];
    for (name, value) in fields {
        body.extend_from_slice(format!("--XBOUNDARY\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(b"--XBOUNDARY\r\nContent-Disposition: form-data; name=\"image\"; filename=\"o.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--XBOUNDARY--\r\n");
    Request::post("/v1/prescriptions/read")
        .header("content-type", "multipart/form-data; boundary=XBOUNDARY")
        .body(Body::from(body))
        .unwrap()
}

/// Blocks (type, content), then OCR lines (text, top of the line as a fraction of the page).
struct FakeReader(Vec<(&'static str, &'static str)>, Vec<(&'static str, f64)>);

fn line_box(top: f64) -> BBox {
    BBox { left: 0.05, top, width: 0.9, height: 0.08 }
}

impl PrescriptionReader for FakeReader {
    fn read(&self, _image: Image) -> BoxFuture<'_, Result<Transcript, ReadError>> {
        let blocks = self
            .0
            .iter()
            .map(|(kind, content)| TextBlock {
                kind: (*kind).into(),
                content: (*content).into(),
                bbox: None,
                confidence: Some(0.9),
                low_confidence: false,
            })
            .collect();
        let ocr_lines = self
            .1
            .iter()
            .map(|(text, top)| OcrLine { text: (*text).into(), bbox: line_box(*top), confidence: Some(0.9) })
            .collect();
        Box::pin(async move { Ok(Transcript { blocks, ocr_lines, pages: 1, provider: "fake", model: "test".into() }) })
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
    let app = app(pool, Some(Arc::new(FakeReader(vec![], vec![])))).await;
    let (s, v) = send(&app, photo_request(b"%PDF-1.7 not an image")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_image");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn blurry_photo_asks_for_a_new_one(pool: PgPool) {
    let app = app(pool, Some(Arc::new(FakeReader(vec![("Text", "~ ., 1 i")], vec![])))).await;
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

/// A real white JPEG, so that the archive can decode and mask it.
fn white_jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]));
    let mut out = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
    out
}

async fn archive_app(pool: PgPool, store: Arc<InMemory>) -> Router {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    let reader = FakeReader(
        vec![("Text", "Dr KOUASSI Jean, médecin\nDOLIPRANE 1000 MG N°2\n1 cp 3 fois par jour\nKOUASSI")],
        vec![("Dr KOUASSI Jean, médecin", 0.0), ("DOLIPRANE 1000 MG N°2", 0.3), ("1 cp 3 fois par jour", 0.45), ("KOUASSI", 0.85)],
    );
    hey_doc_api::router_with(AppState {
        pool: pool.clone(),
        reader: Some(Arc::new(reader)),
        archive: Some(Arc::new(Archive::new(store, pool, 30))),
        llm: None,
    })
}

async fn wait_for_rows(pool: &PgPool, n: i64) {
    for _ in 0..60 {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM prescription_reads").fetch_one(pool).await.unwrap();
        if count == n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("archive row not written");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn consented_prescription_is_archived_masked(pool: PgPool) {
    let store = Arc::new(InMemory::new());
    let app = archive_app(pool.clone(), store.clone()).await;
    let (s, v) = send(&app, photo_request_with(&white_jpeg(), &[("consent", "true"), ("channel", "whatsapp")])).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id: uuid::Uuid = v["archive_id"].as_str().unwrap().parse().unwrap();

    // Archiving runs after the response.
    wait_for_rows(&pool, 1).await;
    let (image_key, annotation_key, channel, kept, masked, search_text): (String, String, String, i32, i32, String) =
        sqlx::query_as(
            "SELECT image_key, annotation_key, channel, kept_regions, masked_regions, search_text
             FROM prescription_reads WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((channel.as_str(), kept, masked), ("whatsapp", 2, 2));
    assert!(search_text.contains("DOLIPRANE") && !search_text.contains("KOUASSI"));

    let annotation = store.get(&Path::from(annotation_key.as_str())).await.unwrap().bytes().await.unwrap();
    let annotation = String::from_utf8(annotation.to_vec()).unwrap();
    assert!(!annotation.contains("KOUASSI"), "no name in the annotation");
    assert!(annotation.contains("DOLIPRANE 1000 MG N°2") && annotation.contains("\"to_review\""));

    let image = store.get(&Path::from(image_key.as_str())).await.unwrap().bytes().await.unwrap();
    let image = image::load_from_memory(&image).unwrap().to_rgb8();
    assert!(image.get_pixel(100, 3).0.iter().all(|&c| c < 25), "doctor line masked");
    assert!(image.get_pixel(100, 33).0.iter().all(|&c| c > 230), "product line kept");
    assert!(image.get_pixel(100, 90).0.iter().all(|&c| c < 25), "signature masked");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn nothing_is_archived_without_consent(pool: PgPool) {
    let store = Arc::new(InMemory::new());
    let app = archive_app(pool.clone(), store.clone()).await;
    let (s, v) = send(&app, photo_request(&white_jpeg())).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["archive_id"].is_null());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM prescription_reads").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    assert!(store.list(None).collect::<Vec<_>>().await.is_empty());
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn expired_archives_are_purged(pool: PgPool) {
    let store = Arc::new(InMemory::new());
    let app = archive_app(pool.clone(), store.clone()).await;
    let (_, v) = send(&app, photo_request_with(&white_jpeg(), &[("consent", "oui")])).await;
    assert!(v["archive_id"].is_string(), "{v}");
    wait_for_rows(&pool, 1).await;

    let archive = Archive::new(store.clone(), pool.clone(), 30);
    assert_eq!(archive.purge_expired(chrono::Utc::now()).await.unwrap(), 0);
    assert_eq!(archive.purge_expired(chrono::Utc::now() + chrono::Duration::days(31)).await.unwrap(), 1);
    wait_for_rows(&pool, 0).await;
    assert!(store.list(None).collect::<Vec<_>>().await.is_empty(), "objects deleted too");
}

/// Against a real S3 store: `docker compose up -d minio`, then
/// `S3_ENDPOINT=http://127.0.0.1:9000 S3_ACCESS_KEY=hey_doc S3_SECRET_KEY=hey_doc_local_minio cargo test -- --ignored`.
#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
#[ignore = "needs a running S3 store (MinIO)"]
async fn archive_writes_to_a_real_s3_store(pool: PgPool) {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    let archive = Arc::new(Archive::from_env(pool.clone()).expect("S3_ENDPOINT").unwrap());
    let reader = FakeReader(
        vec![("Text", "Dr KOUASSI Jean\nDOLIPRANE 1000 MG N°2")],
        vec![("Dr KOUASSI Jean", 0.0), ("DOLIPRANE 1000 MG N°2", 0.3)],
    );
    let app = hey_doc_api::router_with(AppState { pool: pool.clone(), reader: Some(Arc::new(reader)), archive: Some(archive.clone()), llm: None });
    let (_, v) = send(&app, photo_request_with(&white_jpeg(), &[("consent", "true")])).await;
    assert!(v["archive_id"].is_string(), "{v}");
    wait_for_rows(&pool, 1).await;
    assert_eq!(archive.purge_expired(chrono::Utc::now() + chrono::Duration::days(400)).await.unwrap(), 1);
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn test_route_returns_the_text_read_by_reducto(pool: PgPool) {
    let reader = FakeReader(vec![("Header", "Clinique X"), ("Text", "1. Doliprane 1000 mg\n- Spasfon 80")], vec![]);
    let app = app(pool, Some(Arc::new(reader))).await;
    let req = photo_request(JPEG);
    let (parts, body) = req.into_parts();
    let mut req = Request::from_parts(parts, body);
    *req.uri_mut() = "/v1/test/reducto-text".parse().unwrap();
    let (s, v) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["text"], "Clinique X\n1. Doliprane 1000 mg\n- Spasfon 80");
    assert_eq!(v["lines"], json!(["Clinique X", "Doliprane 1000 mg", "Spasfon 80"]));
    assert_eq!(v["blocks"][0]["kind"], "Header");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn test_route_finds_products_by_cosine_similarity(pool: PgPool) {
    let app = app(pool, None).await;
    let req = Request::post("/v1/test/similar-products")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "text": "Name of Patient\nDolipane 1000 mg\nSpasfon lyoc 80", "per_line": 2 }).to_string()))
        .unwrap();
    let (s, v) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["catalogue_size"], 5);
    assert_eq!(v["method"], "cosine_char_trigrams");
    assert!(v["lines"][0]["matches"].as_array().unwrap().is_empty());
    assert_eq!(v["lines"][1]["matches"][0]["id"], 1);
    assert_eq!(v["lines"][2]["matches"][0]["id"], 5);
    let top = &v["products"][0];
    assert!(top["score"].as_f64().unwrap() > 0.5);
    assert!(top["lines"].is_array());
}

/// A local stand-in for Ollama Cloud's /api/chat: the real adapter talks to it.
#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn llm_route_structures_checks_and_matches(pool: PgPool) {
    sqlx::raw_sql(FIXTURE).execute(&pool).await.unwrap();
    let fake = Router::new().route(
        "/api/chat",
        post(|h: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(h.get("authorization").unwrap(), "Bearer cloud-key");
            assert_eq!(body["model"], "test-model");
            assert_eq!(body["stream"], false);
            assert_eq!(body["options"]["temperature"], 0);
            assert!(body["format"]["properties"]["items"].is_object(), "JSON schema imposed");
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("N'invente rien"));
            assert!(body["messages"][1]["content"].as_str().unwrap().contains("Dolipane 1000 mg"));
            let answer = json!({
                "items": [
                    { "source_text": "Dolipane 1000 mg N°2", "name": "Dolipane", "ingredients": [], "strengths": ["1000 mg"],
                      "form": null, "boxes": 2, "units": null, "therapeutic_class": "Antalgique et antipyrétique" },
                    { "source_text": "Augmentin 1 g", "name": "Augmentin", "ingredients": ["amoxicilline"], "strengths": ["1 g"],
                      "form": "sachet", "boxes": null, "units": null, "therapeutic_class": "Antibiotique" }
                ],
                "ignored": [ { "text": "Dr [nom]", "reason": "identite" } ]
            });
            Json(json!({
                "model": "test-model",
                "message": { "role": "assistant", "content": format!("```json\n{answer}\n```") },
                "prompt_eval_count": 812, "eval_count": 164, "total_duration": 2_500_000_000u64
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let llm = hey_doc_api::llm::Llm::new(base, Some("cloud-key".into()), "test-model");
    let app = hey_doc_api::router_with(AppState { pool, reader: None, archive: None, llm: Some(Arc::new(llm)) });
    let req = Request::post("/v1/test/llm-structure")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "text": "Dr Kouassi\nDolipane 1000 mg N°2\n1 cp 3 fois par jour" }).to_string()))
        .unwrap();
    let (s, v) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["usage"], json!({ "model": "test-model", "prompt_tokens": 812, "completion_tokens": 164, "duration_ms": 2500 }));
    assert_eq!(v["items"][0]["grounded"], true);
    assert_eq!(v["items"][0]["boxes"], 2);
    assert_eq!(v["items"][0]["matches"][0]["id"], 1, "{v}");
    assert_eq!(v["items"][0]["matches"][0]["strength_match"], true);
    assert_eq!(v["items"][0]["therapeutic_class"], "Antalgique et antipyrétique");
    assert!(v.get("language").is_none());
    assert_eq!(v["items"][1]["grounded"], false, "invented by the model");
    assert_eq!(v["ignored"][0]["text"], "Dr [nom]");
}

#[sqlx::test(migrator = "hey_doc_api::MIGRATOR")]
async fn llm_route_without_a_model_is_unavailable(pool: PgPool) {
    let app = app(pool, None).await;
    let req = Request::post("/v1/test/llm-structure")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "text": "Doliprane" }).to_string()))
        .unwrap();
    let (s, v) = send(&app, req).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"], "llm_unavailable");
}
