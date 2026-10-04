//! Evidence sources for the suggestion engine. Each function runs a small
//! number of **set-based** queries scoped to one library and returns
//! [`Candidate`]s; none of them loops over series pairs in Rust.
//!
//! Every query is bounded twice: the SQL caps its own output
//! ([`SOURCE_ROW_LIMIT`]), and the pair-generating ones use a star (hub →
//! members) or adjacency (`lag()` over an ordered partition) shape instead
//! of all-pairs, so output grows linearly with the number of series.
//!
//! Confidence numbers are documented per source in
//! `docs/dev/series-relationships.md` ("Suggestion engine").

use super::citations;
use super::detectors;
use super::{Candidate, EvidenceSource, Target};
use crate::metadata::title_norm::{FormatClass, classify_format, infer_format_from_title};
use crate::relationships::{RelationshipCoverage, RelationshipKind, RelationshipQualifier, Scope};
use sea_orm::{ConnectionTrait, DbErr, FromQueryResult, Statement, Value};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

/// Upper bound on rows any single evidence query returns.
pub const SOURCE_ROW_LIMIT: i64 = 5000;
/// Candidate issues scanned for collected-edition citations (most are
/// annuals/specials that classify as non-collected and are skipped).
const COLLECTED_ROW_LIMIT: i64 = SOURCE_ROW_LIMIT * 4;

/// Rows the arc tie-in query may return (one per arc × series).
const ARC_ROW_LIMIT: i64 = SOURCE_ROW_LIMIT * 4;

/// SQL expression mirroring [`entity::series::normalize_name`]: lowercase,
/// keep alphanumerics, whitespace / `-` / `_` / `.` collapse to one space,
/// other punctuation is dropped.
pub(super) fn norm_sql(expr: &str) -> String {
    format!(
        "btrim(regexp_replace(regexp_replace(lower({expr}), '[^[:alnum:][:space:]._-]', '', 'g'), '[[:space:]._-]+', ' ', 'g'))"
    )
}

/// SQL expression: a normalized name minus trailing volume / year tokens
/// ("x men vol 2" → "x men", "silk 2015" → "silk"). Years are 1930–2049 so
/// "spider man 2099" keeps its number.
pub(super) fn base_sql(norm_expr: &str) -> String {
    format!(
        "coalesce(nullif(regexp_replace({norm_expr}, '( (vol|volume|v) ?[0-9]{{1,3}}| (19[3-9][0-9]|20[0-4][0-9]))+$', ''), ''), {norm_expr})"
    )
}

/// Rust twin of [`base_sql`] (used for the collected-edition fallback name).
pub fn base_name(norm: &str) -> String {
    let mut words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    loop {
        let n = words.len();
        if n >= 2 {
            let last = words[n - 1];
            let prev = words[n - 2];
            let is_year = last.len() == 4
                && last
                    .parse::<u32>()
                    .is_ok_and(|y| (1930..=2049).contains(&y));
            let is_vnum = last.strip_prefix('v').is_some_and(|d| {
                !d.is_empty() && d.len() <= 3 && d.chars().all(|c| c.is_ascii_digit())
            });
            let is_num = last.len() <= 3 && last.chars().all(|c| c.is_ascii_digit());
            if is_year || is_vnum {
                words.pop();
                continue;
            }
            if is_num && matches!(prev, "vol" | "volume" | "v") && n >= 3 {
                words.truncate(n - 2);
                continue;
            }
        }
        break;
    }
    if words.is_empty() {
        norm.to_owned()
    } else {
        words.join(" ")
    }
}

pub(super) fn stmt<C: ConnectionTrait>(conn: &C, sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(conn.get_database_backend(), sql, values)
}

pub(super) fn round2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

pub(super) fn label(name: &str, year: Option<i32>) -> String {
    match year {
        Some(y) => format!("{name} ({y})"),
        None => name.to_owned(),
    }
}

/// Compact issue-number intervals into a range string ("1-6,9"), merging
/// overlapping and adjacent ones (`next.lo <= prev.hi + 1`). Returns the
/// string and whether the merged set is one contiguous run. Falls back to
/// the overall `lo-hi` span (not contiguous) when the list would not fit
/// [`MAX_RANGE_LEN`](crate::relationships::MAX_RANGE_LEN). `None` for an
/// empty input.
pub fn compact_ranges(intervals: &[(f64, f64)]) -> Option<(String, bool)> {
    let mut v: Vec<(f64, f64)> = intervals
        .iter()
        .copied()
        .filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi)
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut runs: Vec<(f64, f64)> = Vec::new();
    for (lo, hi) in v {
        match runs.last_mut() {
            Some(last) if lo <= last.1 + 1.0 => last.1 = last.1.max(hi),
            _ => runs.push((lo, hi)),
        }
    }
    let fmt = |(lo, hi): (f64, f64)| {
        if lo == hi {
            fmt_num(lo)
        } else {
            format!("{}-{}", fmt_num(lo), fmt_num(hi))
        }
    };
    let contiguous = runs.len() == 1;
    let text = runs.iter().copied().map(fmt).collect::<Vec<_>>().join(",");
    if text.chars().count() <= crate::relationships::MAX_RANGE_LEN {
        return Some((text, contiguous));
    }
    let lo = runs.first().map_or(0.0, |r| r.0);
    let hi = runs.last().map_or(0.0, |r| r.1);
    Some((fmt((lo, hi)), false))
}

/// [`compact_ranges`] over single issue numbers.
pub fn compact_numbers(numbers: &[f64]) -> Option<(String, bool)> {
    compact_ranges(&numbers.iter().map(|n| (*n, *n)).collect::<Vec<_>>())
}

/// Continuation qualifier (WP-7.6) for "`later` continues `earlier`", or
/// `None` when the evidence doesn't say:
///
/// - `retitle`: the two share provider continuity under different names;
/// - `split`: a `series_provider_range` row links the two series
///   ([`range_link_sql`]) — a provider files part of one under a provider
///   series the other is matched to, or both map ranges into the same
///   provider series. A range of either series pointing somewhere unrelated
///   doesn't count;
/// - `numbering`: the later series' first number continues upward past the
///   earlier one's last (legacy numbering, not a restart);
/// - `relaunch`: the later series restarts at #1 (or #0) after the earlier
///   one's issues ended (its last issue year is not after the new start).
pub(super) fn continuation_qualifier(
    retitle: bool,
    split: bool,
    later_lo: Option<f64>,
    later_first_year: Option<i32>,
    earlier_hi: Option<f64>,
    earlier_last_year: Option<i32>,
) -> Option<RelationshipQualifier> {
    if retitle {
        return Some(RelationshipQualifier::Retitle);
    }
    if split {
        return Some(RelationshipQualifier::Split);
    }
    let (lo, hi) = (later_lo?, earlier_hi?);
    if lo > 1.0 && lo > hi {
        return Some(RelationshipQualifier::Numbering);
    }
    let ended = match (earlier_last_year, later_first_year) {
        (Some(e), Some(l)) => e <= l,
        _ => true,
    };
    (lo <= 1.0 && ended).then_some(RelationshipQualifier::Relaunch)
}

/// SQL predicate: a `series_provider_range` row **links** series `a` and
/// `b` (both SQL expressions yielding a series `uuid`) — the evidence the
/// `split` continuation qualifier needs:
///
/// - a range of one points at a provider series the other is matched to
///   (its series-level `external_ids` row, or — ComicVine / Metron — the
///   provider series id in one of its issues' ComicInfo), or
/// - both map ranges into the same provider series.
///
/// A range that points at some third provider series says nothing about
/// this pair. Only evaluated for adjacent pairs, so the per-pair index
/// probes stay cheap.
pub(super) fn range_link_sql(a: &str, b: &str) -> String {
    let matched = |owner: &str, p: &str| {
        format!(
            "(EXISTS (SELECT 1 FROM external_ids e \
                       WHERE e.entity_type = 'series' AND e.entity_id = ({owner})::text \
                         AND e.source = {p}.source AND e.external_id = {p}.provider_series_id) \
              OR EXISTS (SELECT 1 FROM issues i \
                          WHERE i.series_id = {owner} AND i.removed_at IS NULL \
                            AND (({p}.source = 'comicvine' AND i.comic_info_raw->>'comicvine_series_id' = {p}.provider_series_id) \
                              OR ({p}.source = 'metron' AND i.comic_info_raw->>'metron_series_id' = {p}.provider_series_id))))"
        )
    };
    let a_to_b = matched(b, "pa");
    let b_to_a = matched(a, "pb");
    format!(
        "(EXISTS (SELECT 1 FROM series_provider_range pa \
                   WHERE pa.series_id = {a} \
                     AND ({a_to_b} \
                          OR EXISTS (SELECT 1 FROM series_provider_range pq \
                                      WHERE pq.series_id = {b} AND pq.source = pa.source \
                                        AND pq.provider_series_id = pa.provider_series_id))) \
          OR EXISTS (SELECT 1 FROM series_provider_range pb \
                      WHERE pb.series_id = {b} AND {b_to_a}))"
    )
}

/// SQL: a language code folded to ISO 639-1 where we know the mapping
/// (`eng` / `EN` / `en-US` → `en`, `fre` / `fra` → `fr`, …). Mirrors
/// [`norm_lang`].
pub(super) fn lang_sql(expr: &str) -> String {
    let base = format!("lower(split_part(replace(btrim(coalesce({expr}, '')), '_', '-'), '-', 1))");
    let arms = LANG_3_TO_2
        .iter()
        .map(|(three, two)| format!("WHEN '{three}' THEN '{two}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("(CASE {base} {arms} ELSE {base} END)")
}

/// ISO 639-2 (bibliographic and terminology) → 639-1 for the languages
/// comics are commonly published in.
const LANG_3_TO_2: &[(&str, &str)] = &[
    ("eng", "en"),
    ("fre", "fr"),
    ("fra", "fr"),
    ("ger", "de"),
    ("deu", "de"),
    ("spa", "es"),
    ("ita", "it"),
    ("jpn", "ja"),
    ("por", "pt"),
    ("dut", "nl"),
    ("nld", "nl"),
    ("kor", "ko"),
    ("chi", "zh"),
    ("zho", "zh"),
    ("rus", "ru"),
    ("pol", "pl"),
    ("swe", "sv"),
    ("dan", "da"),
    ("nor", "no"),
    ("fin", "fi"),
    ("cze", "cs"),
    ("ces", "cs"),
    ("gre", "el"),
    ("ell", "el"),
    ("tur", "tr"),
    ("hun", "hu"),
    ("ara", "ar"),
    ("heb", "he"),
    ("ind", "id"),
    ("tha", "th"),
    ("vie", "vi"),
    ("ukr", "uk"),
    ("cat", "ca"),
];

/// Rust twin of [`lang_sql`]. Empty when unknown.
pub(super) fn norm_lang(raw: &str) -> String {
    let base = raw
        .trim()
        .replace('_', "-")
        .split('-')
        .next()
        .unwrap_or("")
        .to_lowercase();
    LANG_3_TO_2
        .iter()
        .find(|(three, _)| *three == base)
        .map_or(base, |(_, two)| (*two).to_owned())
}

// ───── AlternateSeries → crossover_with ─────

#[derive(Debug, FromQueryResult)]
struct AltRow {
    from_id: Uuid,
    from_name: String,
    from_year: Option<i32>,
    to_id: Uuid,
    to_name: String,
    to_year: Option<i32>,
    n_issues: i64,
    alt_raw: String,
    issue_year: Option<i32>,
    reading_list_style: bool,
    n_candidates: i64,
}

/// ComicInfo `AlternateSeries` on issues of A naming series B.
///
/// The field is comma/semicolon separated. ComicVine-style taggers write
/// reading-list entries as `"Avengers" Civil War` — the quoted part is the
/// series family, the tail the event; those are weaker evidence than a
/// plain `AlternateSeries` value naming a series outright. When several
/// series share the cited name, the one closest in year to the citing
/// issues wins. An alternate naming A's own title is ignored (that's the
/// same run, not a crossover).
pub async fn alternate_series<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let alt_norm = norm_sql("alt_raw");
    let sql = format!(
        r#"
        WITH alt AS (
            SELECT i.series_id AS from_id, i.year AS iyear,
                   btrim(coalesce(substring(p FROM '^\s*"([^"]+)"'), p)) AS alt_raw,
                   (p ~ '^\s*"[^"]+"\s*\S') AS rl
              FROM issues i
             CROSS JOIN LATERAL regexp_split_to_table(i.alternate_series, '\s*[,;]\s*') AS p
             WHERE i.library_id = $1
               AND i.removed_at IS NULL
               AND i.alternate_series IS NOT NULL
        ), agg AS (
            SELECT from_id, {alt_norm} AS alt_norm, min(alt_raw) AS alt_raw,
                   count(*) AS n_issues, max(iyear) AS issue_year, bool_and(rl) AS reading_list_style
              FROM alt
             WHERE alt_raw <> ''
             GROUP BY 1, 2
        )
        SELECT DISTINCT ON (a.from_id, a.alt_norm)
               a.from_id, f.name AS from_name, f.year AS from_year,
               t.id AS to_id, t.name AS to_name, t.year AS to_year,
               a.n_issues, a.alt_raw, a.issue_year, a.reading_list_style,
               count(*) OVER (PARTITION BY a.from_id, a.alt_norm) AS n_candidates
          FROM agg a
          JOIN series f ON f.id = a.from_id AND f.removed_at IS NULL
          JOIN series t ON t.library_id = $1 AND t.removed_at IS NULL
                       AND t.normalized_name = a.alt_norm AND t.id <> a.from_id
         WHERE a.alt_norm <> '' AND a.alt_norm <> f.normalized_name
         ORDER BY a.from_id, a.alt_norm, (t.year IS NULL),
                  abs(coalesce(t.year, 0) - coalesce(a.issue_year, t.year, 0)), t.id
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = AltRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut c: f32 = if r.reading_list_style { 0.5 } else { 0.75 };
            if r.n_issues >= 3 {
                c += 0.05;
            }
            if r.n_candidates > 1 {
                c -= 0.05;
            }
            let year_gap = match (r.to_year, r.issue_year) {
                (Some(a), Some(b)) => (a - b).abs(),
                _ => 0,
            };
            if year_gap > 3 {
                c -= 0.15;
            }
            let c = round2(c.clamp(0.05, 0.95));
            Candidate {
                from: r.from_id,
                to: r.to_id.into(),
                kind: RelationshipKind::CrossoverWith,
                confidence: c,
                source: EvidenceSource::AlternateSeries,
                scope: Scope::default(),
                reason: format!(
                    "{} of {} list \"{}\" as an alternate series ({})",
                    plural(r.n_issues, "issue", "issues"),
                    label(&r.from_name, r.from_year),
                    r.alt_raw,
                    label(&r.to_name, r.to_year)
                ),
                evidence: json!({
                    "source": "alternate_series",
                    "issues": r.n_issues,
                    "alternate_series": r.alt_raw,
                    "reading_list_style": r.reading_list_style,
                    "same_name_candidates": r.n_candidates,
                    "issue_year": r.issue_year,
                    "to_year": r.to_year,
                }),
            }
        })
        .collect())
}

// ───── story arcs → tie_in_to (series → arc), WP-7.6 ─────

#[derive(Debug, FromQueryResult)]
struct ArcSeriesRow {
    arc_id: Uuid,
    arc_name: String,
    series_id: Uuid,
    series_name: String,
    series_year: Option<i32>,
    series_norm: String,
    /// Issues of this series in the arc.
    n: i64,
    /// Their numbers (for the edge's `from_range`).
    numbers: Option<Vec<f64>>,
    /// First / last cover month of those issues (`year * 12 + month`).
    first_ym: Option<i32>,
    last_ym: Option<i32>,
    /// An issue title in the arc says Prelude / Road to …
    t_prelude: bool,
    /// An issue title in the arc says Aftermath / Epilogue.
    t_after: bool,
}

/// How sure the detector is about an arc's main series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainBy {
    /// The series is named like the arc ("Secret Wars" ← `"Secret Wars"
    /// Battleworld`).
    Name,
    /// Holds at least twice as many of the arc's issues as the runner-up.
    Margin,
    /// Two series hold (nearly) as many issues each: both are main, and
    /// they get a `crossover_with` pair (a genuine two-title crossover).
    CoMain,
    /// Holds the most issues, without a clear margin.
    Weak,
}

/// Normalized name key for arc / series comparison: base name, minus a
/// leading "the".
pub(super) fn match_key(norm: &str) -> String {
    let b = base_name(norm);
    b.strip_prefix("the ").map_or(b.clone(), str::to_owned)
}

/// The keys a main series may be named by, **in priority order**: the
/// arc's own name, then — for ComicVine reading-list style names
/// (`"Secret Wars" Battleworld`) — the quoted family ("Secret Wars").
/// WP-8.2: the tail alone ("Battleworld") no longer names a main series.
fn arc_keys(name: &str) -> Vec<String> {
    let mut keys = vec![match_key(&entity::series::normalize_name(name))];
    let t = name.trim_start();
    if let Some(rest) = t.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        keys.push(match_key(&entity::series::normalize_name(&rest[..end])));
    }
    keys.retain(|k| !k.is_empty());
    keys.dedup();
    keys
}

/// WP-8.2 main-series rule, step 1: the series named exactly like the arc
/// (its [`match_key`] equals the arc's), else one named like the arc's
/// reading-list family; among several of the same name, the one with the
/// most issues in the arc (`group` is sorted by issue count). `None` →
/// step 2, the most issues in the arc ([`MainBy::Margin`] /
/// [`MainBy::CoMain`] / [`MainBy::Weak`]).
fn main_by_name(group: &[ArcSeriesRow], keys: &[String]) -> Option<usize> {
    keys.iter()
        .find_map(|k| group.iter().position(|r| match_key(&r.series_norm) == *k))
}

/// Series-name markers of a prelude / aftermath.
fn name_role(norm: &str) -> Option<RelationshipQualifier> {
    let padded = format!(" {norm} ");
    if padded.contains(" prelude ") || padded.contains(" road to ") {
        Some(RelationshipQualifier::Prelude)
    } else if padded.contains(" aftermath ") || padded.contains(" epilogue ") {
        Some(RelationshipQualifier::Aftermath)
    } else {
        None
    }
}

/// Story arcs (`issue_arcs`) spanning ≥ 2 series of the library. Every
/// participating series gets `tie_in_to` **the arc** (an arc target, so an
/// event with 60 tie-ins is 60 rows, never pairs) with a role:
///
/// - `main`: the series named like the arc (or its reading-list family),
///   else the one holding the most of the arc's issues;
/// - `prelude` / `aftermath`: an issue title or the series name says
///   Prelude / Road to / Aftermath / Epilogue, or all of the series' arc
///   issues are cover-dated before (after) the main series' arc span;
/// - `tie_in`: everything else.
///
/// Confidence follows how unambiguous the main series is ([`MainBy`]). Two
/// co-main series also get a `crossover_with` pair (WP-7.6 keeps
/// `crossover_with` only from that and from ComicInfo `AlternateSeries`).
/// One query; the per-arc role assignment is a linear pass over its
/// arc-ordered rows.
pub async fn arc_tie_ins<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        WITH per AS (
            SELECT ia.arc_id, i.series_id, count(*) AS n,
                   array_agg(DISTINCT i.sort_number) FILTER (WHERE i.sort_number IS NOT NULL) AS numbers,
                   min(i.year * 12 + coalesce(i.month, 1)) AS first_ym,
                   max(i.year * 12 + coalesce(i.month, 12)) AS last_ym,
                   bool_or(coalesce(i.title, '') ~* '(prelude|road to)') AS t_prelude,
                   bool_or(coalesce(i.title, '') ~* '(aftermath|epilogue)') AS t_after
              FROM issue_arcs ia
              JOIN issues i ON i.id = ia.issue_id
             WHERE i.library_id = $1 AND i.removed_at IS NULL
             GROUP BY 1, 2
        ), live AS (
            SELECT p.* FROM per p
              JOIN series s ON s.id = p.series_id AND s.removed_at IS NULL
        ), multi AS (
            SELECT arc_id FROM live GROUP BY arc_id HAVING count(*) >= 2
        )
        SELECT p.arc_id, a.name AS arc_name, p.series_id, s.name AS series_name,
               s.year AS series_year, s.normalized_name AS series_norm, p.n, p.numbers,
               p.first_ym, p.last_ym, p.t_prelude, p.t_after
          FROM live p
          JOIN multi USING (arc_id)
          JOIN story_arc a ON a.id = p.arc_id
          JOIN series s ON s.id = p.series_id
         ORDER BY p.arc_id, p.n DESC, s.year NULLS LAST, s.id
         LIMIT {ARC_ROW_LIMIT}
        "#
    );
    let rows = ArcSeriesRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    let mut out = Vec::new();
    for group in rows.chunk_by(|a, b| a.arc_id == b.arc_id) {
        if group.len() >= 2 {
            arc_group(group, &mut out);
        }
    }
    Ok(out)
}

/// Roles and confidences for one arc's series (sorted by issue count desc).
fn arc_group(group: &[ArcSeriesRow], out: &mut Vec<Candidate>) {
    let arc = &group[0];
    let keys = arc_keys(&arc.arc_name);
    let by_name = main_by_name(group, &keys);
    let (mains, by): (Vec<usize>, MainBy) = match by_name {
        Some(i) => (vec![i], MainBy::Name),
        None => {
            let (top, second) = (group[0].n, group[1].n);
            let third = group.get(2).map_or(0, |r| r.n);
            if top >= 2 * second {
                (vec![0], MainBy::Margin)
            } else if second >= 2 && 4 * second >= 3 * top && 2 * third <= second {
                (vec![0, 1], MainBy::CoMain)
            } else {
                (vec![0], MainBy::Weak)
            }
        }
    };
    // The main story's cover-date span (both mains for a co-main arc).
    let span_lo = mains.iter().filter_map(|&i| group[i].first_ym).min();
    let span_hi = mains.iter().filter_map(|&i| group[i].last_ym).max();
    let main_names = mains
        .iter()
        .map(|&i| label(&group[i].series_name, group[i].series_year))
        .collect::<Vec<_>>()
        .join(" and ");
    let why_main = match by {
        MainBy::Name => "named after the arc".to_owned(),
        MainBy::Margin => "holds most of the arc's issues".to_owned(),
        MainBy::CoMain => "shares the arc's issues evenly with another series".to_owned(),
        MainBy::Weak => "holds the most of the arc's issues, without a clear margin".to_owned(),
    };
    for (i, r) in group.iter().enumerate() {
        let is_main = mains.contains(&i);
        let flag_role = if r.t_prelude {
            Some(RelationshipQualifier::Prelude)
        } else if r.t_after {
            Some(RelationshipQualifier::Aftermath)
        } else {
            name_role(&r.series_norm)
        };
        let date_role = match (r.first_ym, r.last_ym, span_lo, span_hi) {
            (_, Some(last), Some(lo), _) if last < lo => Some(RelationshipQualifier::Prelude),
            (Some(first), _, _, Some(hi)) if first > hi => Some(RelationshipQualifier::Aftermath),
            _ => None,
        };
        let (role, c, why): (RelationshipQualifier, f32, String) = if is_main {
            let c = match by {
                MainBy::Name => 0.9,
                MainBy::Margin => 0.75,
                MainBy::CoMain => 0.6,
                MainBy::Weak => 0.55,
            };
            (
                RelationshipQualifier::Main,
                c,
                format!("main story: {why_main}"),
            )
        } else {
            let clear = matches!(by, MainBy::Name | MainBy::Margin);
            let base: f32 = match by {
                MainBy::Name => 0.8,
                MainBy::Margin => 0.7,
                MainBy::CoMain | MainBy::Weak => 0.55,
            };
            match (flag_role, date_role) {
                (Some(q), _) => (
                    q,
                    base,
                    format!("{} (title says so); main story {main_names}", role_word(q)),
                ),
                (None, Some(q)) => (
                    q,
                    if clear { base - 0.05 } else { base },
                    format!(
                        "{}: its arc issues are cover-dated {} {main_names}'s",
                        role_word(q),
                        if q == RelationshipQualifier::Prelude {
                            "before"
                        } else {
                            "after"
                        }
                    ),
                ),
                (None, None) => (
                    RelationshipQualifier::TieIn,
                    base,
                    format!("tie-in; main story {main_names}"),
                ),
            }
        };
        let from_range = r
            .numbers
            .as_deref()
            .and_then(compact_numbers)
            .map(|(t, _)| t);
        out.push(Candidate {
            from: r.series_id,
            to: Target::Arc(r.arc_id),
            kind: RelationshipKind::TieInTo,
            confidence: round2(c),
            source: EvidenceSource::ArcTieIn,
            reason: format!(
                "{} has {} in the story arc \"{}\" — {}",
                label(&r.series_name, r.series_year),
                plural(r.n, "issue", "issues"),
                arc.arc_name,
                why
            ),
            evidence: json!({
                "source": "arc_tie_in",
                "arc_name": arc.arc_name,
                "role": role.as_str(),
                "main_by": format!("{by:?}").to_lowercase(),
                "issues_in_arc": r.n,
                "arc_series": group.len(),
            }),
            scope: Scope {
                from_range,
                qualifier: Some(role),
                ..Scope::default()
            },
        });
    }
    if by == MainBy::CoMain {
        let (a, b) = (&group[mains[0]], &group[mains[1]]);
        out.push(Candidate {
            from: a.series_id,
            to: b.series_id.into(),
            kind: RelationshipKind::CrossoverWith,
            confidence: 0.65,
            source: EvidenceSource::ArcCrossover,
            reason: format!(
                "{} and {} are both main titles of the story arc \"{}\" ({} and {} of its issues)",
                label(&a.series_name, a.series_year),
                label(&b.series_name, b.series_year),
                arc.arc_name,
                a.n,
                b.n
            ),
            evidence: json!({
                "source": "arc_crossover",
                "arc_name": arc.arc_name,
                "issues_in_from": a.n,
                "issues_in_to": b.n,
            }),
            scope: Scope::default(),
        });
    }
}

fn role_word(q: RelationshipQualifier) -> &'static str {
    match q {
        RelationshipQualifier::Prelude => "prelude",
        RelationshipQualifier::Aftermath => "aftermath",
        RelationshipQualifier::Main => "main story",
        _ => "tie-in",
    }
}

// ───── name continuation → continues (WP-7.5: publication continuity) ─────

#[derive(Debug, FromQueryResult)]
struct NameRow {
    id: Uuid,
    name: String,
    year: Option<i32>,
    ord: Option<i32>,
    prev_id: Uuid,
    prev_name: String,
    prev_year: Option<i32>,
    prev_ord: Option<i32>,
    vol_folder: bool,
    lo: Option<f64>,
    iy_min: Option<i32>,
    /// A provider range links the pair ([`range_link_sql`]).
    split: bool,
    prev_hi: Option<f64>,
    prev_iy_max: Option<i32>,
}

/// Same title, later year or volume ("X (2011)" → "X (2016)", or a
/// `Series/Vol 2` folder after `Series/Vol 1`). Series are grouped by
/// `(base name, publisher)` — the base name is the normalized name minus a
/// trailing volume/year token, or the parent folder's name when the series
/// folder itself is just `Vol N` — then each one is paired with its
/// immediate predecessor by `(year, volume)` (`lag()`), so a run of N
/// volumes yields N − 1 suggestions.
pub async fn name_continuation<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let leaf = r"substring(s.folder_path FROM '([^/\\]+)[/\\]*$')";
    let parent = r"substring(s.folder_path FROM '([^/\\]+)[/\\]+[^/\\]+[/\\]*$')";
    let parent_base = base_sql(&norm_sql("parent"));
    let name_base = base_sql("normalized_name");
    let vol_leaf = r"lower(coalesce(leaf, '')) ~ '^(vol(ume)?\.?|v) ?[0-9]{1,3}$'";
    let lang = lang_sql("s.language_code");
    let split = range_link_sql("o.id", "o.prev_id");
    let sql = format!(
        r#"
        WITH s AS (
            SELECT s.id, s.name, s.year, s.volume, s.normalized_name,
                   lower(coalesce(s.publisher, '')) AS pub, {lang} AS lang,
                   {leaf} AS leaf, {parent} AS parent,
                   r.lo, r.hi, r.iy_min, r.iy_max
              FROM series s
              -- Per-series index lookup (robust to missing statistics).
              CROSS JOIN LATERAL (
                  SELECT min(i.sort_number) AS lo, max(i.sort_number) AS hi,
                         min(i.year) AS iy_min, max(i.year) AS iy_max
                    FROM issues i
                   WHERE i.series_id = s.id AND i.removed_at IS NULL
              ) r
             WHERE s.library_id = $1 AND s.removed_at IS NULL
        ), k AS (
            SELECT id, name, year, pub, lang, lo, hi, iy_min, iy_max,
                   ({vol_leaf} AND parent IS NOT NULL) AS vol_folder,
                   CASE WHEN {vol_leaf} AND parent IS NOT NULL THEN {parent_base}
                        ELSE {name_base} END AS base,
                   coalesce(volume,
                            nullif(substring(lower(coalesce(leaf, ''))
                                   FROM '(?:^|\s)v(?:ol(?:ume)?)?\.? ?([0-9]{{1,2}})(?:\s|\(|$)'), '')::int) AS ord
              FROM s
        ), o AS (
            SELECT k.*,
                   lag(id)   OVER w AS prev_id,
                   lag(name) OVER w AS prev_name,
                   lag(year) OVER w AS prev_year,
                   lag(ord)  OVER w AS prev_ord,
                   lag(hi)   OVER w AS prev_hi,
                   lag(iy_max) OVER w AS prev_iy_max
              FROM k
             WHERE base <> '' AND (year IS NOT NULL OR ord IS NOT NULL)
            -- Language too (WP-7.6): the same title in another language is
            -- a translation, not the next volume.
            WINDOW w AS (PARTITION BY base, pub, lang ORDER BY year NULLS LAST, ord NULLS LAST, id)
        )
        SELECT id, name, year, ord, prev_id, prev_name, prev_year, prev_ord, vol_folder,
               lo, iy_min, {split} AS split, prev_hi, prev_iy_max
          FROM o
         WHERE prev_id IS NOT NULL
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = NameRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let this = label(&r.name, r.year);
        let prev = label(&r.prev_name, r.prev_year);
        let vol = |name: &str, v: i32, year: Option<i32>| label(&format!("{name} vol. {v}"), year);
        let (kind, c, why) = match (r.prev_ord, r.ord, r.prev_year, r.year) {
            (Some(a), Some(b), _, _) if b == a + 1 => (
                RelationshipKind::Continues,
                0.9,
                format!(
                    "{} follows {} — same title, next volume",
                    vol(&r.name, b, r.year),
                    vol(&r.prev_name, a, r.prev_year)
                ),
            ),
            (Some(a), Some(b), _, _) if b > a => (
                RelationshipKind::Continues,
                0.65,
                format!(
                    "{} follows {} — same title; the volumes in between aren't in the library",
                    vol(&r.name, b, r.year),
                    vol(&r.prev_name, a, r.prev_year)
                ),
            ),
            (_, _, Some(py), Some(y)) if y > py => (
                RelationshipKind::Continues,
                if r.prev_ord.is_some() && r.ord.is_some() {
                    // Year order and volume order disagree.
                    0.45
                } else {
                    0.7
                },
                format!(
                    "{this} continues {prev} — same title, {} later",
                    plural(i64::from(y - py), "year", "years")
                ),
            ),
            (_, _, Some(py), Some(y)) if y == py => (
                RelationshipKind::SeeAlso,
                0.35,
                format!("{this} and {prev} have the same title and year"),
            ),
            _ => continue,
        };
        let qualifier = (kind == RelationshipKind::Continues)
            .then(|| {
                continuation_qualifier(
                    false,
                    r.split,
                    r.lo,
                    r.iy_min.or(r.year),
                    r.prev_hi,
                    r.prev_iy_max.or(r.prev_year),
                )
            })
            .flatten();
        out.push(Candidate {
            from: r.id,
            to: r.prev_id.into(),
            kind,
            confidence: c,
            source: EvidenceSource::NameContinuation,
            reason: why,
            evidence: json!({
                "source": "name_continuation",
                "from_year": r.year,
                "to_year": r.prev_year,
                "from_volume": r.ord,
                "to_volume": r.prev_ord,
                "volume_folder": r.vol_folder,
                "from_first_issue": r.lo,
                "to_last_issue": r.prev_hi,
                "qualifier": qualifier.map(RelationshipQualifier::as_str),
            }),
            scope: Scope {
                qualifier,
                ..Scope::default()
            },
        });
    }
    Ok(out)
}

// ───── collected editions → collects ─────

#[derive(Debug, FromQueryResult)]
struct CollectedRow {
    series_id: Uuid,
    series_name: String,
    series_norm: String,
    series_type: Option<String>,
    format: Option<String>,
    special_type: Option<String>,
    title: Option<String>,
    notes: Option<String>,
    summary: Option<String>,
    sort_number: Option<f64>,
}

#[derive(Debug, FromQueryResult)]
struct ResolvedCitation {
    idx: i32,
    to_id: Uuid,
    to_name: String,
    to_year: Option<i32>,
    covered: i64,
    n_candidates: i64,
}

fn is_collected(r: &CollectedRow) -> bool {
    let formats: Vec<String> = r.format.iter().chain(&r.special_type).cloned().collect();
    collected_by(&r.series_name, r.series_type.as_deref(), &formats)
}

/// The collected edition's own title minus format words, as the fallback
/// target name for "Collects #1-6".
fn fallback_name(norm: &str) -> String {
    const FORMAT_WORDS: &[&str] = &[
        "tpb",
        "tp",
        "hc",
        "ogn",
        "omnibus",
        "hardcover",
        "compendium",
        "trade",
        "paperback",
        "deluxe",
        "edition",
        "library",
        "collection",
        "graphic",
        "novel",
    ];
    let kept: Vec<&str> = norm
        .split(' ')
        .filter(|w| !w.is_empty() && !FORMAT_WORDS.contains(w))
        .collect();
    base_name(&kept.join(" "))
}

/// Collected editions (`Format` / `special_type` / series type / a title
/// marker like "TPB" or "Omnibus") of this library whose notes, title or
/// "Collects …" summary cite issue ranges of another series. The citation
/// parsing is in [`citations`]; resolution is one set-based query against
/// **every** library (WP-8.2: `collects` may cross libraries; a target in
/// the edition's own library wins a coverage tie).
pub async fn collected_editions<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    // The series-level marker (type or a format word in the name) is
    // evaluated once per series in a MATERIALIZED CTE. Written as one OR in
    // the join filter, Postgres ran the regex once per *issue* row: 50,000
    // times at stress scale, ~390 ms of the whole ~900 ms run (WP-8.3).
    let sql = format!(
        r#"
        WITH s AS MATERIALIZED (
            SELECT id, name, normalized_name, series_type,
                   (series_type IS NOT NULL
                    OR normalized_name ~ '(^| )(tpb|tp|hc|ogn|omnibus|hardcover|compendium)( |$)|trade paperback|graphic novel|collected edition|deluxe edition|library edition')
                       AS marked
              FROM series
             WHERE removed_at IS NULL
        )
        SELECT i.series_id, s.name AS series_name, s.normalized_name AS series_norm,
               s.series_type, i.format, i.special_type, i.title, i.notes, i.summary, i.sort_number
          FROM issues i
          JOIN s ON s.id = i.series_id
         WHERE i.library_id = $1 AND i.removed_at IS NULL
           AND (i.format IS NOT NULL OR i.special_type IS NOT NULL OR s.marked)
         LIMIT {COLLECTED_ROW_LIMIT}
        "#
    );
    let rows = CollectedRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;

    // (from series, cited name, lo, hi) → whether the name was explicit.
    struct Cite {
        from: Uuid,
        from_name: String,
        /// The citing issue's own number (→ the edge's `from_range`).
        from_number: Option<f64>,
        name: String,
        explicit: bool,
        lo: f64,
        hi: f64,
    }
    let mut cites: Vec<Cite> = Vec::new();
    let mut seen: std::collections::HashSet<(Uuid, String, i64, i64)> =
        std::collections::HashSet::new();
    for r in rows.iter().filter(|r| is_collected(r)) {
        let mut texts: Vec<&str> = Vec::new();
        texts.extend(r.title.as_deref());
        texts.extend(r.notes.as_deref());
        if let Some(sum) = r.summary.as_deref()
            && (sum.to_lowercase().contains("collect") || sum.to_lowercase().contains("reprint"))
        {
            texts.push(sum);
        }
        for text in texts {
            for c in citations::parse(text) {
                let explicit = !c.name_norm.is_empty();
                let name = if explicit {
                    c.name_norm
                } else {
                    fallback_name(&r.series_norm)
                };
                if name.is_empty() || name == r.series_norm {
                    continue;
                }
                let key = (r.series_id, name.clone(), c.lo as i64, c.hi as i64);
                if seen.insert(key) {
                    cites.push(Cite {
                        from: r.series_id,
                        from_name: r.series_name.clone(),
                        from_number: r.sort_number,
                        name,
                        explicit,
                        lo: c.lo,
                        hi: c.hi,
                    });
                }
            }
        }
        if cites.len() >= SOURCE_ROW_LIMIT as usize {
            break;
        }
    }
    if cites.is_empty() {
        return Ok(Vec::new());
    }

    let base = base_sql("normalized_name");
    let sql = format!(
        r#"
        WITH c AS (
            SELECT * FROM unnest($2::int[], $3::text[]::uuid[], $4::text[], $5::float8[], $6::float8[])
                       AS c(idx, from_id, name_norm, lo, hi)
        ), t AS (
            -- WP-8.2: `collects` may cross libraries (trades are often
            -- filed in a library of their own), so every live series is a
            -- target; the edition's own library wins a tie.
            SELECT id, name, year, normalized_name, {base} AS base,
                   (library_id <> $1) AS elsewhere
              FROM series WHERE removed_at IS NULL
        ), m AS (
            SELECT c.idx, c.lo, c.hi, t.id, t.name, t.year, t.elsewhere
              FROM c JOIN t ON t.normalized_name = c.name_norm AND t.id <> c.from_id
            UNION
            SELECT c.idx, c.lo, c.hi, t.id, t.name, t.year, t.elsewhere
              FROM c JOIN t ON t.base = c.name_norm AND t.id <> c.from_id
        ), scored AS (
            SELECT m.idx, m.id AS to_id, m.name AS to_name, m.year AS to_year, m.elsewhere,
                   (SELECT count(*) FROM issues i
                     WHERE i.series_id = m.id AND i.removed_at IS NULL
                       AND i.sort_number BETWEEN m.lo AND m.hi) AS covered,
                   count(*) OVER (PARTITION BY m.idx) AS n_candidates
              FROM m
        )
        SELECT DISTINCT ON (idx) idx, to_id, to_name, to_year, covered, n_candidates
          FROM scored
         ORDER BY idx, covered DESC, elsewhere, to_year NULLS LAST, to_id
        "#
    );
    let idx: Vec<i32> = (0..cites.len() as i32).collect();
    let from: Vec<String> = cites.iter().map(|c| c.from.to_string()).collect();
    let names: Vec<String> = cites.iter().map(|c| c.name.clone()).collect();
    let lo: Vec<f64> = cites.iter().map(|c| c.lo).collect();
    let hi: Vec<f64> = cites.iter().map(|c| c.hi).collect();
    let resolved = ResolvedCitation::find_by_statement(stmt(
        conn,
        &sql,
        vec![
            library_id.into(),
            idx.into(),
            from.into(),
            names.into(),
            lo.into(),
            hi.into(),
        ],
    ))
    .all(conn)
    .await?;

    // Score each resolved citation, then roll them up per (edition, target)
    // pair: an edition series citing "#1-6" in one issue and "#7-12" in the
    // next is one suggestion with `to_range = "1-12"`.
    struct Scored<'a> {
        cite: &'a Cite,
        to_name: String,
        to_year: Option<i32>,
        covered: i64,
        n_candidates: i64,
        confidence: f32,
    }
    let mut by_pair: HashMap<(Uuid, Uuid), Vec<Scored>> = HashMap::new();
    for r in resolved {
        let Some(cite) = usize::try_from(r.idx).ok().and_then(|i| cites.get(i)) else {
            continue;
        };
        let span = (cite.hi - cite.lo).floor() as i64 + 1;
        let ratio = r.covered as f64 / span.max(1) as f64;
        let mut c: f32 = if ratio >= 0.8 {
            0.85
        } else if ratio >= 0.5 {
            0.7
        } else if r.covered >= 1 {
            0.55
        } else {
            0.4
        };
        if !cite.explicit {
            c -= 0.1;
        }
        if r.n_candidates > 1 && r.covered == 0 {
            c -= 0.1;
        }
        by_pair
            .entry((cite.from, r.to_id))
            .or_default()
            .push(Scored {
                cite,
                to_name: r.to_name,
                to_year: r.to_year,
                covered: r.covered,
                n_candidates: r.n_candidates,
                confidence: round2(c.max(0.05)),
            });
    }
    let mut out = Vec::with_capacity(by_pair.len());
    for ((from, to), mut parts) in by_pair {
        parts.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.cite
                        .lo
                        .partial_cmp(&b.cite.lo)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        let best = &parts[0];
        let ranges: Vec<(f64, f64)> = parts.iter().map(|p| (p.cite.lo, p.cite.hi)).collect();
        // Coverage: the cited ranges, merged, leave no gap → `full` (the
        // edition says it collects every issue in its span); a gap →
        // `partial`.
        let (to_range, contiguous) = compact_ranges(&ranges).unwrap_or_default();
        let from_numbers: Vec<f64> = parts.iter().filter_map(|p| p.cite.from_number).collect();
        let from_range = compact_numbers(&from_numbers).map(|(t, _)| t);
        let shown = to_range.replace('-', "–");
        out.push(Candidate {
            from,
            to: to.into(),
            kind: RelationshipKind::Collects,
            confidence: best.confidence,
            source: EvidenceSource::CollectedEdition,
            reason: format!(
                "{} is a collected edition citing {} #{} ({} of the best-covered citation's issues are in the library)",
                best.cite.from_name,
                label(&best.to_name, best.to_year),
                shown,
                best.covered
            ),
            evidence: json!({
                "source": "collected_edition",
                "cited_name": best.cite.name,
                "name_explicit": best.cite.explicit,
                "range_low": best.cite.lo,
                "range_high": best.cite.hi,
                "issues_in_library": best.covered,
                "same_name_candidates": best.n_candidates,
                "cited_ranges": ranges
                    .iter()
                    .map(|(lo, hi)| format!("{}-{}", fmt_num(*lo), fmt_num(*hi)))
                    .collect::<Vec<_>>(),
            }),
            scope: Scope {
                from_range,
                to_range: (!to_range.is_empty()).then_some(to_range),
                coverage: Some(if contiguous {
                    RelationshipCoverage::Full
                } else {
                    RelationshipCoverage::Partial
                }),
                ..Scope::default()
            },
        });
    }
    Ok(out)
}

pub(super) fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

pub(super) fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ───── issue_reprints roll-up → collects / reprints (WP-7.6) ─────

/// A series is a collected edition (TPB / HC / omnibus / graphic novel) by
/// its `series_type`, any of its issues' `Format` / `special_type`, or a
/// name marker ("… TPB", "… Omnibus").
pub(super) fn collected_by(name: &str, series_type: Option<&str>, formats: &[String]) -> bool {
    series_type.and_then(classify_format) == Some(FormatClass::Collected)
        || formats
            .iter()
            .any(|f| classify_format(f) == Some(FormatClass::Collected))
        || matches!(
            infer_format_from_title(name, None),
            Some("Omnibus" | "Hardcover" | "Graphic Novel" | "TPB")
        )
}

#[derive(Debug, FromQueryResult)]
struct ReprintRow {
    from_id: Uuid,
    from_name: String,
    from_year: Option<i32>,
    from_series_type: Option<String>,
    from_formats: Option<Vec<String>>,
    to_id: Uuid,
    to_name: String,
    to_year: Option<i32>,
    /// Distinct reprinted issues of `to`.
    linked: i64,
    /// … of which numbered (`sort_number` set).
    linked_numbered: i64,
    from_numbers: Option<Vec<f64>>,
    to_numbers: Option<Vec<f64>>,
    /// Issues of `to` in the library within the reprinted span.
    in_span: i64,
}

/// `issue_reprints` (issue of this library → reprinted issue, in any
/// library since WP-8.2) rolled up per series pair. The reprinting side is the subject: `collects` when
/// it is a collected edition ([`collected_by`]), else `reprints`. `to_range`
/// is the reprinted numbers compacted ("1-6,9"), `from_range` the
/// reprinting issues'; `coverage` is `full` when every issue of the target
/// the library holds inside the reprinted span is linked, `partial` when
/// some are not, `unknown` without issue numbers. Label-only rows
/// (`reprinted_issue_id` NULL) are skipped. One grouped query.
pub async fn reprint_rollup<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        WITH r AS (
            SELECT i.series_id AS from_id, t.series_id AS to_id, t.id AS tid,
                   i.sort_number AS fnum, t.sort_number AS tnum
              FROM issue_reprints rp
              JOIN issues i ON i.id = rp.issue_id AND i.library_id = $1 AND i.removed_at IS NULL
              -- WP-8.2: the reprinted issue may be in any library.
              JOIN issues t ON t.id = rp.reprinted_issue_id AND t.removed_at IS NULL
             WHERE t.series_id <> i.series_id
        ), agg AS (
            SELECT from_id, to_id,
                   count(DISTINCT tid) AS linked,
                   count(DISTINCT tid) FILTER (WHERE tnum IS NOT NULL) AS linked_numbered,
                   array_agg(DISTINCT fnum) FILTER (WHERE fnum IS NOT NULL) AS from_numbers,
                   array_agg(DISTINCT tnum) FILTER (WHERE tnum IS NOT NULL) AS to_numbers,
                   min(tnum) AS lo, max(tnum) AS hi
              FROM r GROUP BY 1, 2
        )
        SELECT a.from_id, f.name AS from_name, f.year AS from_year,
               f.series_type AS from_series_type,
               (SELECT array_agg(DISTINCT x.v) FROM (
                    SELECT i.format AS v FROM issues i
                     WHERE i.series_id = a.from_id AND i.removed_at IS NULL AND i.format IS NOT NULL
                    UNION
                    SELECT i.special_type FROM issues i
                     WHERE i.series_id = a.from_id AND i.removed_at IS NULL
                       AND i.special_type IS NOT NULL) x) AS from_formats,
               a.to_id, t.name AS to_name, t.year AS to_year,
               a.linked, a.linked_numbered, a.from_numbers, a.to_numbers,
               (SELECT count(*) FROM issues x
                 WHERE x.series_id = a.to_id AND x.removed_at IS NULL
                   AND x.sort_number BETWEEN a.lo AND a.hi) AS in_span
          FROM agg a
          JOIN series f ON f.id = a.from_id AND f.removed_at IS NULL
          JOIN series t ON t.id = a.to_id AND t.removed_at IS NULL
         ORDER BY a.linked DESC
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = ReprintRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let formats = r.from_formats.clone().unwrap_or_default();
            let collected = collected_by(&r.from_name, r.from_series_type.as_deref(), &formats);
            let (kind, c) = if collected {
                (
                    RelationshipKind::Collects,
                    if r.linked >= 3 { 0.9 } else { 0.85 },
                )
            } else {
                (
                    RelationshipKind::Reprints,
                    if r.linked >= 2 { 0.7 } else { 0.65 },
                )
            };
            let to = r.to_numbers.as_deref().and_then(compact_numbers);
            let from_range = r
                .from_numbers
                .as_deref()
                .and_then(compact_numbers)
                .map(|(t, _)| t);
            let coverage = match &to {
                None => RelationshipCoverage::Unknown,
                Some(_) if r.linked_numbered >= r.in_span => RelationshipCoverage::Full,
                Some(_) => RelationshipCoverage::Partial,
            };
            let to_range = to.map(|(t, _)| t);
            Candidate {
                from: r.from_id,
                to: r.to_id.into(),
                kind,
                confidence: c,
                source: EvidenceSource::ReprintRollup,
                reason: format!(
                    "{} reprints {} of {}{}{}",
                    label(&r.from_name, r.from_year),
                    plural(r.linked, "issue", "issues"),
                    label(&r.to_name, r.to_year),
                    to_range
                        .as_deref()
                        .map(|t| format!(" (#{t})"))
                        .unwrap_or_default(),
                    if collected {
                        " as a collected edition"
                    } else {
                        ""
                    }
                ),
                evidence: json!({
                    "source": "reprint_rollup",
                    "linked_issues": r.linked,
                    "issues_in_span": r.in_span,
                    "collected_edition": collected,
                    "coverage": coverage.as_str(),
                }),
                scope: Scope {
                    from_range,
                    to_range,
                    coverage: Some(coverage),
                    ..Scope::default()
                },
            }
        })
        .collect())
}

/// SQL CTEs `rep` + `claims(series_id, source, pid)`: every provider series a
/// local series of library `$1` claims — its first issue's ComicInfo
/// `comicvine_series_id` / `metron_series_id`, plus series-level
/// `external_ids` (ComicVine / Metron / GCD). Starts with `WITH` and ends
/// **inside** `claims`: the caller closes it with `)` and adds its own CTEs.
pub(super) fn provider_claims_cte() -> &'static str {
    r#"
        WITH rep AS (
            SELECT s.id AS series_id, r.cv, r.metron
              FROM series s
              CROSS JOIN LATERAL (
                  SELECT i.comic_info_raw->>'comicvine_series_id' AS cv,
                         i.comic_info_raw->>'metron_series_id' AS metron
                    FROM issues i
                   WHERE i.series_id = s.id AND i.removed_at IS NULL
                   ORDER BY i.sort_number NULLS LAST, i.id
                   LIMIT 1
              ) r
             WHERE s.library_id = $1 AND s.removed_at IS NULL
        ), claims AS (
            SELECT series_id, 'comicvine' AS source, cv AS pid FROM rep WHERE cv IS NOT NULL AND cv <> ''
            UNION
            SELECT series_id, 'metron', metron FROM rep WHERE metron IS NOT NULL AND metron <> ''
            UNION
            SELECT s.id, e.source, e.external_id
              FROM external_ids e
              JOIN series s ON s.id::text = e.entity_id
             WHERE e.entity_type = 'series' AND e.source IN ('comicvine', 'metron', 'gcd')
               AND s.library_id = $1 AND s.removed_at IS NULL
"#
}

// ───── provider volume ids → continues / see_also ─────

#[derive(Debug, FromQueryResult)]
struct ProviderRow {
    source: String,
    pid: String,
    id: Uuid,
    name: String,
    year: Option<i32>,
    lo: Option<f64>,
    hi: Option<f64>,
    prev_id: Uuid,
    prev_name: String,
    prev_year: Option<i32>,
    prev_lo: Option<f64>,
    prev_hi: Option<f64>,
    members: i64,
    iy_min: Option<i32>,
    prev_iy_max: Option<i32>,
    /// The two claimants' base names differ (a retitle).
    retitled: bool,
    /// A provider range links the pair ([`range_link_sql`]).
    split: bool,
}

/// Local series that claim the **same provider volume** — one run split
/// across several local series (folder per year, a relaunch filed
/// separately, …). The series-level `external_ids` row is unique per
/// provider id, so a second series can only claim it through its issues'
/// ComicInfo (`comicvine_series_id` / `metron_series_id`). One
/// representative issue per series (the first by `sort_number`, an
/// index-only pick) is read, plus the series-level ids.
///
/// Members of a shared id are ordered by first issue number; adjacent
/// members whose issue ranges are disjoint and ordered are `continues`
/// (the provider sees one continuous run); overlapping ranges are
/// `see_also` (likely duplicates or variant files of the same run).
pub async fn provider_volumes<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let claims = provider_claims_cte();
    let lang = lang_sql("s.language_code");
    let base = base_sql("s.normalized_name");
    let split = range_link_sql("o.id", "o.prev_id");
    let sql = format!(
        r#"
        {claims}
        ), shared AS (
            SELECT source, pid, count(*) AS members FROM claims
             GROUP BY 1, 2 HAVING count(*) BETWEEN 2 AND 12
        ), rng AS (
            SELECT i.series_id, min(i.sort_number) AS lo, max(i.sort_number) AS hi,
                   min(i.year) AS iy_min, max(i.year) AS iy_max
              FROM issues i
             WHERE i.series_id IN (SELECT c.series_id FROM claims c JOIN shared USING (source, pid))
               AND i.removed_at IS NULL AND i.sort_number IS NOT NULL
             GROUP BY 1
        ), o AS (
            -- Partitioned by language too: two claimants in different
            -- languages are a translation (WP-7.6 translation detector), not
            -- one run continuing.
            SELECT c.source, c.pid, sh.members, s.id, s.name, s.year, r.lo, r.hi, r.iy_min,
                   {base} AS base,
                   lag(s.id)   OVER w AS prev_id,
                   lag(s.name) OVER w AS prev_name,
                   lag(s.year) OVER w AS prev_year,
                   lag(r.lo)   OVER w AS prev_lo,
                   lag(r.hi)   OVER w AS prev_hi,
                   lag(r.iy_max) OVER w AS prev_iy_max,
                   lag({base}) OVER w AS prev_base
              FROM claims c
              JOIN shared sh USING (source, pid)
              JOIN series s ON s.id = c.series_id
              LEFT JOIN rng r ON r.series_id = c.series_id
            WINDOW w AS (PARTITION BY c.source, c.pid, {lang}
                         ORDER BY r.lo NULLS LAST, s.year NULLS LAST, s.id)
        )
        SELECT source, pid, id, name, year, lo, hi, prev_id, prev_name, prev_year, prev_lo, prev_hi,
               members, iy_min, prev_iy_max, (base IS DISTINCT FROM prev_base) AS retitled,
               {split} AS split
          FROM o WHERE prev_id IS NOT NULL
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = ProviderRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let provider = provider_label(&r.source);
            let this = label(&r.name, r.year);
            let prev = label(&r.prev_name, r.prev_year);
            let disjoint = matches!((r.prev_hi, r.lo), (Some(ph), Some(lo)) if ph < lo);
            let (kind, c, why) = if disjoint {
                (
                    RelationshipKind::Continues,
                    0.8,
                    format!(
                        "{this} and {prev} both match {provider} series {}; issues {} continue {}",
                        r.pid,
                        range_label(r.lo, r.hi),
                        range_label(r.prev_lo, r.prev_hi)
                    ),
                )
            } else {
                (
                    RelationshipKind::SeeAlso,
                    if r.lo.is_some() && r.prev_lo.is_some() {
                        0.6
                    } else {
                        0.55
                    },
                    format!(
                        "{this} and {prev} both match {provider} series {} with overlapping issue numbers (possible duplicate run)",
                        r.pid
                    ),
                )
            };
            let qualifier = (kind == RelationshipKind::Continues)
                .then(|| {
                    continuation_qualifier(
                        r.retitled,
                        r.split,
                        r.lo,
                        r.iy_min.or(r.year),
                        r.prev_hi,
                        r.prev_iy_max.or(r.prev_year),
                    )
                })
                .flatten();
            Candidate {
                from: r.id,
                to: r.prev_id.into(),
                kind,
                confidence: c,
                source: EvidenceSource::ProviderVolume,
                reason: why,
                evidence: json!({
                    "source": "provider_volume",
                    "provider": r.source,
                    "provider_series_id": r.pid,
                    "members": r.members,
                    "from_range": [r.lo, r.hi],
                    "to_range": [r.prev_lo, r.prev_hi],
                    "qualifier": qualifier.map(RelationshipQualifier::as_str),
                }),
                scope: Scope {
                    qualifier,
                    ..Scope::default()
                },
            }
        })
        .collect())
}

fn provider_label(source: &str) -> &str {
    match source {
        "comicvine" => "ComicVine",
        "metron" => "Metron",
        "gcd" => "GCD",
        other => other,
    }
}

fn range_label(lo: Option<f64>, hi: Option<f64>) -> String {
    match (lo, hi) {
        (Some(l), Some(h)) if l == h => format!("#{}", fmt_num(l)),
        (Some(l), Some(h)) => format!("#{}–{}", fmt_num(l), fmt_num(h)),
        _ => "(unnumbered)".into(),
    }
}

// ───── series_provider_range → see_also ─────

#[derive(Debug, FromQueryResult)]
struct RangeRow {
    a_id: Uuid,
    a_name: String,
    a_year: Option<i32>,
    b_id: Uuid,
    b_name: String,
    b_year: Option<i32>,
    source: String,
    provider_series_id: String,
    provider_series_name: Option<String>,
    range_low: Option<String>,
    range_high: Option<String>,
}

/// A `series_provider_range` row says "the provider files issues lo–hi of
/// local series A under provider series P". When another local series B is
/// itself matched to P, A's range and B are the same run seen two ways:
/// suggest `see_also`. Not `continues`: the range sits *inside* A (A isn't
/// read entirely before or after B), and B usually duplicates those issues
/// rather than continuing them.
pub async fn provider_ranges<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        SELECT a.id AS a_id, a.name AS a_name, a.year AS a_year,
               b.id AS b_id, b.name AS b_name, b.year AS b_year,
               r.source, r.provider_series_id, r.provider_series_name, r.range_low, r.range_high
          FROM series_provider_range r
          JOIN series a ON a.id = r.series_id AND a.library_id = $1 AND a.removed_at IS NULL
          JOIN external_ids e ON e.entity_type = 'series' AND e.source = r.source
                             AND e.external_id = r.provider_series_id
          JOIN series b ON b.id::text = e.entity_id AND b.library_id = $1
                       AND b.removed_at IS NULL AND b.id <> a.id
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = RangeRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let range = match (&r.range_low, &r.range_high) {
                (Some(l), Some(h)) => format!("#{l}–{h}"),
                (Some(l), None) => format!("#{l} onwards"),
                (None, Some(h)) => format!("up to #{h}"),
                (None, None) => "some issues".into(),
            };
            Candidate {
                from: r.a_id,
                to: r.b_id.into(),
                kind: RelationshipKind::SeeAlso,
                confidence: 0.7,
                source: EvidenceSource::ProviderRange,
                scope: Scope::default(),
                reason: format!(
                    "{} files {} {} under its series {}{}, which {} is matched to",
                    provider_label(&r.source),
                    label(&r.a_name, r.a_year),
                    range,
                    r.provider_series_id,
                    r.provider_series_name
                        .as_deref()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default(),
                    label(&r.b_name, r.b_year)
                ),
                evidence: json!({
                    "source": "provider_range",
                    "provider": r.source,
                    "provider_series_id": r.provider_series_id,
                    "range_low": r.range_low,
                    "range_high": r.range_high,
                }),
            }
        })
        .collect())
}

// ───── provider links (Metron `associated`) → see_also / collects / annual_of (WP-7.8) ─────

#[derive(Debug, FromQueryResult)]
struct AssocSeries {
    id: Uuid,
    library_id: Uuid,
    name: String,
    year: Option<i32>,
    series_type: Option<String>,
}

/// Provider links between two series of the library: a live
/// (`set_by = 'provider'`, not dismissed) `series_external_relationship`
/// row of A whose provider series resolves to local series B
/// ([`crate::relationships::external::resolve_where`]: direct
/// `external_ids` match or the cached id bridge). Metron's `associated` is
/// untyped, so the kind comes from the two **local** series types / names
/// ([`crate::relationships::external::provider_kind`]): `collects` (edition
/// → singles) or `annual_of` when the types say so, else `see_also`.
///
/// WP-8.2: a `collects` link may pair two libraries (a trade library and a
/// singles library), so rows of **every** library are resolved and a pair
/// is kept when either end is in this library; the engine's ownership
/// filter then keeps the rows whose canonical `from` is here and drops a
/// cross-library pair of any non-edition kind. Resolving every library's
/// rows keeps this symmetric: whichever library owns a pair, its run sees
/// it. Provider rows are bounded by curation (a few per applied series).
pub async fn provider_associated<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    use crate::relationships::external;
    let resolved = external::resolve_where(
        conn,
        "e.set_by = 'provider' AND e.dismissed_at IS NULL AND e.from_series_id IN \
           (SELECT id FROM series WHERE removed_at IS NULL)",
        vec![],
    )
    .await?;
    if resolved.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = resolved.iter().map(|r| r.ext_id).collect();
    let rows: HashMap<Uuid, entity::series_external_relationship::Model> = {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        entity::series_external_relationship::Entity::find()
            .filter(entity::series_external_relationship::Column::Id.is_in(ids))
            .all(conn)
            .await?
            .into_iter()
            .map(|r| (r.id, r))
            .collect()
    };
    let mut series_ids: Vec<Uuid> = resolved.iter().map(|r| r.series_id).collect();
    series_ids.extend(rows.values().map(|r| r.from_series_id));
    let series: HashMap<Uuid, AssocSeries> = AssocSeries::find_by_statement(stmt(
        conn,
        "SELECT id, library_id, name, year, series_type FROM series \
          WHERE id = ANY($1) AND removed_at IS NULL",
        vec![series_ids.into()],
    ))
    .all(conn)
    .await?
    .into_iter()
    .map(|s| (s.id, s))
    .collect();
    let mut out = Vec::new();
    for r in resolved {
        if out.len() >= usize::try_from(SOURCE_ROW_LIMIT).unwrap_or(5000) {
            break;
        }
        let (Some(row), Some(b)) = (rows.get(&r.ext_id), series.get(&r.series_id)) else {
            continue;
        };
        let Some(a) = series.get(&row.from_series_id) else {
            continue;
        };
        if a.library_id != library_id && b.library_id != library_id {
            continue;
        }
        let (mut kind, mut confidence) = external::provider_kind(
            a.series_type.as_deref(),
            &a.name,
            b.series_type.as_deref(),
            &b.name,
        );
        // The local rows say nothing (types not applied, no name marker):
        // fall back to the kind recorded at apply time from the provider's
        // own series types.
        if kind == RelationshipKind::SeeAlso
            && let Ok(stored) = row.kind.parse::<RelationshipKind>()
            && stored != RelationshipKind::SeeAlso
        {
            kind = stored;
            confidence = row
                .confidence
                .unwrap_or(external::ASSOCIATED_TYPED_ONE_SIDE);
        }
        let provider = provider_label(&row.source).to_owned();
        let ids: Vec<serde_json::Value> = row
            .evidence
            .get("ids")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(|| vec![json!(row.provider_series_id)]);
        let how = match kind {
            RelationshipKind::Collects => {
                format!(" ({} is a collected edition)", label(&a.name, a.year))
            }
            RelationshipKind::CollectedIn => {
                format!(" ({} is a collected edition)", label(&b.name, b.year))
            }
            RelationshipKind::AnnualOf => format!(" ({} is an annual)", label(&a.name, a.year)),
            RelationshipKind::HasAnnual => format!(" ({} is an annual)", label(&b.name, b.year)),
            _ => String::new(),
        };
        out.push(Candidate {
            from: a.id,
            to: b.id.into(),
            kind,
            confidence,
            source: EvidenceSource::ProviderAssociated,
            reason: format!(
                "{provider} lists {} and {} as associated series{how}",
                label(&a.name, a.year),
                label(&b.name, b.year)
            ),
            evidence: json!({
                "source": "provider_associated",
                "provider": row.source,
                "field": "associated",
                "ids": ids,
                "provider_series_id": row.provider_series_id,
                "provider_series_name": row.provider_series_name,
            }),
            scope: Scope::default(),
        });
    }
    Ok(out)
}

/// Every source, in a fixed order, with per-source counts for the run
/// report. A failing source is logged and skipped so one bad query can't
/// sink the rest; its name comes back in the third element so the caller
/// can tell "no candidates" from "couldn't look" (stale marking is skipped
/// for the latter).
pub async fn collect_all<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> (
    Vec<Candidate>,
    HashMap<&'static str, usize>,
    Vec<&'static str>,
) {
    let mut all = Vec::new();
    let mut counts = HashMap::new();
    let mut failed = Vec::new();
    macro_rules! run {
        ($name:literal, $f:expr) => {{
            let started = std::time::Instant::now();
            match $f.await {
                Ok(v) => {
                    tracing::debug!(
                        library_id = %library_id,
                        source = $name,
                        candidates = v.len(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "relationship suggestions: source done"
                    );
                    counts.insert($name, v.len());
                    all.extend(v);
                }
                Err(e) => {
                    tracing::warn!(library_id = %library_id, source = $name, error = %e,
                        "relationship suggestions: source failed");
                    counts.insert($name, 0);
                    failed.push($name);
                }
            }
        }};
    }
    run!("alternate_series", alternate_series(conn, library_id));
    run!("arc_tie_in", arc_tie_ins(conn, library_id));
    run!("name_continuation", name_continuation(conn, library_id));
    run!("collected_edition", collected_editions(conn, library_id));
    run!("reprint_rollup", reprint_rollup(conn, library_id));
    run!("provider_volume", provider_volumes(conn, library_id));
    run!("provider_range", provider_ranges(conn, library_id));
    run!("provider_associated", provider_associated(conn, library_id));
    // Name-based detectors share one catalogue query (WP-7.6). WP-8.2: the
    // edition detectors also see the other libraries' series that could
    // pair with this library's (story / publication kinds stay local).
    match detectors::Catalogue::load(conn, library_id).await {
        Ok(cat) => {
            run!("annual", async { Ok::<_, DbErr>(detectors::annuals(&cat)) });
            run!("supplement", async {
                Ok::<_, DbErr>(detectors::supplements(&cat))
            });
            match cat.with_other_libraries(conn, library_id).await {
                Ok(wide) => {
                    run!("alternate_edition", async {
                        Ok::<_, DbErr>(detectors::alternate_editions(&wide))
                    });
                    run!("facsimile", async {
                        Ok::<_, DbErr>(detectors::facsimiles(&wide))
                    });
                    run!(
                        "translation",
                        detectors::translations(conn, library_id, &wide)
                    );
                }
                Err(e) => {
                    tracing::warn!(library_id = %library_id, error = %e,
                        "relationship suggestions: cross-library catalogue failed");
                    for name in ["alternate_edition", "facsimile", "translation"] {
                        counts.insert(name, 0);
                        failed.push(name);
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(library_id = %library_id, error = %e,
                "relationship suggestions: series catalogue failed");
            for name in [
                "annual",
                "alternate_edition",
                "facsimile",
                "supplement",
                "translation",
            ] {
                counts.insert(name, 0);
                failed.push(name);
            }
        }
    }
    // An annual is not the next volume of its main series: drop the
    // continuation candidates on a pair the annual detector claims.
    let annual_pairs: std::collections::HashSet<(Uuid, Target)> = all
        .iter()
        .filter(|c: &&Candidate| c.kind == RelationshipKind::AnnualOf)
        .flat_map(|c| {
            [
                (c.from, c.to),
                (c.to.series().unwrap_or(c.from), Target::Series(c.from)),
            ]
        })
        .collect();
    if !annual_pairs.is_empty() {
        all.retain(|c: &Candidate| {
            !(matches!(
                c.kind,
                RelationshipKind::Continues | RelationshipKind::SeeAlso
            ) && matches!(
                c.source,
                EvidenceSource::NameContinuation
                    | EvidenceSource::ProviderVolume
                    | EvidenceSource::ProviderAssociated
            ) && annual_pairs.contains(&(c.from, c.to)))
        });
    }
    // WP-7.8: an untyped provider link that stayed `see_also` adds nothing
    // when another source already says what the pair is (collects,
    // continues, …): drop it rather than queue a second, vaguer row.
    let typed_pairs: std::collections::HashSet<(Uuid, Uuid)> = all
        .iter()
        .filter(|c| c.kind != RelationshipKind::SeeAlso)
        .filter_map(|c| {
            let to = c.to.series()?;
            Some(if c.from < to {
                (c.from, to)
            } else {
                (to, c.from)
            })
        })
        .collect();
    all.retain(|c: &Candidate| {
        if c.source != EvidenceSource::ProviderAssociated || c.kind != RelationshipKind::SeeAlso {
            return true;
        }
        let Some(to) = c.to.series() else {
            return true;
        };
        let key = if c.from < to {
            (c.from, to)
        } else {
            (to, c.from)
        };
        !typed_pairs.contains(&key)
    });
    (all, counts, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_name_strips_trailing_volume_and_year() {
        assert_eq!(base_name("x men vol 2"), "x men");
        assert_eq!(base_name("silk 2015"), "silk");
        assert_eq!(base_name("saga v3"), "saga");
        assert_eq!(base_name("spider man 2099"), "spider man 2099");
        assert_eq!(base_name("1602"), "1602");
        assert_eq!(base_name("saga"), "saga");
    }

    #[test]
    fn compact_ranges_merges_adjacent_runs() {
        assert_eq!(
            compact_numbers(&[3.0, 1.0, 2.0, 4.0, 5.0, 6.0, 9.0]),
            Some(("1-6,9".into(), false))
        );
        assert_eq!(compact_numbers(&[7.0]), Some(("7".into(), true)));
        assert_eq!(
            compact_ranges(&[(1.0, 6.0), (7.0, 12.0)]),
            Some(("1-12".into(), true))
        );
        assert_eq!(compact_numbers(&[]), None);
        // Too long for a range column: falls back to the span.
        let many: Vec<f64> = (0..60).map(|n| f64::from(n * 2)).collect();
        assert_eq!(compact_numbers(&many), Some(("0-118".into(), false)));
    }

    #[test]
    fn continuation_qualifier_rules() {
        use RelationshipQualifier as Q;
        let q = continuation_qualifier;
        assert_eq!(
            q(true, true, Some(1.0), None, Some(5.0), None),
            Some(Q::Retitle)
        );
        assert_eq!(
            q(false, true, Some(1.0), None, Some(5.0), None),
            Some(Q::Split)
        );
        assert_eq!(
            q(false, false, Some(600.0), None, Some(12.0), None),
            Some(Q::Numbering)
        );
        assert_eq!(
            q(false, false, Some(1.0), Some(2016), Some(22.0), Some(2015)),
            Some(Q::Relaunch)
        );
        assert_eq!(
            q(false, false, Some(1.0), Some(2014), Some(22.0), Some(2016)),
            None,
            "the earlier run hadn't ended"
        );
        assert_eq!(q(false, false, None, None, Some(5.0), None), None);
        assert_eq!(norm_lang("eng"), "en");
        assert_eq!(norm_lang("en-US"), "en");
        assert_eq!(norm_lang("FRE"), "fr");
    }

    fn arc_row(arc: &str, series: &str, year: i32, n: i64) -> ArcSeriesRow {
        ArcSeriesRow {
            arc_id: Uuid::from_u128(7),
            arc_name: arc.to_owned(),
            series_id: Uuid::now_v7(),
            series_name: series.to_owned(),
            series_year: Some(year),
            series_norm: entity::series::normalize_name(series),
            n,
            numbers: None,
            first_ym: None,
            last_ym: None,
            t_prelude: false,
            t_after: false,
        }
    }

    fn main_of(group: &[ArcSeriesRow]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        arc_group(group, &mut out);
        out.iter()
            .filter(|c| c.scope.qualifier == Some(RelationshipQualifier::Main))
            .map(|c| {
                let name = group
                    .iter()
                    .find(|r| r.series_id == c.from)
                    .map(|r| r.series_name.clone())
                    .unwrap_or_default();
                (
                    name,
                    c.evidence["main_by"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn arc_main_series_rule_name_branch() {
        // WP-8.2 step 1: the series named exactly like the arc wins, even
        // over the reading-list family's series and over more issues.
        let arc = "\"Secret Wars\" Battleworld";
        let group = vec![
            arc_row(arc, "Secret Wars", 2015, 9),
            arc_row(arc, "Planet Hulk", 2015, 5),
            arc_row(arc, "Secret Wars: Battleworld", 2015, 4),
        ];
        assert_eq!(
            main_of(&group),
            vec![("Secret Wars: Battleworld".to_owned(), "name".to_owned())]
        );
        // No exact match: the family's series wins ("Secret Wars").
        let group = vec![
            arc_row(arc, "Planet Hulk", 2015, 5),
            arc_row(arc, "Secret Wars", 2015, 4),
            arc_row(arc, "Battleworld", 2015, 3),
        ];
        assert_eq!(
            main_of(&group),
            vec![("Secret Wars".to_owned(), "name".to_owned())],
            "the tail alone (\"Battleworld\") doesn't name the main series"
        );
        // Several volumes of the arc's name: the one with more arc issues.
        let group = vec![
            arc_row("Secret Wars", "Secret Wars", 2015, 9),
            arc_row("Secret Wars", "Secret Wars", 1984, 2),
            arc_row("Secret Wars", "Thors", 2015, 4),
        ];
        let m = main_of(&group);
        assert_eq!(m.len(), 1);
        assert_eq!(group[0].series_year, Some(2015));
        assert_eq!(m[0].1, "name");
    }

    #[test]
    fn arc_main_series_rule_most_issues_branch() {
        // WP-8.2 step 2: nothing named like the arc — most issues wins.
        let group = vec![
            arc_row("Dark Reign", "Dark Avengers", 2009, 8),
            arc_row("Dark Reign", "Thunderbolts", 2009, 3),
            arc_row("Dark Reign", "Secret Warriors", 2009, 1),
        ];
        assert_eq!(
            main_of(&group),
            vec![("Dark Avengers".to_owned(), "margin".to_owned())]
        );
        let group = vec![
            arc_row("Dark Reign", "Dark Avengers", 2009, 5),
            arc_row("Dark Reign", "Thunderbolts", 2009, 3),
            arc_row("Dark Reign", "Secret Warriors", 2009, 3),
        ];
        assert_eq!(
            main_of(&group),
            vec![("Dark Avengers".to_owned(), "weak".to_owned())]
        );
    }

    #[test]
    fn fallback_name_drops_format_words() {
        assert_eq!(fallback_name("saga vol 1 tpb"), "saga");
        assert_eq!(fallback_name("saga deluxe edition hc"), "saga");
    }

    #[test]
    fn norm_sql_mentions_input() {
        assert!(norm_sql("x").contains("lower(x)"));
        assert!(base_sql("n").starts_with("coalesce(nullif(regexp_replace(n"));
    }
}
