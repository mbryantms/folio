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
use super::{Candidate, EvidenceSource};
use crate::metadata::title_norm::{FormatClass, classify_format, infer_format_from_title};
use crate::relationships::RelationshipKind;
use sea_orm::{ConnectionTrait, DbErr, FromQueryResult, Statement, Value};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

/// Upper bound on rows any single evidence query returns.
pub const SOURCE_ROW_LIMIT: i64 = 5000;
/// Candidate issues scanned for collected-edition citations (most are
/// annuals/specials that classify as non-collected and are skipped).
const COLLECTED_ROW_LIMIT: i64 = SOURCE_ROW_LIMIT * 4;

/// Series-group size above which `same_universe` confidence drops a bucket
/// (a group like "Marvel" spanning hundreds of series is weak evidence and
/// would flood the hub's page if bulk-accepted).
const GROUP_SMALL: i64 = 12;
const GROUP_LARGE: i64 = 40;

/// Character/team density: a feature counts only when it appears in at
/// most `DENSITY_DF_FRACTION` of the library's series that carry any
/// character/team data, clamped to `DENSITY_MIN_DF..=DENSITY_MAX_DF`
/// (rare = discriminating; "Captain America" in 60 series says nothing).
/// Relative, because junction coverage varies wildly between libraries.
const DENSITY_DF_FRACTION: f64 = 0.02;
const DENSITY_MIN_DF: i64 = 3;
const DENSITY_MAX_DF: i64 = 25;
/// A story arc spanning more series than this is an event; links through
/// it are capped at low confidence.
const ARC_EVENT_SIZE: i64 = 10;
/// Minimum shared rare features for a density pair.
const DENSITY_MIN_SHARED: i64 = 5;
/// Minimum overlap coefficient (shared / smaller set).
const DENSITY_MIN_OVERLAP: f64 = 0.5;
/// At most this many density suggestions per series (each side).
const DENSITY_PER_SERIES: i64 = 3;

/// SQL expression mirroring [`entity::series::normalize_name`]: lowercase,
/// keep alphanumerics, whitespace / `-` / `_` / `.` collapse to one space,
/// other punctuation is dropped.
fn norm_sql(expr: &str) -> String {
    format!(
        "btrim(regexp_replace(regexp_replace(lower({expr}), '[^[:alnum:][:space:]._-]', '', 'g'), '[[:space:]._-]+', ' ', 'g'))"
    )
}

/// SQL expression: a normalized name minus trailing volume / year tokens
/// ("x men vol 2" → "x men", "silk 2015" → "silk"). Years are 1930–2049 so
/// "spider man 2099" keeps its number.
fn base_sql(norm_expr: &str) -> String {
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

fn stmt<C: ConnectionTrait>(conn: &C, sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(conn.get_database_backend(), sql, values)
}

fn round2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

fn label(name: &str, year: Option<i32>) -> String {
    match year {
        Some(y) => format!("{name} ({y})"),
        None => name.to_owned(),
    }
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
                to: r.to_id,
                kind: RelationshipKind::CrossoverWith,
                confidence: c,
                source: EvidenceSource::AlternateSeries,
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

// ───── SeriesGroup → same_universe ─────

#[derive(Debug, FromQueryResult)]
struct GroupRow {
    hub_id: Uuid,
    hub_name: String,
    hub_year: Option<i32>,
    other_id: Uuid,
    other_name: String,
    other_year: Option<i32>,
    grp: String,
    grp_n: i64,
}

/// Series sharing a ComicInfo `SeriesGroup`. Star-shaped: every member links
/// to one hub (the member whose name equals the group, else the oldest), so
/// a group of N yields N − 1 suggestions, not N².
pub async fn series_group<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let gnorm = norm_sql("s.series_group");
    let sql = format!(
        r#"
        WITH g AS (
            SELECT s.id, s.name, s.year, s.series_group, s.normalized_name, {gnorm} AS gnorm
              FROM series s
             WHERE s.library_id = $1 AND s.removed_at IS NULL
               AND s.series_group IS NOT NULL AND btrim(s.series_group) <> ''
        ), r AS (
            SELECT g.*,
                   row_number() OVER (PARTITION BY gnorm
                                      ORDER BY (normalized_name = gnorm) DESC, year NULLS LAST, id) AS rk,
                   count(*) OVER (PARTITION BY gnorm) AS grp_n
              FROM g WHERE gnorm <> ''
        )
        SELECT h.id AS hub_id, h.name AS hub_name, h.year AS hub_year,
               o.id AS other_id, o.name AS other_name, o.year AS other_year,
               o.series_group AS grp, o.grp_n
          FROM r h
          JOIN r o ON o.gnorm = h.gnorm AND o.rk > 1
         WHERE h.rk = 1 AND o.normalized_name <> h.normalized_name
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = GroupRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let c = if r.grp_n <= GROUP_SMALL {
                0.85
            } else if r.grp_n <= GROUP_LARGE {
                0.7
            } else {
                0.5
            };
            Candidate {
                from: r.other_id,
                to: r.hub_id,
                kind: RelationshipKind::SameUniverse,
                confidence: c,
                source: EvidenceSource::SeriesGroup,
                reason: format!(
                    "{} and {} share the series group \"{}\" ({} series in the group)",
                    label(&r.other_name, r.other_year),
                    label(&r.hub_name, r.hub_year),
                    r.grp,
                    r.grp_n
                ),
                evidence: json!({
                    "source": "series_group",
                    "series_group": r.grp,
                    "group_size": r.grp_n,
                }),
            }
        })
        .collect())
}

// ───── shared StoryArc → crossover_with ─────

#[derive(Debug, FromQueryResult)]
struct ArcRow {
    hub_id: Uuid,
    hub_name: String,
    hub_year: Option<i32>,
    other_id: Uuid,
    other_name: String,
    other_year: Option<i32>,
    shared_arcs: i64,
    hub_issues: i64,
    other_issues: i64,
    arc_names: Vec<String>,
    /// Series count of the smallest shared arc.
    smallest_arc: i64,
}

/// Story arcs (the `issue_arcs` junction) spanning several series. For each
/// arc the series with the most issues in it is the hub and every other
/// series links to it (an event with 40 tie-ins → 39 suggestions, not 780).
/// Pairs are then aggregated across arcs. Series with the same normalized
/// name are skipped (that's a run continuing, handled by name continuation).
pub async fn story_arcs<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        WITH per AS (
            SELECT ia.arc_id, i.series_id, count(*) AS n
              FROM issue_arcs ia
              JOIN issues i ON i.id = ia.issue_id
             WHERE i.library_id = $1 AND i.removed_at IS NULL
             GROUP BY 1, 2
        ), multi AS (
            SELECT arc_id FROM per GROUP BY arc_id HAVING count(*) >= 2
        ), ranked AS (
            SELECT p.arc_id, p.series_id, p.n,
                   row_number() OVER (PARTITION BY p.arc_id ORDER BY p.n DESC, s.year NULLS LAST, s.id) AS rk,
                   count(*) OVER (PARTITION BY p.arc_id) AS arc_n
              FROM per p
              JOIN multi USING (arc_id)
              JOIN series s ON s.id = p.series_id AND s.removed_at IS NULL
        ), pairs AS (
            SELECT h.series_id AS hub_id, o.series_id AS other_id, h.arc_id, h.n AS hub_n, o.n AS other_n,
                   h.arc_n
              FROM ranked h
              JOIN ranked o ON o.arc_id = h.arc_id AND o.rk > 1
             WHERE h.rk = 1
        )
        SELECT p.hub_id, hs.name AS hub_name, hs.year AS hub_year,
               p.other_id, os.name AS other_name, os.year AS other_year,
               count(*) AS shared_arcs, sum(p.hub_n)::bigint AS hub_issues,
               sum(p.other_n)::bigint AS other_issues,
               min(p.arc_n) AS smallest_arc,
               (array_agg(a.name ORDER BY a.name))[1:5] AS arc_names
          FROM pairs p
          JOIN story_arc a ON a.id = p.arc_id
          JOIN series hs ON hs.id = p.hub_id
          JOIN series os ON os.id = p.other_id
         WHERE hs.normalized_name <> os.normalized_name
         GROUP BY p.hub_id, hs.name, hs.year, p.other_id, os.name, os.year
         ORDER BY count(*) DESC
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = ArcRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut c: f32 = if r.other_issues >= 2 { 0.65 } else { 0.5 };
            c += 0.1 * (r.shared_arcs.saturating_sub(1).min(3)) as f32;
            // An event arc spanning dozens of series ("Secret Wars"
            // Battleworld) links every tie-in to one hub; each such link
            // is weak, so keep them out of medium/high.
            if r.smallest_arc > ARC_EVENT_SIZE {
                c = c.min(0.45);
            }
            let c = round2(c.min(0.9));
            let arcs = r
                .arc_names
                .iter()
                .map(|a| format!("\"{a}\""))
                .collect::<Vec<_>>()
                .join(", ");
            Candidate {
                from: r.other_id,
                to: r.hub_id,
                kind: RelationshipKind::CrossoverWith,
                confidence: c,
                source: EvidenceSource::StoryArc,
                reason: format!(
                    "{} and {} share {}: {}",
                    label(&r.other_name, r.other_year),
                    label(&r.hub_name, r.hub_year),
                    plural(r.shared_arcs, "story arc", "story arcs"),
                    arcs
                ),
                evidence: json!({
                    "source": "story_arc",
                    "shared_arcs": r.shared_arcs,
                    "smallest_arc_series": r.smallest_arc,
                    "arc_names": r.arc_names,
                    "issues_in_from": r.other_issues,
                    "issues_in_to": r.hub_issues,
                }),
            }
        })
        .collect())
}

// ───── name continuation → sequel_of ─────

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
    let sql = format!(
        r#"
        WITH s AS (
            SELECT s.id, s.name, s.year, s.volume, s.normalized_name,
                   lower(coalesce(s.publisher, '')) AS pub,
                   {leaf} AS leaf, {parent} AS parent
              FROM series s
             WHERE s.library_id = $1 AND s.removed_at IS NULL
        ), k AS (
            SELECT id, name, year, pub,
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
                   lag(ord)  OVER w AS prev_ord
              FROM k
             WHERE base <> '' AND (year IS NOT NULL OR ord IS NOT NULL)
            WINDOW w AS (PARTITION BY base, pub ORDER BY year NULLS LAST, ord NULLS LAST, id)
        )
        SELECT id, name, year, ord, prev_id, prev_name, prev_year, prev_ord, vol_folder
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
                RelationshipKind::SequelOf,
                0.9,
                format!(
                    "{} follows {} — same title, next volume",
                    vol(&r.name, b, r.year),
                    vol(&r.prev_name, a, r.prev_year)
                ),
            ),
            (Some(a), Some(b), _, _) if b > a => (
                RelationshipKind::SequelOf,
                0.65,
                format!(
                    "{} follows {} — same title; the volumes in between aren't in the library",
                    vol(&r.name, b, r.year),
                    vol(&r.prev_name, a, r.prev_year)
                ),
            ),
            (_, _, Some(py), Some(y)) if y > py => (
                RelationshipKind::SequelOf,
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
        out.push(Candidate {
            from: r.id,
            to: r.prev_id,
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
            }),
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
    let class = |s: &Option<String>| s.as_deref().and_then(classify_format);
    class(&r.format) == Some(FormatClass::Collected)
        || class(&r.special_type) == Some(FormatClass::Collected)
        || class(&r.series_type) == Some(FormatClass::Collected)
        || matches!(
            infer_format_from_title(&r.series_name, None),
            Some("Omnibus" | "Hardcover" | "Graphic Novel" | "TPB")
        )
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
/// marker like "TPB" or "Omnibus") whose notes, title or "Collects …"
/// summary cite issue ranges of another series. The citation parsing is in
/// [`citations`]; resolution against the library is one set-based query.
pub async fn collected_editions<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        SELECT i.series_id, s.name AS series_name, s.normalized_name AS series_norm,
               s.series_type, i.format, i.special_type, i.title, i.notes, i.summary
          FROM issues i
          JOIN series s ON s.id = i.series_id AND s.removed_at IS NULL
         WHERE i.library_id = $1 AND i.removed_at IS NULL
           AND (i.format IS NOT NULL OR i.special_type IS NOT NULL OR s.series_type IS NOT NULL
                OR s.normalized_name ~ '(^| )(tpb|tp|hc|ogn|omnibus|hardcover|compendium)( |$)|trade paperback|graphic novel|collected edition|deluxe edition|library edition')
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
            SELECT id, name, year, normalized_name, {base} AS base
              FROM series WHERE library_id = $1 AND removed_at IS NULL
        ), m AS (
            SELECT c.idx, c.lo, c.hi, t.id, t.name, t.year
              FROM c JOIN t ON t.normalized_name = c.name_norm AND t.id <> c.from_id
            UNION
            SELECT c.idx, c.lo, c.hi, t.id, t.name, t.year
              FROM c JOIN t ON t.base = c.name_norm AND t.id <> c.from_id
        ), scored AS (
            SELECT m.idx, m.id AS to_id, m.name AS to_name, m.year AS to_year,
                   (SELECT count(*) FROM issues i
                     WHERE i.series_id = m.id AND i.removed_at IS NULL
                       AND i.sort_number BETWEEN m.lo AND m.hi) AS covered,
                   count(*) OVER (PARTITION BY m.idx) AS n_candidates
              FROM m
        )
        SELECT DISTINCT ON (idx) idx, to_id, to_name, to_year, covered, n_candidates
          FROM scored
         ORDER BY idx, covered DESC, to_year NULLS LAST, to_id
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

    let mut out = Vec::with_capacity(resolved.len());
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
        let range = format!("#{}–{}", fmt_num(cite.lo), fmt_num(cite.hi));
        out.push(Candidate {
            from: cite.from,
            to: r.to_id,
            kind: RelationshipKind::Collects,
            confidence: round2(c.max(0.05)),
            source: EvidenceSource::CollectedEdition,
            reason: format!(
                "{} is a collected edition citing {} {} ({} of those issues are in the library)",
                cite.from_name,
                label(&r.to_name, r.to_year),
                range,
                r.covered
            ),
            evidence: json!({
                "source": "collected_edition",
                "cited_name": cite.name,
                "name_explicit": cite.explicit,
                "range_low": cite.lo,
                "range_high": cite.hi,
                "issues_in_library": r.covered,
                "same_name_candidates": r.n_candidates,
            }),
        });
    }
    Ok(out)
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ───── provider volume ids → sequel_of / see_also ─────

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
/// members whose issue ranges are disjoint and ordered are `sequel_of`
/// (the provider sees one continuous run); overlapping ranges are
/// `see_also` (likely duplicates or variant files of the same run).
pub async fn provider_volumes<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
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
        ), shared AS (
            SELECT source, pid, count(*) AS members FROM claims
             GROUP BY 1, 2 HAVING count(*) BETWEEN 2 AND 12
        ), rng AS (
            SELECT i.series_id, min(i.sort_number) AS lo, max(i.sort_number) AS hi
              FROM issues i
             WHERE i.series_id IN (SELECT c.series_id FROM claims c JOIN shared USING (source, pid))
               AND i.removed_at IS NULL AND i.sort_number IS NOT NULL
             GROUP BY 1
        ), o AS (
            SELECT c.source, c.pid, sh.members, s.id, s.name, s.year, r.lo, r.hi,
                   lag(s.id)   OVER w AS prev_id,
                   lag(s.name) OVER w AS prev_name,
                   lag(s.year) OVER w AS prev_year,
                   lag(r.lo)   OVER w AS prev_lo,
                   lag(r.hi)   OVER w AS prev_hi
              FROM claims c
              JOIN shared sh USING (source, pid)
              JOIN series s ON s.id = c.series_id
              LEFT JOIN rng r ON r.series_id = c.series_id
            WINDOW w AS (PARTITION BY c.source, c.pid ORDER BY r.lo NULLS LAST, s.year NULLS LAST, s.id)
        )
        SELECT source, pid, id, name, year, lo, hi, prev_id, prev_name, prev_year, prev_lo, prev_hi, members
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
                    RelationshipKind::SequelOf,
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
            Candidate {
                from: r.id,
                to: r.prev_id,
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
                }),
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
/// suggest `see_also`. Not `sequel_of`: the range sits *inside* A (A isn't
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
                to: r.b_id,
                kind: RelationshipKind::SeeAlso,
                confidence: 0.7,
                source: EvidenceSource::ProviderRange,
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

// ───── publisher + character/team density → same_universe ─────

#[derive(Debug, FromQueryResult)]
struct DensityRow {
    a_id: Uuid,
    a_name: String,
    a_year: Option<i32>,
    b_id: Uuid,
    b_name: String,
    b_year: Option<i32>,
    publisher: String,
    shared: i64,
    overlap: f64,
    sample: Vec<String>,
}

/// Same publisher plus a dense overlap of **uncommon** characters/teams
/// (`series_characters` / `series_teams`). Only features present in at most
/// a library-relative document-frequency cap (see `DENSITY_DF_FRACTION`) take part, which both makes the signal
/// discriminating and bounds the self-join; each series keeps its top
/// [`DENSITY_PER_SERIES`] partners (counted across both ends). Always low confidence (≤ 0.5): within
/// one publisher "same universe" is nearly always true and rarely useful,
/// and WP-7.1 flagged `same_universe` bloat.
pub async fn character_density<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<Vec<Candidate>, DbErr> {
    let sql = format!(
        r#"
        WITH lib AS (
            SELECT id FROM series
             WHERE library_id = $1 AND removed_at IS NULL AND publisher IS NOT NULL
        ), feat AS (
            SELECT DISTINCT sc.series_id, 'c:' || coalesce(sc.character_id::text, lower(sc.character)) AS f,
                   sc.character AS label
              FROM series_characters sc JOIN lib ON lib.id = sc.series_id
            UNION
            SELECT DISTINCT st.series_id, 't:' || coalesce(st.team_id::text, lower(st.team)), st.team
              FROM series_teams st JOIN lib ON lib.id = st.series_id
        ), df AS (
            SELECT f, count(DISTINCT series_id) AS n FROM feat GROUP BY f
        ), cap AS (
            SELECT least({DENSITY_MAX_DF}, greatest({DENSITY_MIN_DF},
                   ceil(count(DISTINCT series_id) * {DENSITY_DF_FRACTION})::bigint)) AS max_df
              FROM feat
        ), rare AS (
            SELECT DISTINCT ON (feat.series_id, feat.f) feat.series_id, feat.f, feat.label
              FROM feat JOIN df USING (f) CROSS JOIN cap
             WHERE df.n BETWEEN 2 AND cap.max_df
        ), sz AS (
            SELECT series_id, count(*) AS n FROM rare GROUP BY 1
        ), pairs AS (
            SELECT a.series_id AS a_id, b.series_id AS b_id, count(*) AS shared,
                   (array_agg(a.label ORDER BY a.label))[1:5] AS sample
              FROM rare a JOIN rare b ON a.f = b.f AND a.series_id < b.series_id
             GROUP BY 1, 2
            HAVING count(*) >= {DENSITY_MIN_SHARED}
        ), scored AS (
            SELECT p.*, p.shared::float8 / least(sa.n, sb.n) AS overlap
              FROM pairs p
              JOIN sz sa ON sa.series_id = p.a_id
              JOIN sz sb ON sb.series_id = p.b_id
        ), same_pub AS (
            SELECT sc.*, x.name AS a_name, x.year AS a_year, y.name AS b_name, y.year AS b_year,
                   x.publisher
              FROM scored sc
              JOIN series x ON x.id = sc.a_id
              JOIN series y ON y.id = sc.b_id
             WHERE lower(x.publisher) = lower(y.publisher)
               AND sc.overlap >= {DENSITY_MIN_OVERLAP}
               AND x.normalized_name <> y.normalized_name
        ), ends AS (
            -- Each pair once per end, so a series' rank counts partners on
            -- both sides of the (a < b) pair.
            SELECT a_id AS sid, a_id, b_id, overlap, shared FROM same_pub
            UNION ALL
            SELECT b_id, a_id, b_id, overlap, shared FROM same_pub
        ), ranked AS (
            SELECT a_id, b_id,
                   row_number() OVER (PARTITION BY sid
                                      ORDER BY overlap DESC, shared DESC, a_id, b_id) AS r
              FROM ends
        ), kept AS (
            SELECT a_id, b_id FROM ranked GROUP BY a_id, b_id
            HAVING max(r) <= {DENSITY_PER_SERIES}
        )
        SELECT sp.a_id, sp.a_name, sp.a_year, sp.b_id, sp.b_name, sp.b_year, sp.publisher,
               sp.shared, sp.overlap, sp.sample
          FROM same_pub sp
          JOIN kept USING (a_id, b_id)
         ORDER BY sp.overlap DESC, sp.shared DESC
         LIMIT {SOURCE_ROW_LIMIT}
        "#
    );
    let rows = DensityRow::find_by_statement(stmt(conn, &sql, vec![library_id.into()]))
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let c = round2((0.25 + 0.25 * r.overlap as f32).min(0.5));
            Candidate {
                from: r.a_id,
                to: r.b_id,
                kind: RelationshipKind::SameUniverse,
                confidence: c,
                source: EvidenceSource::CharacterDensity,
                reason: format!(
                    "{} and {} are both {} and share {} uncommon characters/teams (e.g. {})",
                    label(&r.a_name, r.a_year),
                    label(&r.b_name, r.b_year),
                    r.publisher,
                    r.shared,
                    r.sample.join(", ")
                ),
                evidence: json!({
                    "source": "character_density",
                    "publisher": r.publisher,
                    "shared_features": r.shared,
                    "overlap": (r.overlap * 100.0).round() / 100.0,
                    "sample": r.sample,
                }),
            }
        })
        .collect())
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
    run!("series_group", series_group(conn, library_id));
    run!("story_arc", story_arcs(conn, library_id));
    run!("name_continuation", name_continuation(conn, library_id));
    run!("collected_edition", collected_editions(conn, library_id));
    run!("provider_volume", provider_volumes(conn, library_id));
    run!("provider_range", provider_ranges(conn, library_id));
    run!("character_density", character_density(conn, library_id));
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
