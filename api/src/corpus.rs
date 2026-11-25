//! Test routes to explore prescriptions as text.
//!
//! - `reducto-text`: what Reducto read on a photo, as one text, nothing matched or stored.
//! - `similar-products`: for each line of a text, the catalogue labels closest by cosine
//!   similarity on character trigrams. No model and no index: simple and explainable, to judge
//!   what the catalogue covers.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Multipart, State, rejection::JsonRejection},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    prescriptions::{ReaderInfo, clean_line, fold, no_reader, reader_error, strip_markup, upload_from},
};

const MAX_TEXT_CHARS: usize = 8000;
const MAX_LINES: usize = 80;

#[derive(Serialize, ToSchema)]
pub struct BlockText {
    /// Type de bloc Reducto : `Text`, `List Item`, `Table`, `Header`…
    kind: String,
    content: String,
    /// Confiance de lecture de 0 à 1, si Reducto la donne.
    confidence: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct ReductoText {
    reader: ReaderInfo,
    /// Tout le texte lu, bloc après bloc, une ligne par ligne écrite.
    #[schema(example = "Name of Patient\nAmoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab #21")]
    text: String,
    /// Les mêmes lignes, nettoyées des puces et numéros.
    lines: Vec<String>,
    blocks: Vec<BlockText>,
}

/// Test · texte lu par Reducto.
///
/// Envoie la photo à Reducto et renvoie le texte lu, sans rapprochement ni archivage.
/// Chaque appel est facturé par Reducto.
#[utoipa::path(post, path = "/v1/test/reducto-text", tag = "test",
    request_body(content = crate::prescriptions::ImageUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, body = ReductoText),
        (status = 400, body = crate::ErrorBody),
        (status = 502, body = crate::ErrorBody),
        (status = 503, body = crate::ErrorBody)))]
pub async fn reducto_text(State(state): State<AppState>, multipart: Multipart) -> Result<Json<ReductoText>, ApiError> {
    let Some(reader) = state.reader.clone() else { return Err(no_reader()) };
    let upload = upload_from(multipart).await?;
    let t = reader.read(upload.image).await.map_err(reader_error)?;
    let text = t.blocks.iter().map(|b| strip_markup(&b.content).trim().to_owned()).filter(|c| !c.is_empty()).collect::<Vec<_>>().join("\n");
    let lines = text.lines().map(clean_line).filter(|l| !l.is_empty()).collect();
    let blocks = t
        .blocks
        .into_iter()
        .map(|b| BlockText { kind: b.kind, content: b.content, confidence: b.confidence })
        .collect();
    Ok(Json(ReductoText { reader: ReaderInfo { provider: t.provider, model: t.model, pages: t.pages }, text, lines, blocks }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SimilarRequest {
    /// Le corpus : une ordonnance en texte, une ligne par ligne écrite.
    #[schema(example = "Amoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab #21\nParacetamol 500 mg/tab #5")]
    text: String,
    /// Score minimal, de 0 à 1. 0,35 par défaut.
    min_score: Option<f64>,
    /// Nombre de produits par ligne, de 1 à 20. 5 par défaut.
    per_line: Option<usize>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Similar {
    id: i64,
    label: String,
    ingredient: Option<String>,
    /// Similarité cosinus entre la ligne et le libellé, de 0 à 1.
    score: f64,
}

#[derive(Serialize, ToSchema)]
pub struct LineMatches {
    index: usize,
    text: String,
    matches: Vec<Similar>,
}

#[derive(Serialize, ToSchema)]
pub struct CitedProduct {
    #[serde(flatten)]
    product: Similar,
    /// Lignes du texte où le produit apparaît.
    lines: Vec<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct SimilarResponse {
    /// Nombre de libellés comparés.
    catalogue_size: usize,
    method: &'static str,
    lines: Vec<LineMatches>,
    /// Produits cités ou proches, sans doublon, meilleur score d’abord.
    products: Vec<CitedProduct>,
}

/// Test · produits proches d’une ordonnance.
///
/// Compare chaque ligne du texte à tous les libellés du catalogue par similarité cosinus sur
/// les trigrammes de caractères (sans accents ni casse). Renvoie les produits cités ou proches.
#[utoipa::path(post, path = "/v1/test/similar-products", tag = "test", request_body = SimilarRequest,
    responses((status = 200, body = SimilarResponse), (status = 400, body = crate::ErrorBody)))]
pub async fn similar_products(
    State(pool): State<PgPool>,
    body: Result<Json<SimilarRequest>, JsonRejection>,
) -> Result<Json<SimilarResponse>, ApiError> {
    let Json(body) = body?;
    if body.text.trim().is_empty() || body.text.chars().count() > MAX_TEXT_CHARS {
        return Err(ApiError::BadRequest("invalid_text", format!("text doit contenir de 1 à {MAX_TEXT_CHARS} caractères")));
    }
    let min_score = body.min_score.unwrap_or(0.35);
    if !(0.0..=1.0).contains(&min_score) {
        return Err(ApiError::BadRequest("invalid_min_score", "min_score va de 0 à 1".into()));
    }
    let per_line = body.per_line.unwrap_or(5).clamp(1, 20);
    let lines: Vec<String> = body
        .text
        .lines()
        .map(clean_line)
        .filter(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .take(MAX_LINES)
        .collect();

    let catalogue: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, label, ingredient FROM presentations ORDER BY id").fetch_all(&pool).await?;
    let catalogue_size = catalogue.len();
    let lines = tokio::task::spawn_blocking(move || rank(&catalogue, &lines, min_score, per_line))
        .await
        .map_err(|e| ApiError::Upstream("internal", e.to_string()))?;

    let mut products: Vec<CitedProduct> = vec![];
    for line in &lines {
        for m in &line.matches {
            match products.iter_mut().find(|p| p.product.id == m.id) {
                Some(p) => {
                    p.product.score = p.product.score.max(m.score);
                    p.lines.push(line.index);
                }
                None => products.push(CitedProduct { product: m.clone(), lines: vec![line.index] }),
            }
        }
    }
    products.sort_by(|a, b| b.product.score.total_cmp(&a.product.score).then(a.product.id.cmp(&b.product.id)));
    Ok(Json(SimilarResponse { catalogue_size, method: "cosine_char_trigrams", lines, products }))
}

/// Character trigrams of each word, padded with spaces, counted.
pub fn trigrams(s: &str) -> HashMap<String, f64> {
    let mut v = HashMap::new();
    let folded: String = fold(s).chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    for word in folded.split_whitespace() {
        let padded: Vec<char> = format!(" {word} ").chars().collect();
        for w in padded.windows(3) {
            *v.entry(w.iter().collect()).or_insert(0.0) += 1.0;
        }
    }
    v
}

fn norm(v: &HashMap<String, f64>) -> f64 {
    v.values().map(|x| x * x).sum::<f64>().sqrt()
}

pub fn cosine(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let dot: f64 = small.iter().filter_map(|(k, x)| large.get(k).map(|y| x * y)).sum();
    let n = norm(a) * norm(b);
    if n == 0.0 { 0.0 } else { dot / n }
}

fn rank(catalogue: &[(i64, String, Option<String>)], lines: &[String], min_score: f64, per_line: usize) -> Vec<LineMatches> {
    let vectors: Vec<HashMap<String, f64>> = catalogue.iter().map(|(_, label, _)| trigrams(label)).collect();
    lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let q = trigrams(text);
            let mut scored: Vec<(usize, f64)> =
                vectors.iter().enumerate().map(|(i, v)| (i, cosine(&q, v))).filter(|(_, s)| *s >= min_score).collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            scored.truncate(per_line);
            let matches = scored
                .into_iter()
                .map(|(i, s)| {
                    let (id, label, ingredient) = &catalogue[i];
                    Similar { id: *id, label: label.clone(), ingredient: ingredient.clone(), score: (s * 1000.0).round() / 1000.0 }
                })
                .collect();
            LineMatches { index, text: text.clone(), matches }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_one_for_the_same_text_and_ignores_case_and_accents() {
        let a = trigrams("Paracétamol 500 mg");
        assert!((cosine(&a, &trigrams("PARACETAMOL 500 MG")) - 1.0).abs() < 1e-9);
        assert_eq!(cosine(&a, &trigrams("xyz")), 0.0);
        assert_eq!(cosine(&a, &HashMap::new()), 0.0);
    }

    #[test]
    fn closest_label_comes_first_despite_a_reading_error() {
        let catalogue = vec![
            (1, "DOLIPRANE 1000MG CP B/8".to_owned(), None),
            (2, "SPASFON LYOC 80MG B/10".to_owned(), None),
            (3, "AMOXICILLINE 500MG GELULE B/12".to_owned(), None),
        ];
        let lines = vec!["Dolipane 1000 mg".to_owned(), "Name of Patient".to_owned()];
        let r = rank(&catalogue, &lines, 0.3, 5);
        assert_eq!(r[0].matches[0].id, 1);
        assert!(r[1].matches.is_empty(), "a header matches nothing");
    }
}

// --- Structuring by a language model --------------------------------------------------------

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmRequest {
    /// Le texte lu sur l’ordonnance, par exemple `text` de `/v1/test/reducto-text`.
    #[schema(example = "Name of Patient\nAmoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab\n#21\nSig. Take one with food\nParacetamol 500 mg/tab #5")]
    text: String,
    /// Produits du catalogue proposés par médicament, de 0 à 10. 3 par défaut.
    per_item: Option<usize>,
}

/// A catalogue presentation proposed for a medicine read by the model.
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct LlmMatch {
    id: i64,
    label: String,
    ingredient: Option<String>,
    strength: Option<String>,
    dosage_form: Option<String>,
    pack: Option<String>,
    therapeutic_group: Option<String>,
    /// De 0 à 1 : molécule ou nom (50 %), dosage (25 %), forme (15 %), marque écrite dans le libellé (10 %).
    score: f64,
    /// `true` si tous les dosages écrits figurent dans la présentation.
    strength_match: bool,
    /// `true` si la forme écrite correspond, `false` si elle diffère, `null` si l’une des deux est inconnue.
    form_match: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct LlmItem {
    #[serde(flatten)]
    item: crate::llm::Item,
    /// `true` si le passage cité figure bien dans le texte envoyé. `false` signale une invention.
    grounded: bool,
    /// Présentations du catalogue les plus proches, meilleur score d’abord.
    matches: Vec<LlmMatch>,
}

#[derive(Serialize, ToSchema)]
pub struct LlmResponse {
    usage: crate::llm::Usage,
    items: Vec<LlmItem>,
    ignored: Vec<crate::llm::Ignored>,
}

fn letters_and_digits(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Each line of the quoted passage must be found in the text that was sent.
pub fn grounded(source_text: &str, text: &str) -> bool {
    let haystack = letters_and_digits(text);
    let lines: Vec<String> = source_text.lines().map(letters_and_digits).filter(|l| !l.is_empty()).collect();
    !lines.is_empty() && lines.iter().all(|l| haystack.contains(l.as_str()))
}

#[derive(sqlx::FromRow, Clone)]
pub struct CatalogueRow {
    id: i64,
    label: String,
    ingredient: Option<String>,
    strength: Option<String>,
    dosage_form: Option<String>,
    pack: Option<String>,
    therapeutic_group: Option<String>,
}

/// Word prefixes that name each form the model may return, in labels and `dosage_form`.
const FORMS: &[(&str, &[&str])] = &[
    ("comprime", &["comprim", "cp", "cpr", "comp", "tab", "lyoc"]),
    ("gelule", &["gelul", "caps", "gel"]),
    ("sirop", &["sirop", "sir"]),
    ("suspension", &["susp"]),
    ("sachet", &["sach", "poudre", "pdre", "granul"]),
    ("injectable", &["inject", "inj", "amp", "perf"]),
    ("suppositoire", &["suppo", "supp"]),
    ("pommade", &["pommade", "pde"]),
    ("creme", &["creme"]),
    ("gouttes", &["goutte", "gtt"]),
    ("collyre", &["collyre"]),
    ("spray", &["spray", "aerosol", "inhal"]),
];

fn forms_in(text: &str) -> Vec<&'static str> {
    let words: Vec<String> = fold(text).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect();
    FORMS
        .iter()
        .filter(|(_, prefixes)| words.iter().any(|w| prefixes.iter().any(|p| if p.len() <= 3 { w == p } else { w.starts_with(p) })))
        .map(|(f, _)| *f)
        .collect()
}

/// First word of the written name with at least 4 letters: "Co-Amoxiclav" gives "amoxiclav".
fn brand_word(name: &str) -> Option<String> {
    fold(name).split(|c: char| !c.is_alphanumeric()).find(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 4).map(str::to_owned)
}

pub fn rank_item(catalogue: &[CatalogueRow], item: &crate::llm::Item, per_item: usize) -> Vec<LlmMatch> {
    let by_name = trigrams(&item.name);
    let by_ingredient = (!item.ingredients.is_empty()).then(|| trigrams(&item.ingredients.join(" ")));
    let wanted = crate::prescriptions::strengths(&item.strengths.join(" "), false);
    let wanted_form = item.form.as_deref().map(fold).filter(|f| FORMS.iter().any(|(k, _)| k == f));
    let brand = brand_word(&item.name);
    let mut scored: Vec<(f64, LlmMatch)> = catalogue
        .iter()
        .filter_map(|r| {
            let dci = r.ingredient.as_deref().map(trigrams).map_or(0.0, |ing| {
                let from_name = cosine(&by_name, &ing);
                by_ingredient.as_ref().map_or(from_name, |q| cosine(q, &ing).max(from_name))
            });
            let identity = dci.max(cosine(&by_name, &trigrams(&r.label)));
            if identity < 0.3 {
                return None;
            }
            let have = crate::prescriptions::strengths(&format!("{} {}", r.label, r.strength.as_deref().unwrap_or("")), false);
            let strength_match = !wanted.is_empty() && wanted.iter().all(|n| have.contains(n));
            let strength = if wanted.is_empty() || have.is_empty() {
                0.5
            } else if strength_match {
                1.0
            } else if wanted.iter().any(|n| have.contains(n)) {
                0.4
            } else {
                0.0
            };
            let stored = forms_in(&format!("{} {}", r.dosage_form.as_deref().unwrap_or(""), r.label));
            let form_match = match &wanted_form {
                Some(f) if !stored.is_empty() => Some(stored.contains(&f.as_str())),
                _ => None,
            };
            let form = match form_match {
                Some(true) => 1.0,
                Some(false) => 0.0,
                None => 0.5,
            };
            let branded = brand.as_ref().is_some_and(|b| fold(&r.label).split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(b.as_str())));
            let score = 0.5 * identity + 0.25 * strength + 0.15 * form + if branded { 0.1 } else { 0.0 };
            Some((
                score,
                LlmMatch {
                    id: r.id,
                    label: r.label.clone(),
                    ingredient: r.ingredient.clone(),
                    strength: r.strength.clone(),
                    dosage_form: r.dosage_form.clone(),
                    pack: r.pack.clone(),
                    therapeutic_group: r.therapeutic_group.clone(),
                    score: (score * 1000.0).round() / 1000.0,
                    strength_match,
                    form_match,
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().take(per_item).map(|(_, m)| m).collect()
}

/// Test · structuration par un modèle de langage.
///
/// Envoie le texte au modèle configuré (Ollama local ou Ollama Cloud) avec le prompt
/// `prompts/prescription-structure.md`. Le modèle renvoie les médicaments en JSON. Chaque
/// passage cité est vérifié dans le texte (`grounded`), puis comparé au catalogue.
#[utoipa::path(post, path = "/v1/test/llm-structure", tag = "test", request_body = LlmRequest,
    responses(
        (status = 200, body = LlmResponse),
        (status = 400, body = crate::ErrorBody),
        (status = 502, description = "Le modèle a échoué ou n’a pas renvoyé le JSON attendu", body = crate::ErrorBody),
        (status = 503, description = "Aucun modèle configuré (LLM_MODEL)", body = crate::ErrorBody)))]
pub async fn llm_structure(
    State(state): State<AppState>,
    body: Result<Json<LlmRequest>, JsonRejection>,
) -> Result<Json<LlmResponse>, ApiError> {
    let Some(llm) = state.llm.clone() else {
        return Err(ApiError::Unavailable("llm_unavailable", "Aucun modèle de langage configuré (LLM_MODEL).".into()));
    };
    let Json(body) = body?;
    if body.text.trim().is_empty() || body.text.chars().count() > MAX_TEXT_CHARS {
        return Err(ApiError::BadRequest("invalid_text", format!("text doit contenir de 1 à {MAX_TEXT_CHARS} caractères")));
    }
    let per_item = body.per_item.unwrap_or(3).min(10);
    let (structured, usage) = llm.structure(&body.text).await.map_err(|e| {
        let detail = match e {
            crate::llm::LlmError::Unavailable(d) | crate::llm::LlmError::InvalidAnswer(d) => d,
        };
        tracing::error!(%detail, "structuration par le modèle impossible");
        ApiError::Upstream("llm_failed", format!("Le modèle de langage a échoué : {detail}"))
    })?;
    let catalogue: Vec<CatalogueRow> = sqlx::query_as(
        "SELECT id, label, ingredient, strength, dosage_form, pack, therapeutic_group FROM presentations ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    let text = body.text.clone();
    let items = tokio::task::spawn_blocking(move || {
        structured
            .items
            .into_iter()
            .map(|item| LlmItem { grounded: grounded(&item.source_text, &text), matches: rank_item(&catalogue, &item, per_item), item })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ApiError::Upstream("internal", e.to_string()))?;
    Ok(Json(LlmResponse { usage, items, ignored: structured.ignored }))
}

#[cfg(test)]
mod llm_tests {
    use super::*;

    #[test]
    fn quoted_passages_must_exist_in_the_text() {
        let text = "Amoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab\n#21";
        assert!(grounded("Amoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab", text));
        assert!(grounded("co-amoxiclav 500/125", text), "case and punctuation do not matter");
        assert!(!grounded("Augmentin 1 g", text));
        assert!(!grounded("", text));
    }

    fn row(id: i64, label: &str, ingredient: &str, strength: &str, form: &str) -> CatalogueRow {
        CatalogueRow {
            id,
            label: label.into(),
            ingredient: Some(ingredient.into()),
            strength: Some(strength.into()),
            dosage_form: Some(form.into()),
            pack: None,
            therapeutic_group: None,
        }
    }

    fn item(name: &str, ingredients: &[&str], strengths: &[&str], form: Option<&str>) -> crate::llm::Item {
        crate::llm::Item {
            source_text: name.into(),
            name: name.into(),
            ingredients: ingredients.iter().map(|s| s.to_string()).collect(),
            strengths: strengths.iter().map(|s| s.to_string()).collect(),
            form: form.map(Into::into),
            boxes: None,
            units: None,
            therapeutic_class: None,
        }
    }

    #[test]
    fn strength_and_form_decide_between_presentations_of_the_same_molecules() {
        let catalogue = vec![
            row(14, "ACECLAV NOUR 250MG/31.25MG PDRE SUSP BUV FL/60ML", "AMOXICILLINE + ACIDE CLAVULANIQUE", "250MG/31.25MG", "Suspension"),
            row(15, "ACECLAV 1125MG PDRE SUSP BUV SACH B/16", "AMOXICILLINE + ACIDE CLAVULANIQUE", "1125MG", "Suspension"),
            row(1439, "CO-AMOXICLAV SP ENFANT PDRE POUR SUSP BUVABLE 500MG/125MG B/12", "AMOXICILLINE + ACIDE CLAVULANIQUE", "500MG/125MG", "Suspension"),
            row(341, "AMOCLAN FORTE CP 500 MG/125 MG B/15", "AMOXICILLINE+ACIDE CLAVULANIQUE", "500MG/125MG", "Comprimé"),
        ];
        let m = rank_item(&catalogue, &item("Co-Amoxiclav", &["amoxicilline", "acide clavulanique"], &["500 mg", "125 mg"], Some("comprimé")), 4);
        assert_eq!(m[0].id, 341, "{m:?}");
        assert!(m[0].strength_match && m[0].form_match == Some(true));
        assert_eq!(m[1].id, 1439, "same strength, wrong form");
    }

    #[test]
    fn a_molecule_name_matches_without_ingredients_and_a_written_brand_comes_first() {
        let catalogue = vec![
            row(4175, "PARACETAMOL CP 500 MG B/10", "PARACETAMOL", "500MG", "Comprimé"),
            row(4178, "PARACETAMOL GELULE 500 MG B/24", "PARACETAMOL", "500MG", "Gélule"),
            row(1, "DOLIPRANE 1000MG CP B/8", "PARACETAMOL", "1000MG", "Comprimé"),
            row(2, "PARACETAMOL CP 1000 MG B/8", "PARACETAMOL", "1000MG", "Comprimé"),
        ];
        assert_eq!(rank_item(&catalogue, &item("Paracetamol", &[], &["500 mg"], Some("comprimé")), 1)[0].id, 4175);
        assert_eq!(rank_item(&catalogue, &item("Doliprane", &["paracétamol"], &["1000 mg"], None), 1)[0].id, 1);
    }
}
