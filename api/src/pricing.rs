//! Price ranges for a prescription read in the chat.
//!
//! A prescribed medicine is sold under several brands and box sizes, each with its own price.
//! For each line, the equivalent presentations are those with the same molecules, the same
//! strengths and the same form as the line (or as its best candidate when the line does not say).
//! Each one is priced with its latest price published by the MUGEF-CI. When the prescription
//! says how many units (« #21 ») or boxes (« N°2 ») to deliver, the cost counts the boxes needed;
//! otherwise it is the price of one box. The range goes from the cheapest to the dearest option.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::{
    ApiError,
    prescriptions::{LineStatus, ReadResponse, fold, strengths, written_form},
};

const OPTIONS_SHOWN: usize = 8;

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Le nombre d’unités est écrit (« #21 ») : le coût compte les boîtes nécessaires.
    Units,
    /// Le nombre de boîtes est écrit (« N°2 »).
    Boxes,
    /// Aucune quantité écrite : prix d’une boîte.
    PerBox,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct PriceOption {
    presentation_id: i64,
    label: String,
    /// Unités par boîte, quand le conditionnement les compte (B/15, plaquette de 10).
    pack_size: Option<u32>,
    boxes: u32,
    #[schema(value_type = String, example = "5955.00")]
    box_price_fcfa: Decimal,
    #[schema(value_type = String, example = "11910.00")]
    cost_fcfa: Decimal,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct PriceRange {
    basis: Basis,
    #[schema(value_type = String)]
    min_fcfa: Decimal,
    #[schema(value_type = String)]
    max_fcfa: Decimal,
    /// Nombre de présentations équivalentes chiffrées.
    count: usize,
    /// Les moins chères d’abord, 8 au plus.
    options: Vec<PriceOption>,
    /// Date de publication la plus récente des prix utilisés.
    published_on: NaiveDate,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Estimate {
    #[schema(value_type = String)]
    min_fcfa: Decimal,
    #[schema(value_type = String)]
    max_fcfa: Decimal,
    priced_lines: usize,
    total_lines: usize,
    /// `true` si une ligne est chiffrée à la boîte faute de quantité écrite.
    per_box_lines: usize,
    /// Toujours `mugefci_published` : prix publiés, pas relevés au comptoir.
    source: &'static str,
}

/// Units in one box: "B/15", "B / 20", "BTE 24", "PLAQUETTE DE 10", "B/2X10". Volumes and
/// weights (FL/60 ML, T/30 G) are not counted: `None`.
pub fn pack_size(text: &str) -> Option<u32> {
    let t = fold(text);
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut found = None;
    for (i, w) in words.iter().enumerate() {
        if !matches!(*w, "b" | "bte" | "btes" | "boite" | "boites" | "plaquette" | "plaquettes" | "plq" | "etui") {
            continue;
        }
        let mut j = i + 1;
        if words.get(j) == Some(&"de") {
            j += 1;
        }
        let Some(n) = words.get(j).and_then(|w| w.parse::<u32>().ok()) else {
            // "B/2X10" splits into "2x10".
            if let Some((a, b)) = words.get(j).and_then(|w| w.split_once('x')) {
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    found = Some(a * b);
                }
            }
            continue;
        };
        // A unit right after the number is a volume or a weight, not a count.
        if matches!(words.get(j + 1), Some(&("ml" | "g" | "mg" | "l"))) {
            continue;
        }
        let n = match (words.get(j + 1), words.get(j + 2).and_then(|w| w.parse::<u32>().ok())) {
            (Some(&"x"), Some(m)) => n * m,
            _ => n,
        };
        found = Some(n);
    }
    found.filter(|n| *n > 0 && *n <= 1000)
}

/// Molecules compared without spaces, accents or punctuation: "AMOXICILLINE + ACIDE CLAVULANIQUE"
/// and "AMOXICILLINE+ACIDE CLAVULANIQUE" are the same.
fn molecules(ingredient: &str) -> String {
    fold(ingredient).chars().filter(|c| c.is_alphanumeric()).collect()
}

fn strength_set(text: &str) -> BTreeSet<u64> {
    strengths(text, false).into_iter().collect()
}

/// The form family of a presentation: its label first, which is more reliable than `dosage_form`.
fn form_of(label: &str, dosage_form: Option<&str>) -> Option<&'static str> {
    written_form(&fold(label)).or_else(|| written_form(&fold(dosage_form.unwrap_or(""))))
}

#[derive(sqlx::FromRow)]
struct Priced {
    id: i64,
    label: String,
    ingredient: Option<String>,
    strength: Option<String>,
    dosage_form: Option<String>,
    pack: Option<String>,
    amount_fcfa: Decimal,
    observed_on: NaiveDate,
}

/// What the line asks for, taken from its text and completed by its best candidate.
struct Wanted {
    molecules: String,
    first_word: String,
    strengths: BTreeSet<u64>,
    form: Option<&'static str>,
}

fn wanted(line: &crate::prescriptions::PrescriptionLine) -> Option<Wanted> {
    if line.status == LineStatus::NoMatch {
        return None;
    }
    let best = &line.candidates.first()?.presentation;
    let ingredient = best.ingredient.as_deref()?;
    let first_word = fold(ingredient).split(|c: char| !c.is_alphanumeric()).find(|w| w.len() >= 4)?.to_owned();
    let written = strength_set(&line.text);
    let strengths = if written.is_empty() {
        strength_set(&format!("{} {}", best.label, best.strength.as_deref().unwrap_or("")))
    } else {
        written
    };
    if strengths.is_empty() {
        return None;
    }
    let form = written_form(&fold(&line.text)).or_else(|| form_of(&best.label, best.dosage_form.as_deref()));
    Some(Wanted { molecules: molecules(ingredient), first_word, strengths, form })
}

fn range_for(line: &crate::prescriptions::PrescriptionLine, rows: &[Priced], want: &Wanted) -> Option<PriceRange> {
    let basis = match (line.units, line.quantity) {
        (Some(u), _) if u > 0 => Basis::Units,
        (_, Some(q)) if q > 0 => Basis::Boxes,
        _ => Basis::PerBox,
    };
    let mut options: Vec<(PriceOption, NaiveDate)> = rows
        .iter()
        .filter(|r| r.ingredient.as_deref().map(molecules).as_deref() == Some(want.molecules.as_str()))
        .filter(|r| strength_set(&format!("{} {}", r.label, r.strength.as_deref().unwrap_or(""))) == want.strengths)
        .filter(|r| want.form.is_none() || form_of(&r.label, r.dosage_form.as_deref()) == want.form)
        .filter_map(|r| {
            let pack_size = pack_size(r.pack.as_deref().unwrap_or("")).or_else(|| pack_size(&r.label));
            let boxes = match basis {
                Basis::Units => (line.units? as u32).div_ceil(pack_size?),
                Basis::Boxes => line.quantity? as u32,
                Basis::PerBox => 1,
            };
            let option = PriceOption {
                presentation_id: r.id,
                label: r.label.clone(),
                pack_size,
                boxes,
                box_price_fcfa: r.amount_fcfa,
                cost_fcfa: r.amount_fcfa * Decimal::from(boxes),
            };
            Some((option, r.observed_on))
        })
        .collect();
    if options.is_empty() {
        return None;
    }
    options.sort_by(|a, b| a.0.cost_fcfa.cmp(&b.0.cost_fcfa).then_with(|| a.0.label.cmp(&b.0.label)));
    let published_on = options.iter().map(|(_, d)| *d).max()?;
    let min_fcfa = options.first()?.0.cost_fcfa;
    let max_fcfa = options.last()?.0.cost_fcfa;
    let count = options.len();
    Some(PriceRange {
        basis,
        min_fcfa,
        max_fcfa,
        count,
        options: options.into_iter().take(OPTIONS_SHOWN).map(|(o, _)| o).collect(),
        published_on,
    })
}

/// Fills `price` on each line and `estimate` on the reading.
pub async fn add_price_ranges(pool: &PgPool, read: &mut ReadResponse) -> Result<(), ApiError> {
    let total_lines = read.lines.len();
    for line in &mut read.lines {
        let Some(want) = wanted(line) else { continue };
        let rows: Vec<Priced> = sqlx::query_as(
            "SELECT p.id, p.label, p.ingredient, p.strength, p.dosage_form, p.pack, pr.amount_fcfa, pr.observed_on
             FROM presentations p
             JOIN LATERAL (SELECT amount_fcfa, observed_on FROM prices
                           WHERE presentation_id = p.id AND kind = 'mugefci_published'
                           ORDER BY observed_on DESC, amount_fcfa DESC LIMIT 1) pr ON true
             WHERE strpos(lower(p.ingredient), $1) > 0",
        )
        .bind(&want.first_word)
        .fetch_all(pool)
        .await?;
        line.price = range_for(line, &rows, &want);
    }
    let priced: Vec<&PriceRange> = read.lines.iter().filter_map(|l| l.price.as_ref()).collect();
    if !priced.is_empty() {
        read.estimate = Some(Estimate {
            min_fcfa: priced.iter().map(|p| p.min_fcfa).sum(),
            max_fcfa: priced.iter().map(|p| p.max_fcfa).sum(),
            priced_lines: priced.len(),
            total_lines,
            per_box_lines: priced.iter().filter(|p| p.basis == Basis::PerBox).count(),
            source: "mugefci_published",
        });
    }
    Ok(())
}

/// "10 140 FCFA", or "10 140 à 15 750 FCFA" for a range.
pub fn fcfa(min: Decimal, max: Decimal) -> String {
    let fmt = |d: Decimal| {
        let n = d.round().to_string();
        let mut out = String::new();
        for (i, c) in n.chars().enumerate() {
            if i > 0 && (n.len() - i) % 3 == 0 {
                out.push('\u{202f}');
            }
            out.push(c);
        }
        out
    };
    if min == max { format!("{} FCFA", fmt(min)) } else { format!("{} à {} FCFA", fmt(min), fmt(max)) }
}

impl Estimate {
    pub fn min_fcfa(&self) -> Decimal {
        self.min_fcfa
    }

    pub fn max_fcfa(&self) -> Decimal {
        self.max_fcfa
    }
}

impl PriceRange {
    pub fn describe(&self) -> String {
        let per_box = if self.basis == Basis::PerBox { " la boîte" } else { "" };
        let brands = if self.count > 1 { format!(", selon la marque ({} présentations)", self.count) } else { String::new() };
        format!("{}{per_box}{brands}", fcfa(self.min_fcfa, self.max_fcfa))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_sizes_count_units_but_not_volumes() {
        assert_eq!(pack_size("B/15"), Some(15));
        assert_eq!(pack_size("PARACETAMOL AFRICURE 500MG CPR B / 20"), Some(20));
        assert_eq!(pack_size("PLAQUETTE DE 10"), Some(10));
        assert_eq!(pack_size("CLAVOR COMPRIME 500/62,5BTE 24"), None, "glued to the strength: unreadable");
        assert_eq!(pack_size("BTE 24"), Some(24));
        assert_eq!(pack_size("B/2X10"), Some(20));
        assert_eq!(pack_size("FL/60 ML"), None);
        assert_eq!(pack_size("T/30 G"), None);
    }

    #[test]
    fn amounts_are_written_the_french_way() {
        assert_eq!(fcfa(Decimal::from(10140), Decimal::from(15750)), "10\u{202f}140 à 15\u{202f}750 FCFA");
        assert_eq!(fcfa(Decimal::from(105), Decimal::from(105)), "105 FCFA");
    }

    #[test]
    fn same_molecules_whatever_the_spacing() {
        assert_eq!(molecules("AMOXICILLINE + ACIDE CLAVULANIQUE"), molecules("AMOXICILLINE+ACIDE CLAVULANIQUE"));
    }
}
