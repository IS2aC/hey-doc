//! Consented archive of read prescriptions, to build an annotated corpus for a local
//! handwriting model.
//!
//! Nothing identifying is stored in clear. The archived image starts black and only the lines
//! recognised as a product or a dosage instruction are copied from the photo: names, dates,
//! stamps and signatures cannot leak even when the reader missed them. The annotation keeps the
//! position of every OCR line, with text only for those kept lines. The original photo and the
//! raw Reducto response stay in memory and are dropped after the request.

use std::{io::Cursor, sync::Arc, time::Duration};

use chrono::{DateTime, Datelike, Utc};
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage, codecs::jpeg::JpegEncoder};
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, aws::AmazonS3Builder, path::Path};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    prescriptions::fold,
    reducto::{BBox, Transcript},
};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Product,
    Posology,
    /// Header, patient, doctor, date, stamp, signature, or anything not recognised.
    Other,
}

/// A line as split by `prescriptions`, with what it was recognised as.
pub struct ClassifiedLine {
    pub text: String,
    pub kind: LineKind,
    pub block: Option<usize>,
}

#[derive(Serialize, Debug)]
pub struct Region {
    pub bbox: BBox,
    /// `product`, `posology` or `other` (masked in the image, no text).
    pub kind: LineKind,
    pub text: Option<String>,
    pub confidence: Option<f64>,
    /// `ocr_line` when Reducto located the line, `block` otherwise.
    pub source: &'static str,
}

fn key(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Which parts of the photo may stay visible, and the annotation of each located line.
pub fn regions(transcript: &Transcript, lines: &[ClassifiedLine]) -> Vec<Region> {
    let kept: Vec<(String, LineKind)> =
        lines.iter().filter(|l| l.kind != LineKind::Other).map(|l| (key(&l.text), l.kind)).collect();
    if !transcript.ocr_lines.is_empty() {
        return transcript
            .ocr_lines
            .iter()
            .map(|o| {
                let k = key(&o.text);
                let kind = kept
                    .iter()
                    .find(|(t, _)| k.chars().count() >= 3 && (t.contains(&k) || k.contains(t.as_str())))
                    .map_or(LineKind::Other, |(_, kind)| *kind);
                Region {
                    bbox: o.bbox,
                    kind,
                    text: (kind != LineKind::Other).then(|| o.text.clone()),
                    confidence: o.confidence,
                    source: "ocr_line",
                }
            })
            .collect();
    }
    // Without OCR lines, a block stays visible only if every line in it was kept.
    transcript
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let bbox = b.bbox?;
            let own: Vec<&ClassifiedLine> = lines.iter().filter(|l| l.block == Some(i)).collect();
            let all_kept = !own.is_empty() && own.iter().all(|l| l.kind != LineKind::Other);
            let kind = if !all_kept {
                LineKind::Other
            } else if own.iter().any(|l| l.kind == LineKind::Product) {
                LineKind::Product
            } else {
                LineKind::Posology
            };
            let text = all_kept.then(|| own.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"));
            Some(Region { bbox, kind, text, confidence: b.confidence, source: "block" })
        })
        .collect()
}

/// Copies only the `keep` boxes of the photo onto a black image, after applying the EXIF
/// orientation, and re-encodes it as JPEG, which also drops every metadata field (GPS…).
pub fn redact(photo: &[u8], keep: &[BBox]) -> Result<(Vec<u8>, u32, u32), image::ImageError> {
    let mut decoder = ImageReader::new(Cursor::new(photo)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let src = img.to_rgb8();
    let (w, h) = src.dimensions();
    let mut out = RgbImage::new(w, h);
    const MARGIN: f64 = 0.004;
    for b in keep {
        let x0 = ((b.left - MARGIN).max(0.0) * w as f64).floor() as u32;
        let y0 = ((b.top - MARGIN).max(0.0) * h as f64).floor() as u32;
        let x1 = (((b.left + b.width + MARGIN).min(1.0)) * w as f64).ceil().min(w as f64) as u32;
        let y1 = (((b.top + b.height + MARGIN).min(1.0)) * h as f64).ceil().min(h as f64) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                out.put_pixel(x, y, *src.get_pixel(x, y));
            }
        }
    }
    let mut jpeg = vec![];
    JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&out)?;
    Ok((jpeg, w, h))
}

pub struct Archive {
    store: Arc<dyn ObjectStore>,
    pool: PgPool,
    retention_days: i64,
}

pub struct Entry {
    pub id: Uuid,
    pub channel: String,
    pub photo: Vec<u8>,
    pub transcript: Transcript,
    pub lines: Vec<ClassifiedLine>,
    /// The lines and candidates returned to the person, without masked text.
    pub result: Value,
}

impl Archive {
    /// `S3_ENDPOINT`, `S3_BUCKET`, `S3_ACCESS_KEY` and `S3_SECRET_KEY` switch the archive on.
    pub fn from_env(pool: PgPool) -> Option<Result<Self, Error>> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let endpoint = var("S3_ENDPOINT")?;
        let built = AmazonS3Builder::new()
            .with_endpoint(&endpoint)
            .with_allow_http(endpoint.starts_with("http://"))
            .with_bucket_name(var("S3_BUCKET").unwrap_or_else(|| "prescriptions".into()))
            .with_access_key_id(var("S3_ACCESS_KEY").unwrap_or_default())
            .with_secret_access_key(var("S3_SECRET_KEY").unwrap_or_default())
            .with_region(var("S3_REGION").unwrap_or_else(|| "us-east-1".into()))
            .build();
        let days = var("HEYDOC_ARCHIVE_DAYS").and_then(|d| d.parse().ok()).unwrap_or(365);
        Some(built.map(|s| Self::new(Arc::new(s), pool, days)).map_err(Into::into))
    }

    pub fn new(store: Arc<dyn ObjectStore>, pool: PgPool, retention_days: i64) -> Self {
        Self { store, pool, retention_days }
    }

    pub async fn save(&self, entry: Entry) -> Result<(), Error> {
        let regions = regions(&entry.transcript, &entry.lines);
        let keep: Vec<BBox> = regions.iter().filter(|r| r.kind != LineKind::Other).map(|r| r.bbox).collect();
        let photo = entry.photo;
        let (jpeg, width, height) = tokio::task::spawn_blocking(move || redact(&photo, &keep)).await??;
        let now = Utc::now();
        let prefix = format!("reads/{:04}/{:02}/{}", now.year(), now.month(), entry.id);
        let (image_key, annotation_key) = (format!("{prefix}/image.jpg"), format!("{prefix}/annotation.json"));
        let sha: String = Sha256::digest(&jpeg).iter().map(|b| format!("{b:02x}")).collect();
        let search_text = regions.iter().filter_map(|r| r.text.as_deref()).collect::<Vec<_>>().join("\n");
        let annotation = json!({
            "schema": "heydoc.prescription-annotation/1",
            "id": entry.id,
            "created_at": now,
            "channel": entry.channel,
            "reader": { "provider": entry.transcript.provider, "model": entry.transcript.model },
            "image": { "key": image_key, "width": width, "height": height, "sha256": sha },
            "review": { "status": "to_review", "note": "Pré-annotation automatique, à relire avant entraînement." },
            "regions": regions,
            "result": entry.result,
        });

        self.store.put(&Path::from(image_key.as_str()), PutPayload::from(jpeg)).await?;
        self.store.put(&Path::from(annotation_key.as_str()), PutPayload::from(serde_json::to_vec_pretty(&annotation)?)).await?;
        let kept = regions.iter().filter(|r| r.kind != LineKind::Other).count() as i32;
        let masked = regions.len() as i32 - kept;
        let saved = sqlx::query(
            "INSERT INTO prescription_reads
               (id, created_at, expires_at, channel, image_key, annotation_key, image_sha256, width, height,
                provider, model, kept_regions, masked_regions, search_text)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(entry.id)
        .bind(now)
        .bind(now + chrono::Duration::days(self.retention_days))
        .bind(&entry.channel)
        .bind(&image_key)
        .bind(&annotation_key)
        .bind(&sha)
        .bind(width as i32)
        .bind(height as i32)
        .bind(entry.transcript.provider)
        .bind(&entry.transcript.model)
        .bind(kept)
        .bind(masked)
        .bind(&search_text)
        .execute(&self.pool)
        .await;
        if let Err(e) = saved {
            // Do not leave orphan objects behind.
            self.delete_objects(&image_key, &annotation_key).await;
            return Err(e.into());
        }
        Ok(())
    }

    async fn delete_objects(&self, image_key: &str, annotation_key: &str) {
        for k in [image_key, annotation_key] {
            if let Err(e) = self.store.delete(&Path::from(k)).await {
                tracing::warn!(error = %e, "objet d’archive non supprimé");
            }
        }
    }

    /// Deletes the reads past their retention date, objects first.
    pub async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, Error> {
        let expired: Vec<(Uuid, String, String)> =
            sqlx::query_as("SELECT id, image_key, annotation_key FROM prescription_reads WHERE expires_at <= $1")
                .bind(now)
                .fetch_all(&self.pool)
                .await?;
        for (id, image_key, annotation_key) in &expired {
            self.delete_objects(image_key, annotation_key).await;
            sqlx::query("DELETE FROM prescription_reads WHERE id = $1").bind(id).execute(&self.pool).await?;
        }
        Ok(expired.len() as u64)
    }

    /// Runs the purge at start-up, then every 6 hours.
    pub fn spawn_purge(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                match self.purge_expired(Utc::now()).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(n, "ordonnances archivées expirées supprimées"),
                    Err(e) => tracing::error!(error = %e, "purge des archives impossible"),
                }
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reducto::{OcrLine, TextBlock};
    use image::{ImageEncoder, Rgb, codecs::png::PngEncoder};

    fn transcript(ocr: Vec<(&str, BBox)>, blocks: Vec<(&str, BBox)>) -> Transcript {
        Transcript {
            blocks: blocks
                .into_iter()
                .map(|(c, b)| TextBlock { kind: "Text".into(), content: c.into(), bbox: Some(b), confidence: None, low_confidence: false })
                .collect(),
            ocr_lines: ocr.into_iter().map(|(t, b)| OcrLine { text: t.into(), bbox: b, confidence: Some(0.9) }).collect(),
            pages: 1,
            provider: "fake",
            model: "test".into(),
        }
    }

    fn bb(top: f64) -> BBox {
        BBox { left: 0.1, top, width: 0.8, height: 0.1 }
    }

    fn line(text: &str, kind: LineKind, block: usize) -> ClassifiedLine {
        ClassifiedLine { text: text.into(), kind, block: Some(block) }
    }

    #[test]
    fn only_product_and_posology_lines_keep_their_text() {
        let t = transcript(
            vec![("Dr KOUASSI Jean", bb(0.0)), ("Doliprane 1000 mg N°2", bb(0.3)), ("1 cp 3 fois par jour", bb(0.45)), ("KOUASSI", bb(0.8))],
            vec![],
        );
        let lines = [
            line("Dr KOUASSI Jean", LineKind::Other, 0),
            line("Doliprane 1000 mg N°2", LineKind::Product, 0),
            line("1 cp 3 fois par jour", LineKind::Posology, 0),
        ];
        let r = regions(&t, &lines);
        let kinds: Vec<LineKind> = r.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [LineKind::Other, LineKind::Product, LineKind::Posology, LineKind::Other]);
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("KOUASSI"), "{json}");
    }

    #[test]
    fn without_ocr_lines_a_mixed_block_is_masked() {
        let t = transcript(vec![], vec![("Dr X\nDoliprane", bb(0.0)), ("Spasfon 80", bb(0.5))]);
        let lines = [line("Dr X", LineKind::Other, 0), line("Doliprane", LineKind::Product, 0), line("Spasfon 80", LineKind::Product, 1)];
        let r = regions(&t, &lines);
        assert_eq!(r[0].kind, LineKind::Other);
        assert!(r[0].text.is_none());
        assert_eq!(r[1].kind, LineKind::Product);
        assert_eq!(r[1].text.as_deref(), Some("Spasfon 80"));
    }

    #[test]
    fn redacted_image_is_black_outside_kept_boxes() {
        let mut white = RgbImage::from_pixel(100, 100, Rgb([255, 255, 255]));
        white.put_pixel(50, 50, Rgb([255, 255, 255]));
        let mut png = vec![];
        PngEncoder::new(&mut png).write_image(white.as_raw(), 100, 100, image::ExtendedColorType::Rgb8).unwrap();
        let (jpeg, w, h) = redact(&png, &[BBox { left: 0.0, top: 0.5, width: 1.0, height: 0.2 }]).unwrap();
        assert_eq!((w, h), (100, 100));
        let out = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert!(out.get_pixel(50, 10).0.iter().all(|&c| c < 20), "header stays black");
        assert!(out.get_pixel(50, 60).0.iter().all(|&c| c > 235), "kept line is copied");
        assert!(out.get_pixel(50, 90).0.iter().all(|&c| c < 20), "signature area stays black");
    }
}
