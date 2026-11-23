//! Reducto Parse adapter: sends a prescription photo and gets back text blocks.
//!
//! Flow: `POST /upload` (multipart), `POST /parse` with the `reducto://` id and model r-1, then
//! best-effort deletion of the upload and the job. Temporary failures (408, 409, 429, 5xx,
//! timeouts) are retried twice with backoff. Prescription text is never logged.

use std::{future::Future, pin::Pin, time::Duration};

use reqwest::{StatusCode, multipart};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A photo checked by the route: its bytes and a file extension Reducto understands.
#[derive(Clone)]
pub struct Image {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// A box in fractions of the page, `top` = 0 at the top edge.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One line as located by the reader's OCR, used to mask the archived image line by line.
pub struct OcrLine {
    pub text: String,
    pub bbox: BBox,
    pub confidence: Option<f64>,
}

pub struct TextBlock {
    /// Reducto block type: `Text`, `List Item`, `Table`, `Header`…
    pub kind: String,
    pub content: String,
    pub bbox: Option<BBox>,
    /// `parse_confidence` from 0 to 1, when given.
    pub confidence: Option<f64>,
    pub low_confidence: bool,
}

pub struct Transcript {
    pub blocks: Vec<TextBlock>,
    pub ocr_lines: Vec<OcrLine>,
    pub pages: u32,
    pub provider: &'static str,
    pub model: String,
}

#[derive(Debug)]
pub enum ReadError {
    /// The provider refused the image itself (too large, unreadable format…).
    Rejected(String),
    /// The provider is down, rate limited or misconfigured.
    Unavailable(String),
}

/// Anything that turns a photo into text blocks. Reducto in production, a fake in tests.
pub trait PrescriptionReader: Send + Sync {
    fn read(&self, image: Image) -> BoxFuture<'_, Result<Transcript, ReadError>>;
}

pub struct Reducto {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl Reducto {
    /// `REDUCTO_API_KEY` is required; `REDUCTO_BASE_URL` and `REDUCTO_MODEL` are optional.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("REDUCTO_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
        let base = std::env::var("REDUCTO_BASE_URL").unwrap_or_else(|_| "https://platform.reducto.ai".into());
        let model = std::env::var("REDUCTO_MODEL").unwrap_or_else(|_| "r-1".into());
        Some(Self::new(base, key, model))
    }

    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("client HTTP");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn send(
        &self,
        step: &'static str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ReadError> {
        let mut attempt = 0;
        loop {
            let retry_in = Duration::from_millis(1500 * 3u64.pow(attempt));
            match build().bearer_auth(&self.api_key).send().await {
                Ok(res) if res.status().is_success() => return Ok(res),
                Ok(res) => {
                    let status = res.status();
                    tracing::warn!(step, %status, attempt, "Reducto a répondu en erreur");
                    let retriable = matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error();
                    if retriable && attempt < 2 {
                        attempt += 1;
                        tokio::time::sleep(retry_in).await;
                        continue;
                    }
                    return Err(match status {
                        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::UNSUPPORTED_MEDIA_TYPE | StatusCode::BAD_REQUEST => {
                            ReadError::Rejected(format!("{step} : {status}"))
                        }
                        _ => ReadError::Unavailable(format!("{step} : {status}")),
                    });
                }
                Err(e) => {
                    let retriable = e.is_timeout() || e.is_connect();
                    let e = e.without_url();
                    tracing::warn!(step, error = %e, attempt, "Reducto injoignable");
                    if retriable && attempt < 2 {
                        attempt += 1;
                        tokio::time::sleep(retry_in).await;
                        continue;
                    }
                    return Err(ReadError::Unavailable(format!("{step} : {e}")));
                }
            }
        }
    }

    async fn transcribe(&self, image: Image) -> Result<Transcript, ReadError> {
        let filename = format!("ordonnance.{}", image.extension);
        let upload: UploadResponse = self
            .send("upload", || {
                let part = multipart::Part::bytes(image.bytes.clone()).file_name(filename.clone());
                self.http.post(self.url("/upload")).multipart(multipart::Form::new().part("file", part))
            })
            .await?
            .json()
            .await
            .map_err(|e| ReadError::Unavailable(format!("upload : réponse illisible ({e})")))?;

        // OCR lines give the position of each written line, needed to mask the archived image.
        let body = json!({ "input": upload.file_id, "settings": { "model": self.model, "return_ocr_data": true } });
        let parsed = self.send("parse", || self.http.post(self.url("/parse")).json(&body)).await;
        let parsed: Result<ParseResponse, ReadError> = match parsed {
            Ok(res) => res
                .json()
                .await
                .map_err(|e| ReadError::Unavailable(format!("parse : réponse illisible ({e})"))),
            Err(e) => Err(e),
        };
        let result = match parsed {
            Ok(p) => {
                let full = match p.result {
                    ParseResult::Full { chunks, ocr } => Ok(FullResult { chunks, ocr }),
                    ParseResult::Url { url } => self.fetch_result(&url).await,
                };
                self.cleanup(&upload.file_id, Some(&p.job_id)).await;
                full.map(|full| (full, p.usage.num_pages))
            }
            Err(e) => {
                self.cleanup(&upload.file_id, None).await;
                Err(e)
            }
        }?;
        let (full, pages) = result;
        let ocr_lines = full
            .ocr
            .map(|o| o.lines)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|l| Some(OcrLine { text: l.text, bbox: l.bbox?, confidence: l.confidence }))
            .collect();
        let blocks = full
            .chunks
            .into_iter()
            .flat_map(|c| c.blocks)
            .map(|b| TextBlock {
                kind: b.kind,
                content: b.content,
                bbox: b.bbox,
                confidence: b.granular_confidence.and_then(|g| g.parse_confidence),
                low_confidence: b.confidence.as_deref() == Some("low"),
            })
            .collect();
        Ok(Transcript { blocks, ocr_lines, pages: pages.max(0.0) as u32, provider: "reducto", model: self.model.clone() })
    }

    /// Large results come back as a link that expires after 12 hours.
    async fn fetch_result(&self, url: &str) -> Result<FullResult, ReadError> {
        let res = self
            .http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| ReadError::Unavailable(format!("résultat : {}", e.without_url())))?;
        match res.json::<UrlResult>().await {
            Ok(UrlResult::Chunks(chunks)) => Ok(FullResult { chunks, ocr: None }),
            Ok(UrlResult::Wrapped(full)) | Ok(UrlResult::Result { result: full }) => Ok(full),
            Err(e) => Err(ReadError::Unavailable(format!("résultat illisible ({e})"))),
        }
    }

    /// Prescriptions are health data: do not leave them on the provider longer than needed.
    async fn cleanup(&self, file_id: &str, job_id: Option<&str>) {
        let upload_id = file_id.trim_start_matches("reducto://");
        let mut paths = vec![format!("/upload/{upload_id}")];
        paths.extend(job_id.map(|j| format!("/job/{j}")));
        for path in paths {
            let res = self.http.delete(self.url(&path)).bearer_auth(&self.api_key).send().await;
            if let Err(e) = res.and_then(|r| r.error_for_status()) {
                tracing::debug!(error = %e.without_url(), "suppression Reducto non confirmée");
            }
        }
    }
}

impl PrescriptionReader for Reducto {
    fn read(&self, image: Image) -> BoxFuture<'_, Result<Transcript, ReadError>> {
        Box::pin(self.transcribe(image))
    }
}

#[derive(Deserialize)]
struct UploadResponse {
    file_id: String,
}

#[derive(Deserialize)]
struct ParseResponse {
    job_id: String,
    usage: Usage,
    result: ParseResult,
}

#[derive(Deserialize)]
struct Usage {
    num_pages: f64,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ParseResult {
    Full {
        chunks: Vec<Chunk>,
        #[serde(default)]
        ocr: Option<Ocr>,
    },
    Url { url: String },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UrlResult {
    Result { result: FullResult },
    Chunks(Vec<Chunk>),
    Wrapped(FullResult),
}

#[derive(Deserialize)]
struct FullResult {
    chunks: Vec<Chunk>,
    #[serde(default)]
    ocr: Option<Ocr>,
}

#[derive(Deserialize)]
struct Ocr {
    #[serde(default)]
    lines: Vec<OcrItem>,
}

#[derive(Deserialize)]
struct OcrItem {
    #[serde(default)]
    text: String,
    bbox: Option<BBox>,
    confidence: Option<f64>,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: String,
    bbox: Option<BBox>,
    confidence: Option<String>,
    granular_confidence: Option<Granular>,
}

#[derive(Deserialize)]
struct Granular {
    parse_confidence: Option<f64>,
}
