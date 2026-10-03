//! Provider range detection for **every** series — resolve a local
//! series' id at each issue-enumerating provider (Metron, GCD), then run
//! the split detector ([`crate::metadata::auto_split`]) against it.
//!
//! Before this, "Detect from providers" only scanned providers the series
//! already had an *applied* match for. Most libraries are matched through
//! ComicVine, which can't enumerate issue numbers, so detection did
//! nothing for nearly every series. This module finds the Metron / GCD id
//! even when the series was only ever matched through ComicVine:
//!
//! 1. **Linked** — a series-level `external_ids` row for that source
//!    (a `user` row first), or the latest applied run candidate.
//! 2. **Bridge (cache)** — free: a cached provider series detail that
//!    lists the target's id (Metron series carry `cv_id` / `gcd_id`), or a
//!    cached target-provider series whose identifiers list one of ours.
//! 3. **Bridge (network)** — Metron's curated cross-reference
//!    (`/api/series/?cv_id=`; one request, and its row carries the GCD id
//!    too), or the Metron series detail's `gcd_id` (7-day cached).
//! 4. **Search** — the provider's series search, pre-filtered
//!    ([`PreFilter`], hard year gate) and scored by the matcher
//!    ([`crate::metadata::matcher::score_series`]). Only a *strict* match
//!    is used automatically — see [`classify_search`]. Anything weaker is
//!    returned for the admin to confirm, never written.
//!
//! Ids found by (2)–(4) are recorded through the audited writer
//! ([`writers::set_external_id_promoting`], `SetBy::Provider`), which
//! keeps the user-precedence rule. Each click is bounded: at most one
//! cross-reference lookup and one series search per provider, the
//! detector's [`auto_split::MAX_GAPS_RESOLVED`] gaps, and a wall-clock
//! budget ([`DETECT_TIME_BUDGET`]) so the request stays inside the JSON
//! route timeout.

use crate::metadata::auto_split::{self, DetectOutcome, LocalIssue};
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::matcher::{
    self, Confidence, Score, SeriesQueryFacts, Thresholds, canonical_issue_number,
};
use crate::metadata::orchestrator::{PreFilter, pre_filter_series};
use crate::metadata::provider::{MetadataProvider, ProviderError, SeriesCandidate, SeriesQuery};
use crate::metadata::writers::{self, SetBy, SetExternalIdOutcome};
use crate::state::AppState;
use entity::{external_id, metadata_cache, series};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, Statement,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Providers that can enumerate a series' issues, in the order detection
/// resolves them. Metron goes first: its cross-reference row also carries
/// the GCD id, so GCD usually resolves without a GCD request.
pub const ENUMERATING_SOURCES: [Source; 2] = [Source::Metron, Source::Gcd];

/// Share of the local numbered issues a searched candidate's issue list
/// must carry before it is linked automatically.
pub const MIN_CONFIRM_OVERLAP: f32 = 0.5;

/// Wall-clock budget for one detection click. Sources not started (and
/// gaps not attempted) when it runs out are reported `skipped`, so the
/// request finishes well inside the 60 s JSON route timeout.
pub const DETECT_TIME_BUDGET: Duration = Duration::from_secs(40);

/// Review candidates returned when a search finds no strict match.
const MAX_REVIEW_CANDIDATES: usize = 3;

/// How a provider series id was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkMethod {
    /// A series-level external id (user- or provider-set).
    Linked,
    /// The series' latest applied match for that provider.
    Applied,
    /// A provider's curated cross-reference (Metron `cv_id` / `gcd_id`).
    Bridge,
    /// A strict series-search match confirmed by issue coverage.
    Search,
}

/// One provider's outcome for a detection click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// The provider series was enumerated and the gaps checked.
    Scanned,
    /// The provider can't list a series' issues (ComicVine).
    NotEnumerable,
    /// The provider isn't configured / enabled.
    NotConfigured,
    /// No provider series could be found for this series.
    NoSeries,
    /// Possible matches were found but none strong enough to use
    /// automatically; see `candidates`.
    NeedsConfirmation,
    /// The provider's rate limit stopped the work.
    RateLimited,
    /// Not attempted: the per-click time budget ran out.
    Skipped,
    /// A provider call failed.
    Error,
}

/// A provider series the admin may confirm (medium-confidence search hit).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LinkCandidate {
    pub external_id: String,
    pub name: String,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    /// Matcher text score (0–100).
    pub score: f32,
    /// Share of the local numbered issues the candidate lists, when it was
    /// enumerated (the top candidate only).
    pub issue_overlap: Option<f32>,
    /// Why it wasn't linked automatically.
    pub reason: String,
}

/// The provider series a source resolved to.
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    pub external_id: String,
    pub method: LinkMethod,
    /// The provider whose data vouched for the id (bridge: Metron).
    pub attested_by: Source,
    pub name: Option<String>,
    pub year: Option<i32>,
    /// The id was recorded on the series' `external_ids` this click.
    pub written: bool,
    /// Issue numbers already enumerated while confirming the match.
    covered: Option<Vec<String>>,
}

/// Everything detection found for one source.
#[derive(Debug, Clone)]
pub struct SourceDetect {
    pub source: Source,
    pub status: SourceStatus,
    pub link: Option<ResolvedLink>,
    pub candidates: Vec<LinkCandidate>,
    pub outcome: Option<DetectOutcome>,
    pub error: Option<String>,
}

impl SourceDetect {
    fn new(source: Source, status: SourceStatus) -> Self {
        Self {
            source,
            status,
            link: None,
            candidates: Vec::new(),
            outcome: None,
            error: None,
        }
    }
}

/// A series-level id this series already has for one source.
#[derive(Debug, Clone)]
struct Known {
    id: String,
    method: LinkMethod,
}

/// Run detection for `series_row` across every configured provider.
pub async fn detect_series(state: &AppState, series_row: &series::Model) -> Vec<SourceDetect> {
    let started = Instant::now();
    let db = &state.db;

    let mut known = known_ids(state, series_row.id).await;
    // Ids learned from a cross-reference this click (target → (id, attestor)).
    let mut bridged: HashMap<Source, (String, Source)> = HashMap::new();
    let local = auto_split::load_local_issues(db, series_row.id)
        .await
        .unwrap_or_default();
    let library = entity::library::Entity::find_by_id(series_row.library_id)
        .one(db)
        .await
        .ok()
        .flatten();
    let pre_filter = library
        .as_ref()
        .map(PreFilter::from_library)
        .unwrap_or_default();
    let cfg = state.cfg();
    let thresholds = Thresholds::new(
        cfg.metadata_auto_apply_threshold as f32,
        cfg.metadata_match_medium_threshold as f32,
    );

    // Enumerators first (Metron, GCD), then any other source the series
    // is linked to (reported as "can't enumerate").
    let mut order: Vec<Source> = ENUMERATING_SOURCES.to_vec();
    let mut others: Vec<Source> = known
        .keys()
        .copied()
        .filter(|s| !order.contains(s))
        .collect();
    others.sort_by_key(|s| s.as_str());
    order.extend(others);

    let mut results = Vec::new();
    for source in order {
        let provider = crate::metadata::apply::build_provider(state, source);
        let Some(provider) = provider else {
            // Only list an unconfigured provider when it matters: it's an
            // enumerator, or the series is linked to it.
            if ENUMERATING_SOURCES.contains(&source) || known.contains_key(&source) {
                results.push(SourceDetect::new(source, SourceStatus::NotConfigured));
            }
            continue;
        };
        if !provider.enumerates_series_issues() {
            let mut r = SourceDetect::new(source, SourceStatus::NotEnumerable);
            if let Some(k) = known.get(&source) {
                r.link = Some(ResolvedLink {
                    external_id: k.id.clone(),
                    method: k.method,
                    attested_by: source,
                    name: None,
                    year: None,
                    written: false,
                    covered: None,
                });
            }
            results.push(r);
            continue;
        }
        if started.elapsed() >= DETECT_TIME_BUDGET {
            results.push(SourceDetect::new(source, SourceStatus::Skipped));
            continue;
        }

        let mut r = SourceDetect::new(source, SourceStatus::NoSeries);
        match resolve_link(
            state,
            series_row,
            source,
            &*provider,
            &known,
            &mut bridged,
            &local,
            &pre_filter,
            thresholds,
        )
        .await
        {
            Ok(Resolution::Found(link)) => r.link = Some(link),
            Ok(Resolution::Review(cands)) => {
                r.status = SourceStatus::NeedsConfirmation;
                r.candidates = cands;
                results.push(r);
                continue;
            }
            Ok(Resolution::NotFound(note)) => {
                r.error = note;
                results.push(r);
                continue;
            }
            Err(e) => {
                r.status = status_for(&e);
                r.error = Some(e.to_string());
                results.push(r);
                continue;
            }
        }

        // Record a newly-found id (bridge / search) through the audited
        // writer. A live series already owning that id blocks the link.
        let link = r.link.as_mut().expect("set above");
        if matches!(link.method, LinkMethod::Bridge | LinkMethod::Search) {
            match record_link(state, series_row.id, source, link).await {
                Ok(true) => link.written = true,
                Ok(false) => {
                    r.status = SourceStatus::NoSeries;
                    r.error = Some(format!(
                        "{} series {} is already linked to another series in the library",
                        source.label(),
                        link.external_id
                    ));
                    r.link = None;
                    results.push(r);
                    continue;
                }
                Err(e) => {
                    r.status = SourceStatus::Error;
                    r.error = Some(format!("db: {e}"));
                    results.push(r);
                    continue;
                }
            }
            known.insert(
                source,
                Known {
                    id: link.external_id.clone(),
                    method: link.method,
                },
            );
        }

        // Detect the split against the resolved series.
        let link = r.link.as_ref().expect("set above");
        let covered = match &link.covered {
            Some(c) => Ok(c.clone()),
            None => provider.list_series_issue_numbers(&link.external_id).await,
        };
        let outcome = match covered {
            Ok(covered) => {
                auto_split::detect_with_coverage(
                    db,
                    series_row,
                    source,
                    &link.external_id,
                    &covered,
                    &*provider,
                )
                .await
            }
            Err(e) => {
                r.status = status_for(&e);
                r.error = Some(e.to_string());
                results.push(r);
                continue;
            }
        };
        match outcome {
            Ok(o) => {
                r.status = if o.error.is_some() {
                    SourceStatus::RateLimited
                } else {
                    SourceStatus::Scanned
                };
                r.error = o.error.clone();
                r.outcome = Some(o);
            }
            Err(e) => {
                r.status = SourceStatus::Error;
                r.error = Some(e.to_string());
            }
        }
        results.push(r);
    }
    results
}

fn status_for(e: &ProviderError) -> SourceStatus {
    match e {
        ProviderError::QuotaExceeded { .. } => SourceStatus::RateLimited,
        _ => SourceStatus::Error,
    }
}

enum Resolution {
    Found(ResolvedLink),
    Review(Vec<LinkCandidate>),
    NotFound(Option<String>),
}

/// Series-level ids per source: a `user` external id wins, then the
/// latest applied run candidate, then any other external id.
async fn known_ids(state: &AppState, series_id: Uuid) -> HashMap<Source, Known> {
    let rows = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .all(&state.db)
        .await
        .unwrap_or_default();
    let applied = applied_series_targets(&state.db, series_id).await;

    let mut out: HashMap<Source, Known> = HashMap::new();
    for r in &rows {
        if let Ok(src) = Source::from_str(&r.source)
            && r.set_by == "user"
        {
            out.insert(
                src,
                Known {
                    id: r.external_id.clone(),
                    method: LinkMethod::Linked,
                },
            );
        }
    }
    for (src, id) in applied {
        out.entry(src).or_insert(Known {
            id,
            method: LinkMethod::Applied,
        });
    }
    for r in &rows {
        if let Ok(src) = Source::from_str(&r.source) {
            out.entry(src).or_insert(Known {
                id: r.external_id.clone(),
                method: LinkMethod::Linked,
            });
        }
    }
    out
}

/// Latest applied provider series per source for `series_id`, from the
/// run candidates (most-recent `applied_at` wins). Under writeback the
/// series-level `external_ids` aren't written until a later rescan, so
/// the applied candidates are the freshest record of a manual match.
pub async fn applied_series_targets<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Vec<(Source, String)> {
    use entity::{metadata_run, metadata_run_candidate};

    let run_ids: Vec<Uuid> = metadata_run::Entity::find()
        .filter(metadata_run::Column::Scope.eq("series"))
        .filter(metadata_run::Column::ScopeEntityId.eq(series_id.to_string()))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.id)
        .collect();
    if run_ids.is_empty() {
        return Vec::new();
    }
    let cands = metadata_run_candidate::Entity::find()
        .filter(metadata_run_candidate::Column::RunId.is_in(run_ids))
        .filter(metadata_run_candidate::Column::AppliedAt.is_not_null())
        .order_by_desc(metadata_run_candidate::Column::AppliedAt)
        .all(db)
        .await
        .unwrap_or_default();

    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for c in cands {
        if let Ok(src) = Source::from_str(&c.source)
            && seen.insert(src)
        {
            targets.push((src, c.external_id));
        }
    }
    targets
}

#[allow(clippy::too_many_arguments)]
async fn resolve_link(
    state: &AppState,
    series_row: &series::Model,
    target: Source,
    provider: &dyn MetadataProvider,
    known: &HashMap<Source, Known>,
    bridged: &mut HashMap<Source, (String, Source)>,
    local: &[LocalIssue],
    pre_filter: &PreFilter,
    thresholds: Thresholds,
) -> Result<Resolution, ProviderError> {
    let db = &state.db;
    // 1. Linked / applied.
    if let Some(k) = known.get(&target) {
        let (name, year) = display_meta(db, target, &k.id).await;
        return Ok(Resolution::Found(ResolvedLink {
            external_id: k.id.clone(),
            method: k.method,
            attested_by: target,
            name,
            year,
            written: false,
            covered: None,
        }));
    }
    let known_pairs: Vec<(Source, String)> =
        known.iter().map(|(s, k)| (*s, k.id.clone())).collect();

    // 2. Cross-reference learned earlier this click, then the free cache
    //    bridge.
    let bridge = match bridged.get(&target) {
        Some(b) => Some(b.clone()),
        None => cache_bridge(db, target, &known_pairs).await,
    };
    if let Some((id, attestor)) = bridge {
        let (name, year) = display_meta(db, target, &id).await;
        return Ok(Resolution::Found(ResolvedLink {
            external_id: id,
            method: LinkMethod::Bridge,
            attested_by: attestor,
            name,
            year,
            written: false,
            covered: None,
        }));
    }

    // 3. Network bridge.
    match target {
        Source::Metron => {
            for (src, id) in known_pairs
                .iter()
                .filter(|(s, _)| matches!(s, Source::ComicVine | Source::Gcd))
            {
                let hits = provider.find_series_by_cross_ref(*src, id).await?;
                // Ambiguous (several Metron series claim the id) — don't
                // guess; fall through to search.
                if let [hit] = hits.as_slice() {
                    for ident in &hit.identifiers {
                        if ident.source != *src && !ident.id.is_empty() {
                            bridged
                                .entry(ident.source)
                                .or_insert((ident.id.clone(), Source::Metron));
                        }
                    }
                    return Ok(Resolution::Found(ResolvedLink {
                        external_id: hit.external_id.clone(),
                        method: LinkMethod::Bridge,
                        attested_by: Source::Metron,
                        name: hit.name.clone(),
                        year: hit.year_began,
                        written: false,
                        covered: None,
                    }));
                }
            }
        }
        Source::Gcd => {
            // The Metron series' detail lists its GCD id (7-day cache).
            if let Some(k) = known.get(&Source::Metron)
                && let Some(metron) = crate::metadata::apply::build_provider(state, Source::Metron)
            {
                match crate::metadata::apply::fetch_series_detail(state, &*metron, &k.id).await {
                    Ok(detail) => {
                        if let Some(ident) = detail
                            .identifiers
                            .iter()
                            .find(|i| i.source == Source::Gcd && !i.id.is_empty())
                        {
                            let (name, year) = display_meta(db, target, &ident.id).await;
                            return Ok(Resolution::Found(ResolvedLink {
                                external_id: ident.id.clone(),
                                method: LinkMethod::Bridge,
                                attested_by: Source::Metron,
                                name,
                                year,
                                written: false,
                                covered: None,
                            }));
                        }
                    }
                    // A Metron failure only loses the bridge; GCD's own
                    // search still runs.
                    Err(e) => {
                        tracing::debug!(error = %e, "series link: Metron detail bridge failed");
                    }
                }
            }
        }
        _ => {}
    }

    // 4. Search.
    search_link(series_row, target, provider, local, pre_filter, thresholds).await
}

/// Free bridge over cached provider series details (`metadata_cache`):
///
/// - **forward** — a cached detail of a series we know (any source) that
///   lists the target's id (Metron series carry `cv_id` / `gcd_id`);
/// - **reverse** — a cached target-provider series whose identifiers list
///   one of our known ids. Used only when exactly one series matches.
async fn cache_bridge<C: ConnectionTrait>(
    db: &C,
    target: Source,
    known: &[(Source, String)],
) -> Option<(String, Source)> {
    for (src, id) in known {
        let Ok(Some(row)) = metadata_cache::Entity::find_by_id((
            src.as_str().to_owned(),
            "series".to_owned(),
            id.clone(),
        ))
        .one(db)
        .await
        else {
            continue;
        };
        if let Some(found) = identifier_for(&row.payload, target) {
            return Some((found, *src));
        }
    }

    #[derive(FromQueryResult)]
    struct Row {
        external_id: String,
    }
    for (src, id) in known {
        let needle = serde_json::json!([{ "source": src.as_str(), "id": id }]);
        let rows = Row::find_by_statement(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT external_id FROM metadata_cache \
              WHERE provider = $1 AND entity = 'series' \
                AND jsonb_typeof(payload->'identifiers') = 'array' \
                AND payload->'identifiers' @> $2::jsonb \
              LIMIT 2",
            vec![target.as_str().into(), needle.into()],
        ))
        .all(db)
        .await
        .unwrap_or_default();
        if let [only] = rows.as_slice() {
            return Some((only.external_id.clone(), target));
        }
    }
    None
}

/// The `target` id listed in a cached `GenericMetadata` payload's
/// `identifiers`, read tolerantly from the raw JSON.
fn identifier_for(payload: &serde_json::Value, target: Source) -> Option<String> {
    payload
        .get("identifiers")?
        .as_array()?
        .iter()
        .find(|v| v.get("source").and_then(|s| s.as_str()) == Some(target.as_str()))
        .and_then(|v| v.get("id").and_then(|s| s.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

async fn display_meta<C: ConnectionTrait>(
    db: &C,
    source: Source,
    id: &str,
) -> (Option<String>, Option<i32>) {
    crate::metadata::cache::series_display_meta(db, source, id)
        .await
        .unwrap_or((None, None))
}

/// What [`classify_search`] decided.
#[derive(Debug, PartialEq)]
pub(crate) enum SearchVerdict {
    /// Index of the one strict candidate — still subject to the issue
    /// coverage check before it is used.
    Strict(usize),
    /// Indices of reviewable candidates (best first).
    Review(Vec<usize>),
    None,
}

/// Decide which scored series candidates (already pre-filtered) can be
/// linked automatically.
///
/// A candidate is **strict** when the matcher buckets it at least MEDIUM
/// and its name normalizes to the local name, its start year equals the
/// local year, its publisher doesn't conflict (an unknown publisher is
/// fine — Metron's list and a cold GCD cache carry none), and it has no
/// format mismatch. It's used only when it is the *only* strict
/// candidate. Every other MEDIUM-or-better candidate is for review.
pub(crate) fn classify_search(
    facts: &SeriesQueryFacts,
    scored: &[(SeriesCandidate, Score)],
    thresholds: Thresholds,
) -> SearchVerdict {
    let mut order: Vec<usize> = (0..scored.len())
        .filter(|&i| scored[i].1.bucket(thresholds) != Confidence::Low)
        .collect();
    order.sort_by(|&a, &b| {
        scored[b]
            .1
            .total
            .partial_cmp(&scored[a].1.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let strict: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| {
            let (c, s) = &scored[i];
            matcher::name_similarity(&facts.name, &c.name) >= 0.999
                && facts.year.is_some()
                && facts.year == c.year
                && (s.publisher > 0.0)
                && !s.format_mismatch
        })
        .collect();
    match strict.as_slice() {
        [only] => SearchVerdict::Strict(*only),
        _ if order.is_empty() => SearchVerdict::None,
        _ => SearchVerdict::Review(order),
    }
}

/// Share of the local numbered issues present in `listed`.
fn overlap(local: &[LocalIssue], listed: &[String]) -> Option<f32> {
    let listed: HashSet<String> = listed.iter().map(|n| canonical_issue_number(n)).collect();
    let numbered: HashSet<&str> = local
        .iter()
        .filter(|li| li.value.is_some())
        .map(|li| li.canonical.as_str())
        .collect();
    if numbered.is_empty() || listed.is_empty() {
        return None;
    }
    let hits = numbered.iter().filter(|n| listed.contains(**n)).count();
    Some(hits as f32 / numbered.len() as f32)
}

async fn search_link(
    series_row: &series::Model,
    target: Source,
    provider: &dyn MetadataProvider,
    local: &[LocalIssue],
    pre_filter: &PreFilter,
    thresholds: Thresholds,
) -> Result<Resolution, ProviderError> {
    let facts = SeriesQueryFacts {
        name: series_row.name.clone(),
        year: series_row.year,
        publisher: series_row.publisher.clone(),
        volume: series_row.volume,
        format: series_row.series_type.clone(),
    };
    let raw = provider
        .search_series(&SeriesQuery {
            name: facts.name.clone(),
            year: facts.year,
            publisher: facts.publisher.clone(),
            limit: 25,
        })
        .await?;
    let kept = pre_filter_series(raw, &facts, pre_filter);
    let scored: Vec<(SeriesCandidate, Score)> = kept
        .into_iter()
        .map(|c| {
            let s = matcher::score_series(&facts, &c);
            (c, s)
        })
        .collect();

    let to_candidate = |i: usize, overlap: Option<f32>, reason: &str| {
        let (c, s) = &scored[i];
        LinkCandidate {
            external_id: c.external_id.clone(),
            name: c.name.clone(),
            year: c.year,
            publisher: c.publisher.clone(),
            url: c.external_url.clone().or_else(|| {
                crate::metadata::identifier::canonical_url(target, "series", &c.external_id)
            }),
            score: (s.total * 10.0).round() / 10.0,
            issue_overlap: overlap.map(|o| (o * 100.0).round() / 100.0),
            reason: reason.to_owned(),
        }
    };

    match classify_search(&facts, &scored, thresholds) {
        SearchVerdict::None => Ok(Resolution::NotFound(None)),
        SearchVerdict::Review(order) => Ok(Resolution::Review(
            order
                .into_iter()
                .take(MAX_REVIEW_CANDIDATES)
                .map(|i| to_candidate(i, None, review_reason(&facts, &scored[i])))
                .collect(),
        )),
        SearchVerdict::Strict(i) => {
            // Confirm against the issue list — free for GCD (search
            // results fill its index cache), and reused by the detector.
            let listed = provider
                .list_series_issue_numbers(&scored[i].0.external_id)
                .await?;
            let ov = overlap(local, &listed);
            if ov.is_some_and(|o| o >= MIN_CONFIRM_OVERLAP) {
                let (c, _) = &scored[i];
                return Ok(Resolution::Found(ResolvedLink {
                    external_id: c.external_id.clone(),
                    method: LinkMethod::Search,
                    attested_by: target,
                    name: Some(c.name.clone()),
                    year: c.year,
                    written: false,
                    covered: Some(listed),
                }));
            }
            let reason = match ov {
                Some(o) => format!(
                    "lists only {:.0}% of the local issues (needs {:.0}%)",
                    o * 100.0,
                    MIN_CONFIRM_OVERLAP * 100.0
                ),
                None => "its issue list couldn't be compared".to_owned(),
            };
            Ok(Resolution::Review(vec![to_candidate(i, ov, &reason)]))
        }
    }
}

fn review_reason(facts: &SeriesQueryFacts, (c, s): &(SeriesCandidate, Score)) -> &'static str {
    if matcher::name_similarity(&facts.name, &c.name) < 0.999 {
        "name differs"
    } else if facts.year.is_none() || facts.year != c.year {
        "start year differs"
    } else if s.publisher <= 0.0 {
        "publisher differs"
    } else if s.format_mismatch {
        "format differs"
    } else {
        "several series match equally well"
    }
}

/// Write a bridge / search link onto the series' `external_ids`. `false`
/// when another live series already owns that provider id.
async fn record_link(
    state: &AppState,
    series_id: Uuid,
    source: Source,
    link: &ResolvedLink,
) -> Result<bool, sea_orm::DbErr> {
    let identifier = Identifier::with_canonical_url(source, link.external_id.clone(), "series");
    let (outcome, promoted) = writers::set_external_id_promoting(
        &state.db,
        "series",
        &series_id.to_string(),
        &identifier,
        SetBy::Provider(link.attested_by),
    )
    .await?;
    if promoted > 0 {
        // WP-8.2: a promoted external link is a similar-series signal.
        state.similarity.invalidate_all();
    }
    Ok(!matches!(
        outcome,
        SetExternalIdOutcome::SkippedConflict { .. }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, name: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesCandidate {
        SeriesCandidate {
            source: Source::Gcd,
            external_id: id.into(),
            external_url: None,
            name: name.into(),
            year,
            publisher: publisher.map(str::to_owned),
            issue_count: None,
            cover_image_url: None,
            deck: None,
            alternate_cover_urls: Vec::new(),
            format: None,
        }
    }

    fn facts() -> SeriesQueryFacts {
        SeriesQueryFacts {
            name: "Fantastic Four".into(),
            year: Some(1961),
            publisher: Some("Marvel".into()),
            volume: None,
            format: None,
        }
    }

    fn scored(cands: Vec<SeriesCandidate>) -> Vec<(SeriesCandidate, Score)> {
        let f = facts();
        cands
            .into_iter()
            .map(|c| {
                let s = matcher::score_series(&f, &c);
                (c, s)
            })
            .collect()
    }

    #[test]
    fn exact_name_year_with_unknown_publisher_is_strict() {
        let s = scored(vec![
            cand("1482", "Fantastic Four", Some(1961), None),
            cand("9", "Fantastic Four Annual", Some(1963), None),
        ]);
        assert_eq!(
            classify_search(&facts(), &s, Thresholds::default()),
            SearchVerdict::Strict(0)
        );
    }

    #[test]
    fn two_exact_candidates_are_ambiguous() {
        let s = scored(vec![
            cand("1", "Fantastic Four", Some(1961), None),
            cand("2", "Fantastic Four", Some(1961), Some("Marvel")),
        ]);
        assert!(matches!(
            classify_search(&facts(), &s, Thresholds::default()),
            SearchVerdict::Review(v) if v.len() == 2
        ));
    }

    #[test]
    fn year_off_by_one_is_only_reviewable() {
        let s = scored(vec![cand(
            "1",
            "Fantastic Four",
            Some(1962),
            Some("Marvel"),
        )]);
        assert_eq!(
            classify_search(&facts(), &s, Thresholds::default()),
            SearchVerdict::Review(vec![0])
        );
    }

    #[test]
    fn conflicting_publisher_is_not_strict() {
        let s = scored(vec![cand(
            "1",
            "Fantastic Four",
            Some(1961),
            Some("Dark Horse"),
        )]);
        assert!(!matches!(
            classify_search(&facts(), &s, Thresholds::default()),
            SearchVerdict::Strict(_)
        ));
    }

    #[test]
    fn unrelated_names_yield_nothing() {
        let s = scored(vec![cand("1", "Silver Surfer", Some(1987), None)]);
        assert_eq!(
            classify_search(&facts(), &s, Thresholds::default()),
            SearchVerdict::None
        );
    }

    #[test]
    fn overlap_counts_numbered_local_issues_only() {
        let local = vec![
            LocalIssue::new("1", None),
            LocalIssue::new("2", None),
            LocalIssue::new("500", None),
            LocalIssue::new("Annual 1", None),
        ];
        let listed: Vec<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let o = overlap(&local, &listed).unwrap();
        assert!((o - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(overlap(&local, &[]), None);
    }

    #[test]
    fn identifier_for_reads_cached_payload_tolerantly() {
        let p = serde_json::json!({"identifiers": [
            {"source": "metron", "id": "1711"},
            {"source": "gcd", "id": " 1482 "},
        ]});
        assert_eq!(identifier_for(&p, Source::Gcd), Some("1482".into()));
        assert_eq!(identifier_for(&p, Source::ComicVine), None);
        assert_eq!(identifier_for(&serde_json::json!({}), Source::Gcd), None);
    }
}
