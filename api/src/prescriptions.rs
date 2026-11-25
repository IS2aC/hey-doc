//! Prescription reading: a photo (through Reducto) or typed text becomes lines matched to the
//! catalogue. Nothing here decides for the patient: an ambiguous line comes back with its
//! candidates and a quantity is only kept when the prescription states a number of boxes.

use axum::{
    Json,
    extract::{Multipart, State, rejection::JsonRejection},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use uuid::Uuid;

use crate::{
    ApiError, AppState,
    archive::{ClassifiedLine, Entry, LineKind},
    catalog::Presentation,
    reducto::{Image, ReadError, TextBlock},
};

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_LINES: usize = 40;
const MAX_TEXT_CHARS: usize = 4000;
const CANDIDATES: usize = 5;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadStatus {
    /// Des lignes ont été extraites.
    Read,
    /// Trop peu de texte : demander une nouvelle photo ou une saisie.
    Unreadable,
}

#[derive(Serialize, ToSchema, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum LineStatus {
    /// Un candidat se détache.
    Matched,
    /// Plusieurs présentations proches : la personne doit choisir.
    Ambiguous,
    /// Aucune présentation du catalogue ne correspond.
    NoMatch,
}

#[derive(Serialize, ToSchema)]
pub struct Candidate {
    #[serde(flatten)]
    presentation: Presentation,
    /// Score de 0 à 1 environ : proximité du libellé, corrigée par les dosages.
    score: f64,
}

#[derive(Serialize, ToSchema)]
pub struct PrescriptionLine {
    /// Texte de la ligne tel que lu, sans la posologie.
    text: String,
    /// Posologie lue sur la ligne ou les lignes suivantes. Information seulement.
    instructions: Vec<String>,
    /// Nombre de boîtes, seulement s’il est écrit (N°2, x2, 2 boîtes…).
    quantity: Option<i32>,
    /// Nombre d’unités à délivrer, s’il est écrit (« #21 » : 21 comprimés). Sert à proposer
    /// un nombre de boîtes selon le conditionnement choisi.
    units: Option<i32>,
    /// Le lecteur signale une lecture incertaine.
    low_confidence: bool,
    status: LineStatus,
    candidates: Vec<Candidate>,
}

#[derive(Serialize, ToSchema)]
pub struct ReaderInfo {
    pub(crate) provider: &'static str,
    pub(crate) model: String,
    pub(crate) pages: u32,
}

#[derive(Serialize, ToSchema)]
pub struct ReadResponse {
    status: ReadStatus,
    /// Message à afficher quand la photo est inexploitable.
    message: Option<String>,
    lines: Vec<PrescriptionLine>,
    /// Lignes écartées : en-tête, médecin, patient, dates, signatures.
    ignored_lines: usize,
    reader: Option<ReaderInfo>,
    /// Identifiant d’archive, quand la personne a accepté de verser son ordonnance au corpus.
    /// L’archivage se fait après la réponse ; seules les lignes de médicament et de posologie
    /// restent lisibles dans l’image archivée.
    #[schema(value_type = Option<String>, format = Uuid)]
    archive_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TextRequest {
    /// Une ligne par médicament, comme sur l’ordonnance.
    #[schema(example = "Doliprane 1000 mg N°2\n1 cp 3 fois par jour\nAmoxicilline 500 mg x1")]
    text: String,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ImageUpload {
    /// Photo JPEG, PNG ou HEIC de l’ordonnance, 10 Mo au plus.
    #[schema(value_type = String, format = Binary)]
    image: Vec<u8>,
    /// `true` si la personne accepte de verser son ordonnance masquée au corpus d’annotation.
    /// Absent ou `false` par défaut : rien n’est conservé.
    consent: Option<bool>,
    /// `web` (par défaut), `whatsapp` ou `api`.
    channel: Option<String>,
}

/// Lire une ordonnance photographiée.
///
/// La photo est transcrite par Reducto r-1, puis chaque ligne est rapprochée du catalogue.
/// La photo n’est pas conservée par Hey-Doc et est supprimée chez Reducto après lecture.
#[utoipa::path(post, path = "/v1/prescriptions/read", tag = "ordonnances",
    request_body(content = ImageUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, body = ReadResponse),
        (status = 400, description = "Image absente, trop lourde ou d’un format non pris en charge", body = crate::ErrorBody),
        (status = 502, description = "Le service de lecture a échoué", body = crate::ErrorBody),
        (status = 503, description = "Lecture de photo non configurée ou indisponible", body = crate::ErrorBody)))]
pub async fn read(State(state): State<AppState>, multipart: Multipart) -> Result<Json<ReadResponse>, ApiError> {
    let Some(reader) = state.reader.clone() else { return Err(no_reader()) };
    let Upload { image, consent, channel } = upload_from(multipart).await?;
    // HEIC cannot be decoded here to be masked: it is never archived.
    let archive = state.archive.clone().filter(|_| consent && image.extension != "heic");
    let photo = archive.as_ref().map(|_| image.bytes.clone());
    let transcript = reader.read(image).await.map_err(reader_error)?;
    let info = ReaderInfo { provider: transcript.provider, model: transcript.model.clone(), pages: transcript.pages };
    let letters: usize = transcript.blocks.iter().map(|b| b.content.chars().filter(|c| c.is_alphabetic()).count()).sum();
    if letters < 15 {
        return Ok(Json(ReadResponse {
            status: ReadStatus::Unreadable,
            message: Some("Photo illisible. Reprenez-la à plat, bien éclairée et nette, ou saisissez les médicaments.".into()),
            lines: vec![],
            ignored_lines: 0,
            reader: Some(info),
            archive_id: None,
        }));
    }
    let raw = lines_from_blocks(&transcript.blocks);
    let (mut response, lines) = match_lines(&state.pool, raw).await?;
    response.reader = Some(info);
    if let (Some(archive), Some(photo)) = (archive, photo) {
        let id = Uuid::new_v4();
        response.archive_id = Some(id);
        let entry = Entry {
            id,
            channel,
            photo,
            transcript,
            lines,
            result: serde_json::to_value(&response.lines).unwrap_or_default(),
        };
        tokio::spawn(async move {
            if let Err(e) = archive.save(entry).await {
                tracing::error!(%id, error = %e, "archivage de l’ordonnance impossible");
            }
        });
    }
    Ok(Json(response))
}

/// Lire une ordonnance saisie au clavier.
///
/// Même traitement que pour une photo, sans le lecteur : une ligne par médicament.
#[utoipa::path(post, path = "/v1/prescriptions/match", tag = "ordonnances", request_body = TextRequest,
    responses((status = 200, body = ReadResponse), (status = 400, body = crate::ErrorBody)))]
pub async fn match_text(
    State(pool): State<PgPool>,
    body: Result<Json<TextRequest>, JsonRejection>,
) -> Result<Json<ReadResponse>, ApiError> {
    let Json(body) = body?;
    if body.text.trim().is_empty() || body.text.chars().count() > MAX_TEXT_CHARS {
        return Err(ApiError::BadRequest(
            "invalid_text",
            format!("text doit contenir de 1 à {MAX_TEXT_CHARS} caractères"),
        ));
    }
    let raw = body.text.lines().map(|l| RawLine { text: clean_line(l), low_confidence: false, block: None }).collect();
    Ok(Json(match_lines(&pool, raw).await?.0))
}

pub(crate) fn reader_error(e: ReadError) -> ApiError {
    match e {
        ReadError::Rejected(_) => ApiError::BadRequest(
            "image_rejected",
            "La photo n’a pas pu être lue. Reprenez-la ou saisissez les médicaments.".into(),
        ),
        ReadError::Unavailable(detail) => {
            tracing::error!(%detail, "lecture d’ordonnance impossible");
            ApiError::Upstream("reader_failed", "La lecture de photo est momentanément indisponible.".into())
        }
    }
}

pub(crate) fn no_reader() -> ApiError {
    ApiError::Unavailable("reader_unavailable", "La lecture de photo n’est pas configurée. Utilisez la saisie texte.".into())
}

pub(crate) struct Upload {
    pub(crate) image: Image,
    pub(crate) consent: bool,
    pub(crate) channel: String,
}

pub(crate) async fn upload_from(mut multipart: Multipart) -> Result<Upload, ApiError> {
    let invalid = |msg: &str| ApiError::BadRequest("invalid_image", msg.into());
    let (mut image, mut consent, mut channel) = (None, false, "web".to_owned());
    while let Some(field) = multipart.next_field().await.map_err(|_| invalid("Formulaire multipart invalide ou image de plus de 10 Mo"))? {
        match field.name() {
            Some("image") => {
                let bytes = field.bytes().await.map_err(|_| invalid("Image illisible ou de plus de 10 Mo"))?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(invalid("L’image dépasse 10 Mo"));
                }
                let extension = image_kind(&bytes).ok_or_else(|| invalid("Formats acceptés : JPEG, PNG, HEIC"))?;
                image = Some(Image { bytes: bytes.to_vec(), extension });
            }
            Some("consent") => {
                let v = field.text().await.unwrap_or_default();
                consent = matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "oui" | "yes");
            }
            Some("channel") => {
                let v = field.text().await.unwrap_or_default().trim().to_lowercase();
                if !matches!(v.as_str(), "web" | "whatsapp" | "api") {
                    return Err(ApiError::BadRequest("invalid_channel", "channel vaut web, whatsapp ou api".into()));
                }
                channel = v;
            }
            _ => {}
        }
    }
    let image = image.ok_or_else(|| invalid("Champ « image » manquant"))?;
    Ok(Upload { image, consent, channel })
}

/// Detects the format from the file's first bytes, not from its name or declared type.
pub fn image_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.len() > 12 && &b[4..8] == b"ftyp" && matches!(&b[8..12], b"heic" | b"heix" | b"mif1" | b"msf1" | b"hevc") {
        Some("heic")
    } else {
        None
    }
}

// --- From blocks to lines -------------------------------------------------------------

pub struct RawLine {
    pub text: String,
    pub low_confidence: bool,
    /// Index of the reader block the line comes from.
    pub block: Option<usize>,
}

const SKIPPED_BLOCKS: [&str; 4] = ["Header", "Footer", "Figure", "Comment"];

pub fn lines_from_blocks(blocks: &[TextBlock]) -> Vec<RawLine> {
    let mut out = vec![];
    for (i, b) in blocks.iter().enumerate().filter(|(_, b)| !SKIPPED_BLOCKS.contains(&b.kind.as_str())) {
        let low = b.low_confidence || b.confidence.is_some_and(|c| c < 0.6);
        for line in strip_markup(&b.content).lines() {
            let text = clean_line(line);
            if !text.is_empty() {
                out.push(RawLine { text, low_confidence: low, block: Some(i) });
            }
        }
    }
    out
}

/// HTML tables become one line per row; Markdown table pipes and separators go away.
pub(crate) fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let tag: String = chars.by_ref().take_while(|&c| c != '>').collect();
            let name = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
            out.push(if matches!(name.as_str(), "tr" | "br" | "p" | "li" | "div") { '\n' } else { ' ' });
        } else {
            out.push(c);
        }
    }
    out.lines()
        .filter(|l| !l.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) || l.trim().is_empty())
        .map(|l| l.replace('|', " "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Removes list markers ("1.", "2)", "-", "•", "*", "#") and extra spaces.
pub(crate) fn clean_line(line: &str) -> String {
    let mut s = line.trim().trim_start_matches(['-', '*', '•', '#', '>', '·']).trim_start();
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 2 && s[digits..].starts_with(['.', ')', '/']) && !s[digits..].starts_with("./") {
        let rest = &s[digits + 1..];
        // "1/2 cp" is a dose, not a list number.
        if !(s[digits..].starts_with('/') && rest.starts_with(|c: char| c.is_ascii_digit())) {
            s = rest.trim_start();
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'º' => '°',
            c => c,
        })
        .collect()
}

const HEADER_STARTS: &[&str] = &[
    // French
    "dr ", "dr.", "docteur", "pr ", "pr.", "professeur", "medecin", "clinique", "hopital", "chu ", "chr ",
    "centre", "polyclinique", "cabinet", "service", "tel", "cel", "cell", "contact", "date", "le ", "fait a",
    "abidjan, le", "signature", "cachet", "patient", "nom", "prenom", "age", "poids", "ordonnance", "adresse", "bp ",
    // English
    "name", "address", "doctor", "physician", "md", "m.d", "lic", "license", "licence", "ptr", "prc", "s2",
    "sex", "phone", "mobile", "weight", "follow",
];
const HEADER_ANYWHERE: &[&str] = &["e-mail", "email", "@", "ordre des medecins", "mme ", "mlle ", "name of", "lic. no", "license no"];

pub fn is_header(folded: &str) -> bool {
    let letters = folded.chars().filter(|c| c.is_alphabetic()).count();
    // Whole words only: "tel" must not catch "telfast", nor "cel" "celestene".
    let starts_with_word = |h: &str| {
        folded.starts_with(h)
            && (h.ends_with([' ', '.']) || !folded[h.len()..].starts_with(|c: char| c.is_alphabetic()))
    };
    letters < 3
        || HEADER_STARTS.iter().any(|h| starts_with_word(h))
        || HEADER_ANYWHERE.iter().any(|h| folded.contains(h))
        || folded.starts_with("m.") || folded.starts_with("mr ")
}

const DOSE_UNITS: &[&str] = &[
    "cp", "comp", "gel", "cuil", "cac", "cas", "c.a", "sachet", "goutte", "gtte", "amp", "inj", "suppo",
    "bouffee", "application", "pulv", "tab", "cap", "drop", "puff", "spoon", "tsp",
];
const POSOLOGY_WORDS: &[&str] = &[
    " fois", "/j", " par jour", "/jour", " pendant", " matin", " midi", " soir", " au coucher", " si ",
    " en cas", " a prendre", " prendre", " avant ", " toutes les ", " heures",
    " every ", " hours", " days", " as needed", " with food", " daily", " a day", " for fever", " sig.", " sig ",
];
const POSOLOGY_STARTS: &[&str] = &[
    "prendre", "a prendre", "matin", "soir", "midi", "pendant", "qsp", "si ", "en cas", "toutes les", "tous les",
    "sig", "take", "every", "for ", "as needed", "prn", "apply", "give", "use ", "once", "twice", "then",
];

/// Splits "doliprane 1000 mg 1 cp 3 fois par jour" into the product and the dosage instructions.
pub fn split_posology(folded: &str) -> (&str, Option<&str>) {
    let mut cut = POSOLOGY_WORDS.iter().filter_map(|w| folded.find(w)).min();
    // A number followed by a dose unit, e.g. " 1 cp", " 2 cuil". "500 mg" is a strength.
    let bytes: Vec<(usize, char)> = folded.char_indices().collect();
    for (i, &(pos, c)) in bytes.iter().enumerate() {
        if c.is_ascii_digit() && (i == 0 || bytes[i - 1].1 == ' ') {
            let rest = folded[pos..].trim_start_matches(|c: char| c.is_ascii_digit() || c == '/' || c == ',' || c == '.');
            let rest = rest.trim_start();
            if DOSE_UNITS.iter().any(|u| rest.starts_with(u)) {
                cut = Some(cut.map_or(pos, |c| c.min(pos)));
                break;
            }
        }
    }
    match cut {
        Some(0) => ("", Some(folded.trim())),
        Some(i) => (folded[..i].trim(), Some(folded[i..].trim())),
        None => (folded.trim(), None),
    }
}

pub fn starts_like_posology(folded: &str) -> bool {
    matches!(split_posology(folded), ("", Some(_)))
        || POSOLOGY_STARTS.iter().any(|w| {
            folded.starts_with(w) && (w.ends_with(' ') || !folded[w.len()..].starts_with(|c: char| c.is_alphabetic()))
        })
}

const BOX_WORDS: [&str; 12] = ["boite", "boites", "bte", "btes", "bt", "bts", "flacon", "flacons", "fl", "tube", "tubes", "plaquettes"];

/// "#21" is the number of units to dispense (21 tablets), not of boxes.
pub fn take_units(product: &str) -> (String, Option<i32>) {
    let mut units = None;
    let text = product
        .split_whitespace()
        .filter(|t| match t.strip_prefix('#').and_then(|n| n.parse::<i32>().ok()) {
            Some(n) if units.is_none() && (1..=1000).contains(&n) => {
                units = Some(n);
                false
            }
            _ => true,
        })
        .collect::<Vec<_>>()
        .join(" ");
    (text, units)
}

/// Takes the number of boxes out of the product text: "n°2", "n° 2", "x2", "x 2", "2 boites", "qte 2".
pub fn take_quantity(product: &str) -> (String, Option<i32>) {
    let tokens: Vec<&str> = product.split_whitespace().collect();
    let num = |t: &str| t.trim_matches(|c: char| matches!(c, '(' | ')' | ':' | '.')).parse::<i32>().ok().filter(|n| (1..=100).contains(n));
    let mut keep = vec![true; tokens.len()];
    let mut qty = None;
    for i in 0..tokens.len() {
        let t = tokens[i];
        let next = tokens.get(i + 1).copied();
        let found = if let Some(rest) = t.strip_prefix("n°").or_else(|| t.strip_prefix("x")).filter(|r| !r.is_empty()) {
            num(rest).map(|n| (n, 1))
        } else if matches!(t, "n°" | "x" | "qte" | "qte:" | "quantite" | "quantite:" | "no") {
            next.and_then(num).map(|n| (n, 2))
        } else if t.starts_with('(') && t.ends_with(')') {
            num(t).map(|n| (n, 1))
        } else if next.is_some_and(|n| BOX_WORDS.contains(&n.trim_end_matches(['.', ',']))) {
            num(t).map(|n| (n, 2))
        } else {
            None
        };
        if let (Some((n, width)), None) = (found, qty) {
            qty = Some(n);
            keep[i..(i + width).min(tokens.len())].iter_mut().for_each(|k| *k = false);
        }
    }
    let text = tokens.iter().zip(&keep).filter(|(_, k)| **k).map(|(t, _)| *t).collect::<Vec<_>>().join(" ");
    (text, qty)
}

const FORM_WORDS: &[&str] = &[
    "cp", "comp", "comprime", "comprimes", "gel", "gelule", "gelules", "sirop", "sol", "solution", "inj",
    "injectable", "susp", "suspension", "tab", "tablet", "cap", "caps", "capsule",
];

/// The form written on the prescription, as stored in `dosage_form`.
fn written_form(q: &str) -> Option<&'static str> {
    q.split(|c: char| !c.is_alphanumeric()).find_map(|w| match w {
        "cp" | "comp" | "cpr" | "comprime" | "comprimes" | "tab" | "tabs" | "tablet" | "tablets" => Some("comprim"),
        "gel" | "gelule" | "gelules" | "cap" | "caps" | "capsule" | "capsules" => Some("gélule"),
        "sirop" | "syrup" | "susp" | "suspension" => Some("susp"),
        "inj" | "injectable" | "amp" | "ampoule" => Some("inject"),
        "suppo" | "suppositoire" | "suppository" => Some("suppo"),
        _ => None,
    })
}

fn first_name_word(q: &str) -> Option<&str> {
    q.split_whitespace().find(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3 && !FORM_WORDS.contains(w))
}

/// Strengths in milligrams ("1g" and "1000 mg" both give 1000). Pack sizes such as "B/20"
/// are not strengths. With `bare`, a number without a unit counts too ("spasfon 80").
pub(crate) fn strengths(s: &str, bare: bool) -> Vec<u64> {
    let s = s.to_lowercase().replace(',', ".");
    let chars: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        // After "/", a number is a second strength only if a number or a mass unit precedes the
        // slash ("500/125", "500MG/125MG"); "B/20" and "FL/100ML" are packs.
        let after_strength_slash = i >= 2
            && chars[i - 1] == '/'
            && (chars[i - 2].is_ascii_digit() || (chars[i - 2] == 'g' && i >= 3 && (chars[i - 3] == 'm' || chars[i - 3].is_ascii_digit())));
        if !chars[i].is_ascii_digit()
            || (i > 0 && !after_strength_slash && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '/' || chars[i - 1] == '.'))
        {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
            i += 1;
        }
        let value: f64 = chars[start..i].iter().collect::<String>().trim_end_matches('.').parse().unwrap_or(0.0);
        let mut unit: String = chars[i..].iter().skip_while(|c| **c == ' ').take_while(|c| c.is_alphabetic() || **c == 'µ' || **c == '%').collect();
        // "500/125 mg": the unit after the second number applies to the first.
        if unit.is_empty() && chars.get(i) == Some(&'/') {
            let next: String = chars[i + 1..].iter().skip_while(|c| c.is_ascii_digit() || **c == '.' || **c == ' ').take_while(|c| c.is_alphabetic()).collect();
            if matches!(next.as_str(), "mg" | "g") {
                unit = next;
            }
        }
        let mg = match unit.as_str() {
            "g" | "gr" => Some(value * 1000.0),
            "mg" => Some(value),
            "mcg" | "µg" | "ug" => Some(value / 1000.0),
            "" if bare => Some(value),
            _ => None,
        };
        out.extend(mg.filter(|v| *v > 0.0).map(|v| (v * 1000.0).round() as u64));
    }
    out
}
// --- Matching --------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct Scored {
    #[sqlx(flatten)]
    presentation: Presentation,
    score: f64,
}

/// `names`: the words to look up, usually the first name on the line. A drug written over two
/// lines ("Amoxicillin + Clavulanic acid" then "(Co-Amoxiclav) 500/125") is looked up by both.
async fn candidates(pool: &PgPool, query: &str, names: &[&str]) -> Result<Vec<Candidate>, ApiError> {
    let mut all: Vec<Candidate> = vec![];
    for name in names {
        for c in candidates_for(pool, query, name).await? {
            match all.iter_mut().find(|a| a.presentation.id == c.presentation.id) {
                Some(a) => a.score = a.score.max(c.score),
                None => all.push(c),
            }
        }
    }
    all.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.presentation.id.cmp(&b.presentation.id)));
    all.truncate(CANDIDATES);
    Ok(all)
}

async fn candidates_for(pool: &PgPool, query: &str, name: &str) -> Result<Vec<Candidate>, ApiError> {
    let rows = sqlx::query_as::<_, Scored>(
        // The written name weighs more when it is in the label itself: "amoxicilline" should
        // rank AMOXICILLINE … before brands that only contain the molecule.
        "SELECT id, label, ingredient, strength, dosage_form, pack,
                (0.6 * word_similarity($1, search_text) + 0.4 * word_similarity($2, lower(label)))::float8 AS score
         FROM presentations
         WHERE strpos(search_text, $2) > 0 OR $2 <% search_text
         ORDER BY score DESC, label, id
         LIMIT 40",
    )
    .bind(query)
    .bind(name)
    .fetch_all(pool)
    .await?;
    let wanted = strengths(query, true);
    let form = written_form(query);
    let scored: Vec<Candidate> = rows
        .into_iter()
        .map(|r| {
            let have = strengths(&format!("{} {}", r.presentation.label, r.presentation.strength.as_deref().unwrap_or("")), false);
            let bonus = if wanted.is_empty() {
                0.0
            } else if wanted.iter().all(|n| have.contains(n)) {
                0.15
            } else if wanted.iter().any(|n| have.contains(n)) {
                0.0
            } else {
                -0.2
            };
            let stored_form = fold(r.presentation.dosage_form.as_deref().unwrap_or(""));
            let form_bonus = match form {
                Some(f) if stored_form.contains(&fold(f)) => 0.05,
                Some(_) if !stored_form.is_empty() => -0.2,
                _ => 0.0,
            };
            let score = (r.score + bonus + form_bonus).clamp(0.0, 1.0);
            Candidate { presentation: r.presentation, score: (score * 100.0).round() / 100.0 }
        })
        .collect();
    Ok(scored)
}

pub fn status_of(candidates: &[Candidate]) -> LineStatus {
    match candidates {
        [] => LineStatus::NoMatch,
        [first, ..] if first.score < 0.45 => LineStatus::NoMatch,
        [first, second, ..] if second.score >= first.score - 0.05 => LineStatus::Ambiguous,
        _ => LineStatus::Matched,
    }
}

/// A line read before the catalogue lookup.
struct Draft {
    text: String,
    query: String,
    names: Vec<String>,
    instructions: Vec<String>,
    quantity: Option<i32>,
    units: Option<i32>,
    low_confidence: bool,
    /// Indexes of the raw lines that make the product, then of its instruction lines.
    product_lines: Vec<usize>,
    posology_lines: Vec<usize>,
}

/// "(Co-Amoxiclav) 500/125 mg" or "500 mg/tab" right after a product without strength and
/// before any instruction completes that product.
fn continues(prev: &Draft, folded: &str) -> bool {
    let bare_strength = folded.starts_with(|c: char| c.is_ascii_digit()) && !strengths(folded, false).is_empty();
    prev.instructions.is_empty() && strengths(&prev.query, true).is_empty() && (folded.starts_with('(') || bare_strength)
}

/// Also says what each raw line turned out to be, for the archive's masking.
async fn match_lines(pool: &PgPool, raw: Vec<RawLine>) -> Result<(ReadResponse, Vec<ClassifiedLine>), ApiError> {
    let mut classified: Vec<ClassifiedLine> =
        raw.iter().map(|r| ClassifiedLine { text: r.text.clone(), kind: LineKind::Other, block: r.block }).collect();
    let mut drafts: Vec<Draft> = vec![];
    let mut ignored = 0;
    for (index, raw) in raw.into_iter().enumerate() {
        let folded = fold(&raw.text);
        if is_header(&folded) {
            ignored += 1;
            continue;
        }
        if starts_like_posology(&folded) {
            match drafts.last_mut() {
                Some(prev) => {
                    prev.instructions.push(raw.text);
                    prev.posology_lines.push(index);
                }
                None => ignored += 1,
            }
            continue;
        }
        let (product, posology) = split_posology(&folded);
        let (query, units) = take_units(product);
        let (query, quantity) = take_quantity(&query);
        let instructions: Vec<String> = posology.map(|p| original_suffix(&raw.text, &folded, p).to_owned()).into_iter().collect();
        let text = original_prefix(&raw.text, product.len()).to_owned();
        let name = first_name_word(query.trim_start_matches('(')).map(|n| n.trim_matches(|c: char| !c.is_alphanumeric()).to_owned());
        if let Some(prev) = drafts.last_mut().filter(|p| continues(p, &folded)) {
            prev.text = format!("{} {text}", prev.text);
            prev.query = format!("{} {query}", prev.query);
            prev.names.extend(name);
            prev.instructions.extend(instructions);
            prev.quantity = prev.quantity.or(quantity);
            prev.units = prev.units.or(units);
            prev.low_confidence |= raw.low_confidence;
            prev.product_lines.push(index);
            continue;
        }
        if drafts.len() >= MAX_LINES {
            ignored += 1;
            continue;
        }
        drafts.push(Draft {
            text,
            query,
            names: name.into_iter().collect(),
            instructions,
            quantity,
            units,
            low_confidence: raw.low_confidence,
            product_lines: vec![index],
            posology_lines: vec![],
        });
    }

    let mut lines = vec![];
    for d in drafts {
        let names: Vec<&str> = d.names.iter().map(String::as_str).collect();
        let candidates = candidates(pool, &d.query, &names).await?;
        let status = status_of(&candidates);
        // Without a candidate or a strength, the line is most likely a name, a stamp or a note.
        if status == LineStatus::NoMatch && strengths(&d.query, false).is_empty() {
            ignored += 1;
            continue;
        }
        d.product_lines.iter().for_each(|&i| classified[i].kind = LineKind::Product);
        d.posology_lines.iter().for_each(|&i| classified[i].kind = LineKind::Posology);
        lines.push(PrescriptionLine {
            text: d.text,
            instructions: d.instructions,
            quantity: d.quantity,
            units: d.units,
            low_confidence: d.low_confidence,
            status,
            candidates: if status == LineStatus::NoMatch { vec![] } else { candidates },
        });
    }
    let response = ReadResponse { status: ReadStatus::Read, message: None, lines, ignored_lines: ignored, reader: None, archive_id: None };
    Ok((response, classified))
}

/// `fold` maps one char to one char, so char positions line up between the two strings.
fn original_prefix(original: &str, folded_bytes: usize) -> &str {
    let folded = fold(original);
    let chars = folded[..folded_bytes.min(folded.len())].chars().count();
    let end = original.char_indices().nth(chars).map_or(original.len(), |(i, _)| i);
    original[..end].trim()
}

fn original_suffix<'a>(original: &'a str, folded: &str, part: &str) -> &'a str {
    let start = folded.rfind(part).unwrap_or(0);
    let chars = folded[..start].chars().count();
    let begin = original.char_indices().nth(chars).map_or(original.len(), |(i, _)| i);
    original[begin..].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posology_is_split_from_the_product() {
        assert_eq!(split_posology("doliprane 1000 mg 1 cp 3 fois par jour"), ("doliprane 1000 mg", Some("1 cp 3 fois par jour")));
        assert_eq!(split_posology("amoxicilline 500 mg"), ("amoxicilline 500 mg", None));
        assert_eq!(split_posology("1/2 cp le soir"), ("", Some("1/2 cp le soir")));
        assert!(starts_like_posology("matin et soir pendant 7 jours"));
        assert!(!starts_like_posology("augmentin 1g sachet"));
    }

    #[test]
    fn quantity_is_only_taken_when_written_as_boxes() {
        assert_eq!(take_quantity("doliprane 1000 mg n°2"), ("doliprane 1000 mg".into(), Some(2)));
        assert_eq!(take_quantity("doliprane 1000 mg n° 3"), ("doliprane 1000 mg".into(), Some(3)));
        assert_eq!(take_quantity("amoxicilline 500 x2"), ("amoxicilline 500".into(), Some(2)));
        assert_eq!(take_quantity("efferalgan 2 boites"), ("efferalgan".into(), Some(2)));
        assert_eq!(take_quantity("augmentin 1g (1)"), ("augmentin 1g".into(), Some(1)));
        assert_eq!(take_quantity("metformine 850 mg"), ("metformine 850 mg".into(), None));
    }

    #[test]
    fn headers_and_list_markers_are_removed() {
        assert!(is_header(&fold("Dr KOUASSI Jean, médecin généraliste")));
        assert!(is_header(&fold("Tél : 07 07 07 07 07")));
        assert!(is_header(&fold("Abidjan, le 12/11/2026")));
        assert!(is_header("12/11/2026"));
        assert!(!is_header(&fold("Paracétamol 1000 mg")));
        assert!(!is_header(&fold("Telfast 180 mg")));
        assert!(!is_header(&fold("Célestène 0,05 %")));
        assert_eq!(clean_line("1. Doliprane 1000"), "Doliprane 1000");
        assert_eq!(clean_line("- Amoxicilline  500"), "Amoxicilline 500");
        assert_eq!(clean_line("1/2 cp le soir"), "1/2 cp le soir");
    }

    #[test]
    fn tables_become_lines() {
        let html = "<table><tr><td>Doliprane 1000</td><td>N°2</td></tr><tr><td>Spasfon</td><td>x1</td></tr></table>";
        let lines: Vec<String> = strip_markup(html).lines().map(clean_line).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["Doliprane 1000 N°2", "Spasfon x1"]);
        let md = "| Produit | Qté |\n|---|---|\n| Doliprane | 2 |";
        let lines: Vec<String> = strip_markup(md).lines().map(clean_line).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["Produit Qté", "Doliprane 2"]);
    }

    #[test]
    fn strengths_are_compared_in_milligrams() {
        assert_eq!(strengths("augmentin 1g", true), [1_000_000]);
        assert_eq!(strengths("DOLIPRANE CP SEC 1000MG B/8", false), [1_000_000]);
        assert_eq!(strengths("AUGMENTIN 500 PDRE P P INJ 500 MG B/1", false), [500_000]);
        assert_eq!(strengths("AMOXICILLINE 250MG/5ML FL/100ML", false), [250_000]);
        assert_eq!(strengths("spasfon lyoc 80", true), [80_000]);
        assert_eq!(strengths("celestene 0,5 mg", true), [500]);
    }

    #[test]
    fn english_prescriptions_are_understood() {
        assert!(is_header(&fold("Name of Patient")));
        assert!(is_header(&fold("Address")));
        assert!(is_header(&fold("Lic. No. 12")));
        assert!(starts_like_posology(&fold("Sig. Take one with food")));
        assert!(starts_like_posology(&fold("every 8 hours for 7 days")));
        assert!(starts_like_posology(&fold("for fever (temp. ≥ 37.8°C)")));
        assert!(!starts_like_posology(&fold("Fortum 1g")));
        assert_eq!(take_units("paracetamol 500 mg/tab #5"), ("paracetamol 500 mg/tab".into(), Some(5)));
        assert_eq!(strengths("(co-amoxiclav) 500/125 mg/tab", true), [500_000, 125_000]);
        assert_eq!(strengths("CO-AMOXICLAV CP 500MG/125MG B/16", false), [500_000, 125_000]);
        assert_eq!(written_form("paracetamol 500 mg/tab"), Some("comprim"));
    }

    #[test]
    fn image_kind_reads_magic_bytes() {
        assert_eq!(image_kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_kind(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(image_kind(b"\0\0\0\x18ftypheic\0\0\0\0"), Some("heic"));
        assert_eq!(image_kind(b"%PDF-1.7"), None);
    }

    #[test]
    fn original_text_keeps_its_accents() {
        let raw = "Paracétamol 1000 mg 1 cp 3 fois/j";
        let folded = fold(raw);
        let (product, posology) = split_posology(&folded);
        assert_eq!(original_prefix(raw, product.len()), "Paracétamol 1000 mg");
        assert_eq!(original_suffix(raw, &folded, posology.unwrap()), "1 cp 3 fois/j");
    }
}
