//! WP-7.6 name-based detectors: annuals, alternate editions, facsimiles,
//! supplements and translations.
//!
//! They all compare series *names* (plus years, publisher, issue numbers and
//! language), so they share one set-based query: [`Catalogue::load`] reads
//! every live series of the library with its issue aggregates once, and each
//! detector works over hash indexes on the normalized base name
//! ([`super::sources::match_key`]). Lookups are by key, never all-pairs, so a
//! run stays linear in the number of series. The translation detector adds
//! two small set-based queries (provider claims, shared creators) bounded by
//! the catalogue's candidate groups.
//!
//! Confidence numbers are documented in `docs/dev/series-relationships.md`
//! ("Evidence sources and confidence").

use super::sources::{
    base_name, collected_by, label, lang_sql, match_key, norm_lang, provider_claims_cte, round2,
    stmt,
};
use super::{Candidate, EvidenceSource};
use crate::relationships::{RelationshipCoverage, RelationshipKind, Scope};
use sea_orm::{ConnectionTrait, DbErr, FromQueryResult, Value};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Upper bound on catalogue rows (series per library).
const CATALOGUE_LIMIT: i64 = 200_000;
/// A same-title group larger than this is too generic for the translation
/// detector's title + creators pairing.
const TRANSLATION_GROUP_MAX: usize = 12;

#[derive(Debug, FromQueryResult)]
struct CatRow {
    id: Uuid,
    name: String,
    normalized_name: String,
    year: Option<i32>,
    year_end: Option<i32>,
    publisher: Option<String>,
    series_type: Option<String>,
    language_code: String,
    aliases: serde_json::Value,
    alternate_names: serde_json::Value,
    n_issues: i64,
    lo: Option<f64>,
    hi: Option<f64>,
    iy_min: Option<i32>,
    iy_max: Option<i32>,
    formats: Option<Vec<String>>,
    annual_issues: i64,
    first_number: Option<String>,
}

/// One series of the library, with what the detectors compare.
#[derive(Debug, Clone)]
pub struct Series {
    pub id: Uuid,
    pub name: String,
    pub norm: String,
    /// [`match_key`] of the normalized name.
    pub key: String,
    pub year: Option<i32>,
    pub year_end: Option<i32>,
    /// Lowercased, trimmed; `None` when unknown.
    pub publisher: Option<String>,
    pub series_type: Option<String>,
    /// ISO 639-1 where known ([`norm_lang`]); empty when unknown.
    pub lang: String,
    /// Normalized aliases and alternate names.
    pub aliases: Vec<String>,
    pub n_issues: i64,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub iy_min: Option<i32>,
    pub iy_max: Option<i32>,
    /// Distinct issue `Format` / `special_type` values.
    pub formats: Vec<String>,
    /// Issues whose `Format` or `special_type` says Annual.
    pub annual_issues: i64,
    /// `number_raw` of the lowest-numbered issue.
    pub first_number: Option<String>,
}

impl Series {
    /// First year: the series year or its earliest issue.
    pub fn start(&self) -> Option<i32> {
        [self.year, self.iy_min].into_iter().flatten().min()
    }

    /// Last year: `year_end`, the latest issue, or the series year.
    pub fn end(&self) -> Option<i32> {
        [self.year_end, self.iy_max, self.year]
            .into_iter()
            .flatten()
            .max()
    }

    fn collected(&self) -> bool {
        collected_by(&self.name, self.series_type.as_deref(), &self.formats)
    }

    fn label(&self) -> String {
        label(&self.name, self.year)
    }

    fn tokens(&self) -> Vec<&str> {
        self.norm.split(' ').filter(|w| !w.is_empty()).collect()
    }
}

/// `Some(true)` same publisher, `Some(false)` different, `None` unknown.
fn same_publisher(a: &Series, b: &Series) -> Option<bool> {
    match (&a.publisher, &b.publisher) {
        (Some(x), Some(y)) => Some(x == y),
        _ => None,
    }
}

fn years_overlap(a: &Series, b: &Series) -> Option<bool> {
    let (a0, a1, b0, b1) = (a.start()?, a.end()?, b.start()?, b.end()?);
    Some(a0 <= b1 && b0 <= a1)
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(entity::series::normalize_name)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Every live series of one library, indexed by [`match_key`] and by
/// normalized name.
pub struct Catalogue {
    pub series: Vec<Series>,
    by_key: HashMap<String, Vec<usize>>,
    by_norm: HashMap<String, Vec<usize>>,
}

impl Catalogue {
    /// One set-based query: series + per-series issue aggregates.
    pub async fn load<C: ConnectionTrait>(conn: &C, library_id: Uuid) -> Result<Self, DbErr> {
        let sql = format!(
            r#"
            SELECT s.id, s.name, s.normalized_name, s.year, s.year_end,
                   nullif(lower(btrim(s.publisher)), '') AS publisher, s.series_type,
                   s.language_code, s.aliases, s.alternate_names,
                   a.n AS n_issues, a.lo, a.hi, a.iy_min, a.iy_max, a.formats,
                   a.annual_n AS annual_issues, a.first_number
              FROM series s
              -- Per-series index lookups (issues_series_sortnum_idx): robust
              -- to missing statistics, unlike a join on a grouped subquery.
              CROSS JOIN LATERAL (
                  SELECT count(*) AS n, min(i.sort_number) AS lo, max(i.sort_number) AS hi,
                         min(i.year) AS iy_min, max(i.year) AS iy_max,
                         array_remove(array_agg(DISTINCT i.format)
                                      || array_agg(DISTINCT i.special_type), NULL) AS formats,
                         count(*) FILTER (WHERE i.special_type = 'Annual'
                                             OR lower(i.format) IN ('annual', 'annuals', 'annual series'))
                             AS annual_n,
                         (array_agg(i.number_raw ORDER BY i.sort_number NULLS LAST, i.id))[1]
                             AS first_number
                    FROM issues i
                   WHERE i.series_id = s.id AND i.removed_at IS NULL
              ) a
             WHERE s.library_id = $1 AND s.removed_at IS NULL
             ORDER BY s.id
             LIMIT {CATALOGUE_LIMIT}
            "#
        );
        let rows = CatRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
            .all(conn)
            .await?;
        let series: Vec<Series> = rows
            .into_iter()
            .map(|r| {
                let mut aliases = strings(&r.aliases);
                aliases.extend(strings(&r.alternate_names));
                aliases.sort();
                aliases.dedup();
                Series {
                    id: r.id,
                    key: match_key(&r.normalized_name),
                    name: r.name,
                    norm: r.normalized_name,
                    year: r.year,
                    year_end: r.year_end,
                    publisher: r.publisher,
                    series_type: r.series_type,
                    lang: norm_lang(&r.language_code),
                    aliases,
                    n_issues: r.n_issues,
                    lo: r.lo,
                    hi: r.hi,
                    iy_min: r.iy_min,
                    iy_max: r.iy_max,
                    formats: r.formats.unwrap_or_default(),
                    annual_issues: r.annual_issues,
                    first_number: r.first_number,
                }
            })
            .collect();
        let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_norm: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, s) in series.iter().enumerate() {
            by_key.entry(s.key.clone()).or_default().push(i);
            by_norm.entry(s.norm.clone()).or_default().push(i);
        }
        Ok(Self {
            series,
            by_key,
            by_norm,
        })
    }

    fn with_key(&self, key: &str) -> impl Iterator<Item = &Series> {
        self.by_key
            .get(key)
            .into_iter()
            .flatten()
            .map(|&i| &self.series[i])
    }

    fn with_norm(&self, norm: &str) -> impl Iterator<Item = &Series> {
        self.by_norm
            .get(norm)
            .into_iter()
            .flatten()
            .map(|&i| &self.series[i])
    }
}

/// Position of `phrase` (consecutive tokens) in `tokens`.
fn find_phrase(tokens: &[&str], phrase: &[&str]) -> Option<usize> {
    if phrase.is_empty() || tokens.len() < phrase.len() {
        return None;
    }
    (0..=tokens.len() - phrase.len()).find(|&i| tokens[i..i + phrase.len()] == *phrase)
}

/// `tokens` with `phrase` at `at` removed, joined.
fn without(tokens: &[&str], at: usize, len: usize) -> Vec<String> {
    tokens
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < at || *i >= at + len)
        .map(|(_, t)| (*t).to_owned())
        .collect()
}

// ───── annual → annual_of ─────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnualSignal {
    /// The name ends in "Annual" / "Annuals" (before a year / volume).
    Name,
    /// `series_type` says Annual (Metron's "Annual Series").
    SeriesType,
    /// ≥ 80% of its issues have `Format` / `special_type` Annual.
    Format,
}

/// Is `s` an annual series, and what is the main series' key?
fn annual_signal(s: &Series) -> Option<(AnnualSignal, String)> {
    let base = base_name(&s.norm);
    let words: Vec<&str> = base.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() >= 2 && matches!(words[words.len() - 1], "annual" | "annuals") {
        let key = match_key(&words[..words.len() - 1].join(" "));
        if !key.is_empty() {
            return Some((AnnualSignal::Name, key));
        }
    }
    if s.series_type.as_deref().is_some_and(|t| {
        matches!(
            t.trim().to_lowercase().as_str(),
            "annual" | "annuals" | "annual series"
        )
    }) {
        return Some((AnnualSignal::SeriesType, s.key.clone()));
    }
    if s.n_issues > 0 && s.annual_issues * 5 >= s.n_issues * 4 {
        return Some((AnnualSignal::Format, s.key.clone()));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum YearFit {
    /// The annual's years lie inside the main volume's (+1 year slack).
    Contained,
    /// Overlapping or adjacent (±1 year).
    Overlap,
    /// A year is unknown on either side.
    Unknown,
    /// Too far apart.
    Far,
}

fn year_fit(annual: &Series, main: &Series) -> YearFit {
    match (annual.start(), annual.end(), main.start(), main.end()) {
        (Some(a0), Some(a1), Some(m0), Some(m1)) => {
            if m0 <= a0 && a1 <= m1 + 1 {
                YearFit::Contained
            } else if a0 <= m1 + 1 && a1 >= m0 - 1 {
                YearFit::Overlap
            } else {
                YearFit::Far
            }
        }
        _ => YearFit::Unknown,
    }
}

/// Annual series (name "X Annual", Metron series type "Annual Series", or
/// issues formatted Annual) → `annual_of` the main series `X`: same
/// normalized base name, same publisher, overlapping or adjacent years.
/// The volume whose years contain the annual's wins.
pub fn annuals(cat: &Catalogue) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in &cat.series {
        let Some((signal, key)) = annual_signal(s) else {
            continue;
        };
        let mut cands: Vec<(YearFit, i32, &Series)> = cat
            .with_key(&key)
            .filter(|m| m.id != s.id && annual_signal(m).is_none())
            .filter(|m| same_publisher(s, m) != Some(false))
            .map(|m| {
                let gap = match (s.start(), m.start()) {
                    (Some(a), Some(b)) => (a - b).abs(),
                    _ => i32::MAX,
                };
                (year_fit(s, m), gap, m)
            })
            .filter(|(fit, _, _)| *fit != YearFit::Far)
            .collect();
        if cands.is_empty() {
            continue;
        }
        cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.id.cmp(&b.2.id)));
        let (fit, _, main) = cands[0];
        let same_fit = cands.iter().filter(|c| c.0 == fit).count();
        let mut c: f32 = match signal {
            AnnualSignal::Name => 0.9,
            AnnualSignal::SeriesType | AnnualSignal::Format => 0.8,
        };
        c -= match fit {
            YearFit::Contained => 0.0,
            YearFit::Overlap => 0.15,
            YearFit::Unknown | YearFit::Far => 0.35,
        };
        if same_fit > 1 {
            c -= 0.15;
        }
        let pub_known = same_publisher(s, main) == Some(true);
        if !pub_known {
            c -= 0.1;
        }
        let years = match (main.start(), main.end()) {
            (Some(a), Some(b)) if a != b => format!("{a}–{b}"),
            (Some(a), _) => a.to_string(),
            _ => "unknown years".into(),
        };
        let why_signal = match signal {
            AnnualSignal::Name => "named as an annual",
            AnnualSignal::SeriesType => "series type Annual",
            AnnualSignal::Format => "its issues are formatted Annual",
        };
        let why_years = match fit {
            YearFit::Contained => format!("within that volume's {years}"),
            YearFit::Overlap => format!("next to that volume's {years}"),
            YearFit::Unknown | YearFit::Far => "years unknown".to_owned(),
        };
        out.push(Candidate {
            from: s.id,
            to: main.id.into(),
            kind: RelationshipKind::AnnualOf,
            confidence: round2(c.max(0.2)),
            source: EvidenceSource::Annual,
            reason: format!(
                "{} is an annual of {} ({why_signal}; {}{why_years}{})",
                s.label(),
                main.label(),
                if pub_known { "same publisher; " } else { "" },
                if same_fit > 1 {
                    format!("; {same_fit} volumes fit equally")
                } else {
                    String::new()
                }
            ),
            evidence: json!({
                "source": "annual",
                "signal": format!("{signal:?}").to_lowercase(),
                "year_fit": format!("{fit:?}").to_lowercase(),
                "candidates": cands.len(),
                "same_publisher": same_publisher(s, main),
            }),
            scope: Scope::default(),
        });
    }
    out
}

// ───── alternate editions → alternate_edition_of; facsimiles → reprints ─────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditionStrength {
    /// Director's Cut, Remastered, Colorized, Deluxe, Gallery / Artist's /
    /// Treasury / Special Edition.
    Strong,
    /// "Absolute": only a collected edition counts ("Absolute Carnage" and
    /// "Absolute Batman" are an event and a new line, not editions).
    Absolute,
    /// "Unlimited": as often an anthology or digital-first title.
    Weak,
}

const EDITION_MARKERS: &[(&[&str], &str, EditionStrength)] = &[
    (
        &["directors", "cut"],
        "Director's Cut",
        EditionStrength::Strong,
    ),
    (
        &["artists", "edition"],
        "Artist's Edition",
        EditionStrength::Strong,
    ),
    (
        &["artist", "edition"],
        "Artist's Edition",
        EditionStrength::Strong,
    ),
    (
        &["gallery", "edition"],
        "Gallery Edition",
        EditionStrength::Strong,
    ),
    (
        &["treasury", "edition"],
        "Treasury Edition",
        EditionStrength::Strong,
    ),
    (
        &["special", "edition"],
        "Special Edition",
        EditionStrength::Strong,
    ),
    (
        &["deluxe", "edition"],
        "Deluxe Edition",
        EditionStrength::Strong,
    ),
    (&["deluxe"], "Deluxe", EditionStrength::Strong),
    (&["remastered"], "Remastered", EditionStrength::Strong),
    (&["colorized"], "Colorized", EditionStrength::Strong),
    (&["colourized"], "Colorized", EditionStrength::Strong),
    (&["absolute"], "Absolute", EditionStrength::Absolute),
    (&["unlimited"], "Unlimited", EditionStrength::Weak),
];

/// The edition marker in `s`'s name and the base title's key.
fn edition_marker(s: &Series) -> Option<(&'static str, EditionStrength, String)> {
    let tokens = s.tokens();
    for (phrase, label, strength) in EDITION_MARKERS {
        if let Some(at) = find_phrase(&tokens, phrase) {
            let mut rest = without(&tokens, at, phrase.len());
            rest.retain(|w| w != "edition");
            let key = match_key(&rest.join(" "));
            if !key.is_empty() {
                return Some((label, *strength, key));
            }
        }
    }
    None
}

fn is_facsimile(s: &Series) -> bool {
    s.tokens().contains(&"facsimile")
}

/// Same base title once an edition marker is stripped (Deluxe, Absolute,
/// Director's Cut, Remastered, Colorized, Artist's / Gallery / Special /
/// Treasury Edition, Unlimited) → `alternate_edition_of` the unmarked
/// series, when their years or issue ranges overlap. A collected edition
/// of a singles run is left to the collected-edition / reprint sources
/// (that's `collects`, not an alternate edition).
pub fn alternate_editions(cat: &Catalogue) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in &cat.series {
        if is_facsimile(s) {
            continue;
        }
        let Some((marker, strength, key)) = edition_marker(s) else {
            continue;
        };
        if strength == EditionStrength::Absolute && !s.collected() {
            continue;
        }
        let mut cands: Vec<(bool, bool, i32, &Series)> = cat
            .with_key(&key)
            .filter(|b| b.id != s.id && edition_marker(b).is_none() && !is_facsimile(b))
            .filter(|b| same_publisher(s, b) != Some(false))
            .filter(|b| !(s.collected() && !b.collected()))
            .filter(|b| match (s.start(), b.start()) {
                // An edition can't predate the work.
                (Some(e), Some(o)) => e >= o - 1,
                _ => true,
            })
            .filter_map(|b| {
                let years = years_overlap(s, b).unwrap_or(false);
                let content = matches!(
                    (s.lo, s.hi, b.lo, b.hi),
                    (Some(sl), Some(sh), Some(bl), Some(bh)) if sl >= bl && sh <= bh
                );
                let gap = match (s.start(), b.start()) {
                    (Some(e), Some(o)) => e - o,
                    _ => i32::MAX,
                };
                (years || content).then_some((years, content, gap, b))
            })
            .collect();
        if cands.is_empty() {
            continue;
        }
        cands.sort_by(|a, b| {
            (b.0 && b.1)
                .cmp(&(a.0 && a.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.id.cmp(&b.3.id))
        });
        let (years, content, _, base) = cands[0];
        let mut c: f32 = match strength {
            EditionStrength::Strong | EditionStrength::Absolute => 0.6,
            EditionStrength::Weak => 0.4,
        };
        if years && content {
            c += 0.05;
        }
        if cands.len() > 1 {
            c -= 0.05;
        }
        if same_publisher(s, base).is_none() {
            c -= 0.05;
        }
        let overlap = match (years, content) {
            (true, true) => "overlapping years and issue numbers",
            (true, false) => "overlapping years",
            _ => "the issue numbers fall inside the original's",
        };
        out.push(Candidate {
            from: s.id,
            to: base.id.into(),
            kind: RelationshipKind::AlternateEditionOf,
            confidence: round2(c),
            source: EvidenceSource::AlternateEdition,
            reason: format!(
                "{} looks like a {marker} edition of {} (same base title, {overlap})",
                s.label(),
                base.label()
            ),
            evidence: json!({
                "source": "alternate_edition",
                "marker": marker,
                "years_overlap": years,
                "content_overlap": content,
                "candidates": cands.len(),
            }),
            scope: Scope::default(),
        });
    }
    out
}

/// "ROM #1 Facsimile Edition" reproduces one specific issue: `reprints` the
/// base series with `to_range` = that number (from the name, else the
/// facsimile's own first issue number) and coverage `full`. Not an
/// alternate edition.
pub fn facsimiles(cat: &Catalogue) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in cat.series.iter().filter(|s| is_facsimile(s)) {
        let tokens: Vec<&str> = s
            .tokens()
            .into_iter()
            .filter(|t| !matches!(*t, "facsimile" | "edition"))
            .collect();
        if tokens.is_empty() {
            continue;
        }
        // A trailing number is the reproduced issue ("rom 1") when the title
        // without it names a series; otherwise it is part of the title.
        let last = tokens[tokens.len() - 1];
        let numbered =
            tokens.len() >= 2 && last.len() <= 4 && last.chars().all(|c| c.is_ascii_digit());
        let mut key = match_key(&tokens.join(" "));
        let mut number = s.first_number.clone();
        if numbered {
            let short = match_key(&tokens[..tokens.len() - 1].join(" "));
            if cat.with_key(&short).any(|b| b.id != s.id) {
                key = short;
                number = Some(last.to_owned());
            }
        }
        let Some(number) = number
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        let n: Option<f64> = number.parse().ok();
        let mut cands: Vec<(bool, i32, &Series)> = cat
            .with_key(&key)
            .filter(|b| b.id != s.id && !is_facsimile(b))
            .filter(|b| same_publisher(s, b) != Some(false))
            .map(|b| {
                let holds =
                    matches!((n, b.lo, b.hi), (Some(n), Some(lo), Some(hi)) if lo <= n && n <= hi);
                (holds, b.start().unwrap_or(i32::MAX), b)
            })
            .collect();
        if cands.is_empty() {
            continue;
        }
        // The series holding that number first, then the oldest (facsimiles
        // reproduce old issues).
        cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.id.cmp(&b.2.id)));
        let (holds, _, base) = cands[0];
        let mut c: f32 = if holds { 0.75 } else { 0.6 };
        if cands.len() > 1 {
            c -= 0.1;
        }
        out.push(Candidate {
            from: s.id,
            to: base.id.into(),
            kind: RelationshipKind::Reprints,
            confidence: round2(c),
            source: EvidenceSource::Facsimile,
            reason: format!(
                "{} is a facsimile edition of {} #{number}{}",
                s.label(),
                base.label(),
                if holds {
                    " (that issue is in the library)"
                } else {
                    ""
                }
            ),
            evidence: json!({
                "source": "facsimile",
                "issue_number": number,
                "issue_in_library_range": holds,
                "candidates": cands.len(),
            }),
            scope: Scope {
                to_range: Some(number),
                coverage: Some(RelationshipCoverage::Full),
                ..Scope::default()
            },
        });
    }
    out
}

// ───── supplements → supplement_to ─────

/// Supplement markers with their base confidence and whether the marker
/// must end the name ("Annihilation Saga", but not "Saga of the Swamp
/// Thing").
const SUPPLEMENT_MARKERS: &[(&str, f32, bool)] = &[
    ("handbook", 0.6, false),
    ("guidebook", 0.6, false),
    ("sourcebook", 0.6, false),
    ("guide", 0.55, true),
    ("saga", 0.55, true),
    ("spotlight", 0.5, true),
    ("special", 0.45, true),
];

/// Occasion words dropped before a trailing "Special" ("Fantastic Four:
/// Wedding Special" supplements "Fantastic Four").
const OCCASION_WORDS: &[&str] = &[
    "holiday",
    "halloween",
    "xmas",
    "christmas",
    "wedding",
    "anniversary",
    "valentines",
    "summer",
    "winter",
    "spring",
];

/// The supplement marker, its confidence and the parent title's key.
fn supplement_marker(s: &Series) -> Option<(&'static str, f32, String)> {
    let tokens = s.tokens();
    let last = *tokens.last()?;
    let (marker, conf, _) = SUPPLEMENT_MARKERS.iter().find(|(m, _, at_end)| {
        if *at_end {
            last == *m
        } else {
            tokens.contains(m)
        }
    })?;
    let mut rest: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|t| *t != *marker && *t != "official")
        .collect();
    if *marker == "special" {
        if let Some(at) = find_phrase(&rest, &["x", "mas"]) {
            rest.drain(at..at + 2);
        }
        rest.retain(|t| !OCCASION_WORDS.contains(t));
    }
    while rest
        .first()
        .is_some_and(|t| matches!(*t, "of" | "the" | "a" | "an"))
    {
        rest.remove(0);
    }
    while rest.last().is_some_and(|t| matches!(*t, "of" | "the")) {
        rest.pop();
    }
    let key = match_key(&rest.join(" "));
    (!key.is_empty()).then_some((marker, *conf, key))
}

/// Handbook / Guidebook / Sourcebook / Guide / Saga (recap) / Spotlight /
/// "Special" one-shots whose remaining title names another series of the
/// same publisher → `supplement_to` it. The parent volume whose years
/// contain the supplement's wins, else the nearest in time.
pub fn supplements(cat: &Catalogue) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in &cat.series {
        if annual_signal(s).is_some() || edition_marker(s).is_some() || is_facsimile(s) {
            continue;
        }
        let Some((marker, base_c, key)) = supplement_marker(s) else {
            continue;
        };
        let mut cands: Vec<(bool, i32, &Series)> = cat
            .with_key(&key)
            .filter(|p| p.id != s.id && supplement_marker(p).is_none())
            .filter(|p| same_publisher(s, p) != Some(false))
            .map(|p| {
                let within = matches!((s.start(), p.start(), p.end()), (Some(y), Some(a), Some(b)) if a <= y && y <= b);
                let gap = match (s.start(), p.start()) {
                    (Some(y), Some(a)) => (y - a).abs(),
                    _ => i32::MAX,
                };
                (within, gap, p)
            })
            .collect();
        if cands.is_empty() {
            continue;
        }
        cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.id.cmp(&b.2.id)));
        let (within, _, parent) = cands[0];
        let mut c = base_c;
        if cands.len() > 1 && !(within && cands.iter().filter(|c| c.0).count() == 1) {
            c -= 0.05;
        }
        if same_publisher(s, parent).is_none() {
            c -= 0.05;
        }
        out.push(Candidate {
            from: s.id,
            to: parent.id.into(),
            kind: RelationshipKind::SupplementTo,
            confidence: round2(c),
            source: EvidenceSource::Supplement,
            reason: format!(
                "{} looks like a {marker} for {} (shares its title{})",
                s.label(),
                parent.label(),
                if within {
                    ", published while it ran"
                } else {
                    ""
                }
            ),
            evidence: json!({
                "source": "supplement",
                "marker": marker,
                "within_parent_years": within,
                "candidates": cands.len(),
            }),
            scope: Scope::default(),
        });
    }
    out
}

// ───── translations → translation_of ─────

#[derive(Debug, FromQueryResult)]
struct ClaimRow {
    source: String,
    pid: String,
    series_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct CreatorRow {
    series_id: Uuid,
    people: Vec<String>,
}

/// Which of two series is the original: the earlier one. `None` when the
/// years don't tell.
fn original_of<'a>(a: &'a Series, b: &'a Series) -> Option<(&'a Series, &'a Series)> {
    match (a.start(), b.start()) {
        (Some(x), Some(y)) if x < y => Some((b, a)),
        (Some(x), Some(y)) if y < x => Some((a, b)),
        _ => None,
    }
}

fn translation(
    t: &Series,
    o: &Series,
    c: f32,
    why: String,
    evidence: serde_json::Value,
) -> Candidate {
    Candidate {
        from: t.id,
        to: o.id.into(),
        kind: RelationshipKind::TranslationOf,
        confidence: round2(c),
        source: EvidenceSource::Translation,
        reason: format!(
            "{} ({}) looks like a translation of {} ({}): {why}",
            t.label(),
            t.lang,
            o.label(),
            o.lang
        ),
        evidence,
        scope: Scope::default(),
    }
}

/// The same work in another language (`series.language_code`, folded to
/// ISO 639-1) → `translation_of` the original, from three kinds of
/// evidence:
///
/// - both series claim the **same provider series** (issue ComicInfo ids or
///   series `external_ids`) — 0.7;
/// - one lists the other's title as an **alias / alternate name** — 0.6;
///   the alias carrier is the translation unless the years say otherwise;
/// - the **same normalized title plus shared writers / pencillers** —
///   0.55, 0.6 with two or more.
///
/// Direction: the later series translates the earlier one; without years
/// to tell, only the alias evidence (carrier → named) gives a direction.
pub async fn translations<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
    cat: &Catalogue,
) -> Result<Vec<Candidate>, DbErr> {
    let idx: HashMap<Uuid, &Series> = cat.series.iter().map(|s| (s.id, s)).collect();
    let differs =
        |a: &Series, b: &Series| !a.lang.is_empty() && !b.lang.is_empty() && a.lang != b.lang;
    let mut out = Vec::new();

    // (a) shared provider series across languages.
    let claims = provider_claims_cte();
    let lang = lang_sql("s.language_code");
    let sql = format!(
        r#"
        {claims}
        ), multi AS (
            SELECT c.source, c.pid
              FROM claims c JOIN series s ON s.id = c.series_id
             GROUP BY 1, 2
            HAVING count(*) BETWEEN 2 AND {TRANSLATION_GROUP_MAX}
               AND count(DISTINCT {lang}) > 1
        )
        SELECT c.source, c.pid, c.series_id
          FROM claims c JOIN multi USING (source, pid)
         ORDER BY c.source, c.pid, c.series_id
        "#
    );
    let rows = ClaimRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    for group in rows.chunk_by(|a, b| a.source == b.source && a.pid == b.pid) {
        let members: Vec<&Series> = group
            .iter()
            .filter_map(|r| idx.get(&r.series_id).copied())
            .collect();
        // The original: the earliest member (unique).
        let Some(first) = members.iter().filter_map(|m| m.start()).min() else {
            continue;
        };
        let originals: Vec<&&Series> = members
            .iter()
            .filter(|m| m.start() == Some(first))
            .collect();
        if originals.len() != 1 {
            continue;
        }
        let o = *originals[0];
        for t in members.iter().filter(|t| t.id != o.id && differs(t, o)) {
            out.push(translation(
                t,
                o,
                0.7,
                format!("both match {} series {}", group[0].source, group[0].pid),
                json!({
                    "source": "translation",
                    "evidence": "provider_series",
                    "provider": group[0].source,
                    "provider_series_id": group[0].pid,
                }),
            ));
        }
    }

    // (b) one lists the other's title as an alias.
    for s in &cat.series {
        for alias in &s.aliases {
            for t in cat
                .with_norm(alias)
                .filter(|t| t.id != s.id && differs(s, t))
            {
                // `s` carries `t`'s title: `s` is the translation unless the
                // years say `t` came later.
                let (tr, orig) = match original_of(s, t) {
                    Some((tr, orig)) => (tr, orig),
                    None => (s, t),
                };
                out.push(translation(
                    tr,
                    orig,
                    0.6,
                    format!("{} lists \"{}\" as an alternate title", s.label(), t.name),
                    json!({
                        "source": "translation",
                        "evidence": "alias",
                        "alias": alias,
                    }),
                ));
            }
        }
    }

    // (c) same title, different language, shared creators.
    let mut pairs: Vec<(&Series, &Series)> = Vec::new();
    for members in cat.by_key.values() {
        if members.len() < 2 || members.len() > TRANSLATION_GROUP_MAX {
            continue;
        }
        for (i, &a) in members.iter().enumerate() {
            for &b in &members[i + 1..] {
                let (a, b) = (&cat.series[a], &cat.series[b]);
                if differs(a, b)
                    && let Some(p) = original_of(a, b)
                {
                    pairs.push(p);
                }
            }
        }
    }
    if !pairs.is_empty() {
        let ids: Vec<String> = pairs
            .iter()
            .flat_map(|(t, o)| [t.id, o.id])
            .collect::<HashSet<_>>()
            .into_iter()
            .map(|u| u.to_string())
            .collect();
        let rows = CreatorRow::find_by_statement(stmt(
            conn,
            "SELECT i.series_id, array_agg(DISTINCT lower(c.person)) AS people \
               FROM issue_credits c \
               JOIN issues i ON i.id = c.issue_id \
              WHERE i.series_id = ANY($1::text[]::uuid[]) AND i.removed_at IS NULL \
                AND c.role IN ('writer', 'penciller', 'artist') \
              GROUP BY 1",
            vec![Value::from(ids)],
        ))
        .all(conn)
        .await?;
        let people: HashMap<Uuid, HashSet<String>> = rows
            .into_iter()
            .map(|r| (r.series_id, r.people.into_iter().collect()))
            .collect();
        let empty = HashSet::new();
        for (t, o) in pairs {
            let shared: Vec<&String> = people
                .get(&t.id)
                .unwrap_or(&empty)
                .intersection(people.get(&o.id).unwrap_or(&empty))
                .collect();
            if shared.is_empty() {
                continue;
            }
            let mut sample: Vec<&str> = shared.iter().map(|s| s.as_str()).collect();
            sample.sort_unstable();
            sample.truncate(3);
            out.push(translation(
                t,
                o,
                if shared.len() >= 2 { 0.6 } else { 0.55 },
                format!(
                    "same title, {} shared creator{} ({})",
                    shared.len(),
                    if shared.len() == 1 { "" } else { "s" },
                    sample.join(", ")
                ),
                json!({
                    "source": "translation",
                    "evidence": "title_and_creators",
                    "shared_creators": shared.len(),
                }),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(name: &str) -> Series {
        let norm = entity::series::normalize_name(name);
        Series {
            id: Uuid::now_v7(),
            name: name.into(),
            key: match_key(&norm),
            norm,
            year: Some(2010),
            year_end: None,
            publisher: Some("marvel".into()),
            series_type: None,
            lang: "en".into(),
            aliases: Vec::new(),
            n_issues: 1,
            lo: Some(1.0),
            hi: Some(1.0),
            iy_min: Some(2010),
            iy_max: Some(2010),
            formats: Vec::new(),
            annual_issues: 0,
            first_number: Some("1".into()),
        }
    }

    #[test]
    fn annual_signals() {
        let s = series("The Amazing Spider-Man Annual (2018)");
        assert_eq!(
            annual_signal(&s).map(|x| x.1).as_deref(),
            Some("amazing spider man")
        );
        let mut f = series("Iron Man 1999");
        f.annual_issues = 1;
        assert_eq!(
            annual_signal(&f),
            Some((AnnualSignal::Format, "iron man".into()))
        );
        let mut t = series("Saga Specials");
        t.series_type = Some("Annual Series".into());
        assert_eq!(
            annual_signal(&t).map(|x| x.0),
            Some(AnnualSignal::SeriesType)
        );
        assert!(annual_signal(&series("Deadpool Bi-Annual")).is_none_or(|x| x.1 == "deadpool bi"));
        assert!(annual_signal(&series("Daredevil")).is_none());
    }

    #[test]
    fn edition_and_supplement_markers() {
        assert_eq!(
            edition_marker(&series("House of M Director's Cut")).map(|m| m.2),
            Some("house of m".into())
        );
        assert_eq!(
            edition_marker(&series("The Walking Dead Deluxe")).map(|m| m.2),
            Some("walking dead".into())
        );
        assert!(edition_marker(&series("Daredevil")).is_none());
        assert_eq!(
            supplement_marker(&series("Official Handbook of the Marvel Universe")).map(|m| m.2),
            Some("marvel universe".into())
        );
        assert_eq!(
            supplement_marker(&series("Fantastic Four: Wedding Special")).map(|m| m.2),
            Some("fantastic four".into())
        );
        assert_eq!(
            supplement_marker(&series("Ghost Rider X-Mas Special")).map(|m| m.2),
            Some("ghost rider".into())
        );
        assert!(supplement_marker(&series("Saga")).is_none());
        assert!(
            supplement_marker(&series("Saga of the Swamp Thing")).is_none(),
            "saga must end the name"
        );
    }
}
