//! Duplicates page (roadmap WP-3.3, audit R15 / UX-9 / DI-21).
//!
//! Routes (admin-only, `api` group):
//!   - `GET    /libraries/{slug}/duplicates` — cursor-paginated duplicate
//!     groups for one library, filterable by `kind`.
//!   - `PUT    /series/{series_slug}/issues/{issue_slug}/duplicate-decision`
//!     — record `keep` or `remove` (soft-remove) for one member.
//!   - `DELETE /series/{series_slug}/issues/{issue_slug}/duplicate-decision`
//!     — clear the decision (and undo a soft-remove).
//!
//! Three grouping rules, all scoped to one library and to live
//! (`removed_at IS NULL`) issues:
//!
//! - **`number`** — issues sharing `(series, sort_number, special_type)`
//!   (more than one). Repacks, re-scans, and `c2c` vs digital copies of the
//!   same issue land here. Issues without a parsed number never group.
//! - **`hash`** — exact `content_hash` matches. With the default
//!   `dedupe_by_content = true` the scanner skips a second identical copy
//!   at ingest, so this group mostly fills in libraries that turned dedupe
//!   off, or where a retag made two rows converge on the same bytes.
//! - **`cover`** — primary-cover perceptual hashes (`issue_cover.phash`)
//!   within Hamming distance ≤ [`COVER_HAMMING_MAX`] inside one series,
//!   clustered transitively. Same threshold as the matcher's
//!   `STRONG_SCORE_THRESH` (ComicTagger ladder).
//!
//! With no `kind` filter every kind is listed and a group whose members are
//! a subset of a stronger group's members (`hash` > `number` > `cover`) is
//! suppressed, so one physical duplicate doesn't show up three times.
//!
//! **Decisions.** `keep` marks a member as reviewed; a group whose live
//! members are all `keep` drops off the list (a new copy with no decision
//! brings it back). `remove` soft-removes the issue (`removed_at = now`) and
//! pins the removal against the scanner's presence-driven restore
//! (`library::reconcile::not_duplicate_removed`). Restore it from the
//! Removed tab or by clearing the decision.
//!
//! Groups are computed for the whole library (window functions for
//! `number`/`hash`, a per-series self-join for `cover`) and **memoised per
//! library** in [`DuplicateGroupsCache`] on `AppState` (audit 2026-11,
//! N-L3): the first page fills the memo, every later page is a binary
//! search over the sorted groups plus the hydrate of one page — O(1) SQL,
//! no re-grouping. `total` and `counts` are read off the memo, so they are
//! not recomputed for pages that don't serialize them. The memo is dropped
//! by a catalogue-changing scan ([`crate::library::scanner`]'s finalize),
//! by a duplicate decision, and by a Removed-tab restore; a ten-minute TTL
//! backstops the write paths that don't invalidate (hash and cover-pHash
//! backfills). Pages are keyset-paginated over a stable sort key so acting
//! on a group between page fetches never skips another.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, issue_duplicate_decision as decision};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, FromQueryResult, Set,
    Statement, Value,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{decode_cursor, encode_cursor};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::api::extractors::Validated;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::state::AppState;
use server_macros::handler;

/// Cover-pHash Hamming distance at or below which two primary covers in
/// the same series are grouped. Mirrors the matcher's
/// `STRONG_SCORE_THRESH` (8 of 64 bits).
pub const COVER_HAMMING_MAX: i32 = 8;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(set_decision, clear_decision))
}

// ───────────────────────────── wire types ─────────────────────────────

/// Which grouping rule produced a duplicate group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateKind {
    /// Exact content-hash match.
    Hash,
    /// Same `(series, sort_number, special_type)`.
    Number,
    /// Primary-cover pHash within Hamming ≤ 8 inside one series.
    Cover,
}

impl DuplicateKind {
    /// Precedence for the all-kinds listing (lower = stronger, listed first).
    fn rank(self) -> u8 {
        match self {
            Self::Hash => 0,
            Self::Number => 1,
            Self::Cover => 2,
        }
    }
}

/// `kind` query filter. `all` (default) lists every kind with
/// subset-suppression; a specific kind lists that kind unsuppressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateKindFilter {
    #[default]
    All,
    Hash,
    Number,
    Cover,
}

/// Admin verdict on one duplicate-group member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateDecision {
    Keep,
    Remove,
}

impl DuplicateDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Remove => "remove",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "keep" => Some(Self::Keep),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DuplicateListQuery {
    #[serde(default)]
    pub kind: DuplicateKindFilter,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Per-kind group counts (first page only) for the filter chips. Counts are
/// unsuppressed — each matches what `?kind=<k>` lists.
#[derive(Debug, Default, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct DuplicateCounts {
    pub hash: u64,
    pub number: u64,
    pub cover: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DuplicateListView {
    pub items: Vec<DuplicateGroupView>,
    pub next_cursor: Option<String>,
    /// Group count for the current `kind` filter. First page only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Per-kind counts for the filter chips. First page only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<DuplicateCounts>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DuplicateGroupView {
    /// Stable group identifier (unique within a listing) — use as a React key.
    pub key: String,
    pub kind: DuplicateKind,
    /// Series the group belongs to. For `hash` groups whose copies span
    /// series this is the first member's series.
    pub series_id: String,
    pub series_slug: String,
    pub series_name: String,
    /// Largest pairwise cover distance inside the group (`cover` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cover_distance: Option<i32>,
    pub issues: Vec<DuplicateIssueView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DuplicateIssueView {
    pub id: String,
    pub slug: String,
    pub series_slug: String,
    pub series_name: String,
    pub number_raw: Option<String>,
    pub title: Option<String>,
    pub special_type: Option<String>,
    pub file_path: String,
    pub file_size: i64,
    pub page_count: Option<i32>,
    pub state: entity::issue::IssueState,
    /// `/issues/{id}/pages/0/thumb` for active issues; `null` otherwise.
    pub cover_url: Option<String>,
    /// The admin's decision so far; `null` when undecided.
    pub decision: Option<DuplicateDecision>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct SetDuplicateDecisionReq {
    #[garde(skip)]
    pub decision: DuplicateDecision,
}

// ───────────────────────────── grouping ─────────────────────────────

/// Per-issue facts the grouping needs (no heavy columns).
#[derive(Debug, Clone, FromQueryResult)]
struct CandidateRow {
    id: String,
    series_id: Uuid,
    series_name: String,
    sort_number: Option<f64>,
    special_type: Option<String>,
    content_hash: String,
    num_dup: bool,
    hash_dup: bool,
}

#[derive(Debug, FromQueryResult)]
struct CoverPairRow {
    a_id: String,
    b_id: String,
    dist: i32,
}

#[derive(Debug, FromQueryResult)]
struct MetaRow {
    id: String,
    series_id: Uuid,
    series_name: String,
    sort_number: Option<f64>,
}

#[derive(Debug, FromQueryResult)]
struct DecisionRow {
    issue_id: String,
    decision: String,
}

/// One computed group, before hydration.
#[derive(Debug, Clone)]
struct Group {
    kind: DuplicateKind,
    series_id: Uuid,
    sort: SortKey,
    members: Vec<String>,
    max_distance: Option<i32>,
}

/// Keyset position. Ordered by (kind rank, series name, lowest number in
/// the group, discriminator) — `key` is unique per group so the order is
/// total. Serialized as the opaque cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SortKey {
    rank: u8,
    series: String,
    num: f64,
    key: String,
}

impl SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank
            .cmp(&other.rank)
            .then_with(|| self.series.cmp(&other.series))
            .then_with(|| self.num.total_cmp(&other.num))
            .then_with(|| self.key.cmp(&other.key))
    }
}

/// Sentinel for "no number" so unnumbered groups sort last in a series.
const NO_NUMBER: f64 = f64::MAX;

async fn load_candidates(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> Result<Vec<CandidateRow>, sea_orm::DbErr> {
    // Window counts over the library's live issues; keep only rows that
    // belong to a number- or hash-group.
    let sql = r"
        SELECT id, series_id, series_name, sort_number, special_type, content_hash,
               num_dup, hash_dup
          FROM (
            SELECT i.id, i.series_id, s.name AS series_name, i.sort_number,
                   i.special_type, i.content_hash,
                   (i.sort_number IS NOT NULL AND count(*) OVER (
                        PARTITION BY i.series_id, i.sort_number,
                                     COALESCE(i.special_type, '')) > 1) AS num_dup,
                   (count(*) OVER (PARTITION BY i.content_hash) > 1) AS hash_dup
              FROM issues i
              JOIN series s ON s.id = i.series_id
             WHERE i.library_id = $1 AND i.removed_at IS NULL
          ) t
         WHERE num_dup OR hash_dup";
    CandidateRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        [library_id.into()],
    ))
    .all(db)
    .await
}

async fn load_cover_pairs(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> Result<Vec<CoverPairRow>, sea_orm::DbErr> {
    // Primary, active, hashed covers; pairs inside one series only. The
    // XOR of two signed 64-bit hashes cast to bit(64) gives the Hamming
    // distance via bit_count (PG 14+).
    let sql = r"
        WITH c AS (
            SELECT i.id, i.series_id, ic.phash
              FROM issues i
              JOIN issue_cover ic ON ic.issue_id = i.id
             WHERE i.library_id = $1 AND i.removed_at IS NULL
               AND ic.kind = 'primary' AND ic.ordinal = 0 AND ic.is_active
               AND ic.phash IS NOT NULL
        )
        SELECT a.id AS a_id, b.id AS b_id,
               bit_count((a.phash # b.phash)::bit(64))::int4 AS dist
          FROM c a
          JOIN c b ON b.series_id = a.series_id AND b.id > a.id
         WHERE bit_count((a.phash # b.phash)::bit(64)) <= $2";
    CoverPairRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        [library_id.into(), COVER_HAMMING_MAX.into()],
    ))
    .all(db)
    .await
}

/// Build `IN ($n, $n+1, …)` placeholders for `ids`, appending the values.
fn in_list(ids: &[String], params: &mut Vec<Value>) -> String {
    ids.iter()
        .map(|id| {
            params.push(Value::from(id.clone()));
            format!("${}", params.len())
        })
        .collect::<Vec<_>>()
        .join(",")
}

async fn load_meta(
    db: &DatabaseConnection,
    ids: &[String],
) -> Result<Vec<MetaRow>, sea_orm::DbErr> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(1000) {
        let mut params = Vec::new();
        let list = in_list(chunk, &mut params);
        let sql = format!(
            "SELECT i.id, i.series_id, s.name AS series_name, i.sort_number \
               FROM issues i JOIN series s ON s.id = i.series_id \
              WHERE i.id IN ({list})"
        );
        out.extend(
            MetaRow::find_by_statement(Statement::from_sql_and_values(
                db.get_database_backend(),
                &sql,
                params,
            ))
            .all(db)
            .await?,
        );
    }
    Ok(out)
}

async fn load_decisions(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> Result<HashMap<String, DuplicateDecision>, sea_orm::DbErr> {
    let rows = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT issue_id, decision FROM issue_duplicate_decision WHERE library_id = $1",
        [library_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| DuplicateDecision::parse(&r.decision).map(|d| (r.issue_id, d)))
        .collect())
}

/// Minimal union-find over string ids for transitive cover clustering.
struct Dsu {
    parent: HashMap<String, String>,
}

impl Dsu {
    fn new() -> Self {
        Self {
            parent: HashMap::new(),
        }
    }
    fn find(&mut self, x: &str) -> String {
        let p = self
            .parent
            .entry(x.to_owned())
            .or_insert_with(|| x.to_owned())
            .clone();
        if p == x {
            return p;
        }
        let root = self.find(&p);
        self.parent.insert(x.to_owned(), root.clone());
        root
    }
    fn union(&mut self, a: &str, b: &str) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            // Deterministic: smaller id becomes the root.
            if ra < rb {
                self.parent.insert(rb, ra);
            } else {
                self.parent.insert(ra, rb);
            }
        }
    }
}

// ───────────────────────────── memo ─────────────────────────────

/// Every list the endpoint can serve for one library, sorted by
/// [`SortKey`]: the all-kinds listing with subset suppression applied and
/// the three unsuppressed per-kind listings. Built once per memo fill by
/// [`compute_groups`]; a page request only slices it.
pub struct ComputedGroups {
    /// Every group, sorted by [`SortKey`]. The four listings below are
    /// ascending indexes into it, so each group is stored once.
    groups: Vec<Group>,
    suppressed: Vec<u32>,
    hash: Vec<u32>,
    number: Vec<u32>,
    cover: Vec<u32>,
    counts: DuplicateCounts,
}

impl ComputedGroups {
    fn list(&self, filter: DuplicateKindFilter) -> &[u32] {
        match filter {
            DuplicateKindFilter::All => &self.suppressed,
            DuplicateKindFilter::Hash => &self.hash,
            DuplicateKindFilter::Number => &self.number,
            DuplicateKindFilter::Cover => &self.cover,
        }
    }

    fn group(&self, index: u32) -> &Group {
        &self.groups[index as usize]
    }
}

/// Libraries whose groups stay memoised at once. A library's memo is a few
/// strings per duplicate group, so even a 50k-issue library is small; the
/// bound is there so an instance with hundreds of libraries can't grow
/// without limit.
const CACHE_LIBRARIES: usize = 16;

/// Backstop for write paths that don't invalidate explicitly (the
/// first-import lazy hash backfill, a cover pHash persisted by the
/// thumbnail worker, a DB-direct provider apply in a non-writeback
/// library): a memo older than this is recomputed.
pub const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

struct CacheEntry {
    generation: u64,
    at: Instant,
    computed: Arc<ComputedGroups>,
}

struct CacheInner {
    /// Per-library invalidation counter. Bumped by [`invalidate`]; a memo
    /// built under an older generation is never served or stored.
    generations: HashMap<Uuid, u64>,
    lru: LruCache<Uuid, CacheEntry>,
}

/// Process-local memo of [`ComputedGroups`] per library (see the module
/// docs). Lives on `AppState::duplicates`.
pub struct DuplicateGroupsCache {
    inner: Mutex<CacheInner>,
}

impl Default for DuplicateGroupsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DuplicateGroupsCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                generations: HashMap::new(),
                lru: LruCache::new(NonZeroUsize::new(CACHE_LIBRARIES).expect("nonzero")),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        // Cache data only: recover from a poisoned lock instead of failing
        // every later request.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop the memo for one library. Called when a scan changed the
    /// library, when a duplicate decision is recorded or cleared, and when
    /// a removed issue or series is restored.
    pub fn invalidate(&self, library_id: Uuid) {
        let mut g = self.lock();
        *g.generations.entry(library_id).or_insert(0) += 1;
        g.lru.pop(&library_id);
    }

    /// Drop every library's memo. For writers that touch grouping inputs
    /// across libraries at once (the cover-pHash backfill drain).
    pub fn invalidate_all(&self) {
        let mut g = self.lock();
        // Bumping every counter retires computes already in flight; the
        // clear covers libraries that have no counter yet.
        for generation in g.generations.values_mut() {
            *generation += 1;
        }
        g.lru.clear();
    }

    /// Current generation for `library_id` (bumped by every invalidation).
    pub fn generation(&self, library_id: Uuid) -> u64 {
        // Materialise the counter so an `invalidate_all` that lands while
        // this library's first compute is in flight retires that compute.
        *self.lock().generations.entry(library_id).or_insert(0)
    }

    /// Whether a live memo exists for `library_id` (tests / diagnostics).
    pub fn is_cached(&self, library_id: Uuid) -> bool {
        self.lock()
            .lru
            .peek(&library_id)
            .is_some_and(|e| e.at.elapsed() < CACHE_TTL)
    }

    fn get(&self, library_id: Uuid) -> Option<Arc<ComputedGroups>> {
        let mut g = self.lock();
        let generation = g.generations.get(&library_id).copied().unwrap_or(0);
        let hit = g
            .lru
            .get(&library_id)
            .filter(|e| e.generation == generation && e.at.elapsed() < CACHE_TTL)
            .map(|e| e.computed.clone());
        if hit.is_none() {
            g.lru.pop(&library_id);
        }
        hit
    }

    /// Store a memo computed under `generation`; dropped when an
    /// invalidation landed mid-compute so a stale memo can't outlive it.
    fn put(&self, library_id: Uuid, generation: u64, computed: Arc<ComputedGroups>) {
        let mut g = self.lock();
        if g.generations.get(&library_id).copied().unwrap_or(0) != generation {
            return;
        }
        g.lru.put(
            library_id,
            CacheEntry {
                generation,
                at: Instant::now(),
                computed,
            },
        );
    }
}

/// Memo hit, or compute + store. Concurrent misses each compute; the
/// generation check in `put` keeps a stale result from winning.
async fn memoised_groups(
    app: &AppState,
    library_id: Uuid,
) -> Result<Arc<ComputedGroups>, sea_orm::DbErr> {
    if let Some(hit) = app.duplicates.get(library_id) {
        return Ok(hit);
    }
    let generation = app.duplicates.generation(library_id);
    let computed = Arc::new(compute_groups(&app.db, library_id).await?);
    app.duplicates.put(library_id, generation, computed.clone());
    Ok(computed)
}

/// Compute every duplicate group for `library_id`, sorted, as the memo
/// shape [`ComputedGroups`]. Three whole-library statements (candidates,
/// cover pairs, decisions) plus the cover members' meta — O(library),
/// which is why the result is memoised.
async fn compute_groups(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> Result<ComputedGroups, sea_orm::DbErr> {
    let candidates = load_candidates(db, library_id).await?;
    let pairs = load_cover_pairs(db, library_id).await?;
    let decisions = load_decisions(db, library_id).await?;

    let mut groups: Vec<Group> = Vec::new();

    // number + hash groups from the candidate rows.
    let mut by_number: HashMap<(Uuid, u64, String), Vec<&CandidateRow>> = HashMap::new();
    let mut by_hash: HashMap<&str, Vec<&CandidateRow>> = HashMap::new();
    for r in &candidates {
        if r.num_dup
            && let Some(n) = r.sort_number
        {
            by_number
                .entry((
                    r.series_id,
                    n.to_bits(),
                    r.special_type.clone().unwrap_or_default(),
                ))
                .or_default()
                .push(r);
        }
        if r.hash_dup {
            by_hash.entry(r.content_hash.as_str()).or_default().push(r);
        }
    }
    for ((series_id, bits, special), rows) in by_number {
        let first = rows[0];
        let num = f64::from_bits(bits);
        groups.push(Group {
            kind: DuplicateKind::Number,
            series_id,
            sort: SortKey {
                rank: DuplicateKind::Number.rank(),
                series: first.series_name.to_lowercase(),
                num,
                key: format!("number:{series_id}:{num}:{special}"),
            },
            members: rows.iter().map(|r| r.id.clone()).collect(),
            max_distance: None,
        });
    }
    for (hash, mut rows) in by_hash {
        // Deterministic "home" series: the member with the smallest id.
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let first = rows[0];
        groups.push(Group {
            kind: DuplicateKind::Hash,
            series_id: first.series_id,
            sort: SortKey {
                rank: DuplicateKind::Hash.rank(),
                series: first.series_name.to_lowercase(),
                num: rows
                    .iter()
                    .filter_map(|r| r.sort_number)
                    .fold(NO_NUMBER, f64::min),
                key: format!("hash:{hash}"),
            },
            members: rows.iter().map(|r| r.id.clone()).collect(),
            max_distance: None,
        });
    }

    // cover groups: transitive clusters of close pairs.
    if !pairs.is_empty() {
        let mut dsu = Dsu::new();
        for p in &pairs {
            dsu.union(&p.a_id, &p.b_id);
        }
        let mut clusters: HashMap<String, BTreeSet<String>> = HashMap::new();
        let ids: Vec<String> = dsu.parent.keys().cloned().collect();
        for id in &ids {
            let root = dsu.find(id);
            clusters.entry(root).or_default().insert(id.clone());
        }
        let mut max_dist: HashMap<String, i32> = HashMap::new();
        for p in &pairs {
            let root = dsu.find(&p.a_id);
            let e = max_dist.entry(root).or_insert(0);
            *e = (*e).max(p.dist);
        }
        let meta: HashMap<String, MetaRow> = load_meta(db, &ids)
            .await?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        for (root, members) in clusters {
            let Some(first) = members.iter().find_map(|id| meta.get(id)) else {
                continue;
            };
            groups.push(Group {
                kind: DuplicateKind::Cover,
                series_id: first.series_id,
                sort: SortKey {
                    rank: DuplicateKind::Cover.rank(),
                    series: first.series_name.to_lowercase(),
                    num: members
                        .iter()
                        .filter_map(|id| meta.get(id).and_then(|m| m.sort_number))
                        .fold(NO_NUMBER, f64::min),
                    key: format!("cover:{root}"),
                },
                members: members.into_iter().collect(),
                max_distance: max_dist.get(&root).copied(),
            });
        }
    }

    // Stable member order + drop fully-reviewed groups (every live member
    // marked `keep`).
    for g in &mut groups {
        g.members.sort();
    }
    groups.retain(|g| {
        !g.members
            .iter()
            .all(|id| decisions.get(id) == Some(&DuplicateDecision::Keep))
    });

    let mut counts = DuplicateCounts::default();
    for g in &groups {
        match g.kind {
            DuplicateKind::Hash => counts.hash += 1,
            DuplicateKind::Number => counts.number += 1,
            DuplicateKind::Cover => counts.cover += 1,
        }
    }

    groups.sort_by(|a, b| a.sort.cmp(&b.sort));
    let by_kind = |kind: DuplicateKind| -> Vec<u32> {
        groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind == kind)
            .map(|(i, _)| i as u32)
            .collect()
    };
    Ok(ComputedGroups {
        hash: by_kind(DuplicateKind::Hash),
        number: by_kind(DuplicateKind::Number),
        cover: by_kind(DuplicateKind::Cover),
        suppressed: suppress_subsets(&groups),
        counts,
        groups,
    })
}

/// All-kinds listing: the indexes of the groups that survive once a group
/// whose members are a subset of a stronger-kind group's members
/// (`hash` > `number` > `cover`) is dropped.
fn suppress_subsets(groups: &[Group]) -> Vec<u32> {
    let stronger: Vec<(u8, HashSet<&str>)> = groups
        .iter()
        .map(|g| {
            (
                g.kind.rank(),
                g.members.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    groups
        .iter()
        .enumerate()
        .filter(|(_, g)| {
            let rank = g.kind.rank();
            !stronger
                .iter()
                .any(|(r, set)| *r < rank && g.members.iter().all(|m| set.contains(m.as_str())))
        })
        .map(|(i, _)| i as u32)
        .collect()
}

// ───────────────────────────── hydration ─────────────────────────────

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    id: String,
    slug: String,
    series_slug: String,
    series_name: String,
    number_raw: Option<String>,
    title: Option<String>,
    special_type: Option<String>,
    file_path: String,
    file_size: i64,
    page_count: Option<i32>,
    state: entity::issue::IssueState,
    decision: Option<String>,
}

async fn hydrate(
    db: &DatabaseConnection,
    groups: Vec<Group>,
) -> Result<Vec<DuplicateGroupView>, sea_orm::DbErr> {
    let ids: Vec<String> = groups
        .iter()
        .flat_map(|g| g.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let series_ids: Vec<Uuid> = groups
        .iter()
        .map(|g| g.series_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut rows: HashMap<String, IssueRow> = HashMap::new();
    for chunk in ids.chunks(1000) {
        let mut params = Vec::new();
        let list = in_list(chunk, &mut params);
        let sql = format!(
            "SELECT i.id, i.slug, s.slug AS series_slug, s.name AS series_name, \
                    i.number_raw, i.title, i.special_type, i.file_path, i.file_size, \
                    i.page_count, i.state, d.decision \
               FROM issues i \
               JOIN series s ON s.id = i.series_id \
               LEFT JOIN issue_duplicate_decision d ON d.issue_id = i.id \
              WHERE i.id IN ({list})"
        );
        for r in IssueRow::find_by_statement(Statement::from_sql_and_values(
            db.get_database_backend(),
            &sql,
            params,
        ))
        .all(db)
        .await?
        {
            rows.insert(r.id.clone(), r);
        }
    }
    let mut series: HashMap<Uuid, (String, String)> = HashMap::new();
    if !series_ids.is_empty() {
        let mut params: Vec<Value> = Vec::new();
        let list = series_ids
            .iter()
            .map(|id| {
                params.push(Value::from(*id));
                format!("${}", params.len())
            })
            .collect::<Vec<_>>()
            .join(",");
        #[derive(FromQueryResult)]
        struct SeriesRow {
            id: Uuid,
            slug: String,
            name: String,
        }
        for s in SeriesRow::find_by_statement(Statement::from_sql_and_values(
            db.get_database_backend(),
            format!("SELECT id, slug, name FROM series WHERE id IN ({list})"),
            params,
        ))
        .all(db)
        .await?
        {
            series.insert(s.id, (s.slug, s.name));
        }
    }

    Ok(groups
        .into_iter()
        .map(|g| {
            let (series_slug, series_name) = series.get(&g.series_id).cloned().unwrap_or_default();
            let issues = g
                .members
                .iter()
                .filter_map(|id| rows.get(id))
                .map(|r| DuplicateIssueView {
                    id: r.id.clone(),
                    slug: r.slug.clone(),
                    series_slug: r.series_slug.clone(),
                    series_name: r.series_name.clone(),
                    number_raw: r.number_raw.clone(),
                    title: r.title.clone(),
                    special_type: r.special_type.clone(),
                    file_path: r.file_path.clone(),
                    file_size: r.file_size,
                    page_count: r.page_count,
                    state: r.state,
                    cover_url: (r.state == entity::issue::IssueState::Active)
                        .then(|| format!("/issues/{}/pages/0/thumb", r.id)),
                    decision: r.decision.as_deref().and_then(DuplicateDecision::parse),
                })
                .collect();
            DuplicateGroupView {
                key: g.sort.key,
                kind: g.kind,
                series_id: g.series_id.to_string(),
                series_slug,
                series_name,
                max_cover_distance: g.max_distance,
                issues,
            }
        })
        .collect())
}

// ───────────────────────────── handlers ─────────────────────────────

fn internal(e: &sea_orm::DbErr, what: &str) -> Response {
    tracing::error!(error = %e, "{what}");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

#[utoipa::path(
    operation_id = "duplicates_list",
    get,
    path = "/libraries/{slug}/duplicates",
    params(
        ("slug" = String, Path,),
        ("kind" = Option<String>, Query, description = "Grouping rule filter: `all` (default), `hash`, `number`, or `cover`."),
        ("limit" = Option<u64>, Query, description = "Groups per page (1..=200, default 50)."),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = DuplicateListView),
        (status = 400, description = "invalid cursor or kind"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<DuplicateListQuery>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    let cursor: Option<SortKey> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<SortKey>(c) {
            Ok(k) => Some(k),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation.cursor",
                    "invalid cursor",
                );
            }
        },
    };
    let first_page = cursor.is_none();

    let computed = match memoised_groups(&app, lib.id).await {
        Ok(v) => v,
        Err(e) => return internal(&e, "duplicates: grouping failed"),
    };
    let listing = computed.list(q.kind);
    // Keyset: the listing is sorted by `SortKey`, so the page starts at
    // the first group strictly after the cursor.
    let start = match &cursor {
        Some(c) => listing.partition_point(|&i| computed.group(i).sort.cmp(c) != Ordering::Greater),
        None => 0,
    };
    let after = &listing[start..];
    let has_more = after.len() > limit;
    let page: Vec<Group> = after
        .iter()
        .take(limit)
        .map(|&i| computed.group(i).clone())
        .collect();
    let next_cursor = if has_more {
        page.last().and_then(|g| encode_cursor(&g.sort).ok())
    } else {
        None
    };
    let items = match hydrate(&app.db, page).await {
        Ok(v) => v,
        Err(e) => return internal(&e, "duplicates: hydrate failed"),
    };
    Json(DuplicateListView {
        items,
        next_cursor,
        total: first_page.then_some(listing.len() as u64),
        counts: first_page.then_some(computed.counts),
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "duplicates_set_decision",
    put,
    path = "/series/{series_slug}/issues/{issue_slug}/duplicate-decision",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = SetDuplicateDecisionReq,
    responses(
        (status = 204, description = "decision recorded"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn set_decision(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Validated(req): Validated<SetDuplicateDecisionReq>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now = chrono::Utc::now().fixed_offset();
    let upsert = Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "INSERT INTO issue_duplicate_decision (issue_id, library_id, decision, decided_by, decided_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (issue_id) DO UPDATE \
            SET decision = EXCLUDED.decision, decided_by = EXCLUDED.decided_by, \
                decided_at = EXCLUDED.decided_at",
        [
            row.id.clone().into(),
            row.library_id.into(),
            req.decision.as_str().into(),
            actor.id.into(),
            now.into(),
        ],
    );
    if let Err(e) = app.db.execute_raw(upsert).await {
        return internal(&e, "duplicates: decision upsert failed");
    }

    let issue_id = row.id.clone();
    let library_id = row.library_id;
    let series_id = row.series_id;
    let was_removed = row.removed_at.is_some();
    match req.decision {
        DuplicateDecision::Remove if !was_removed => {
            let mut am: issue::ActiveModel = row.into();
            am.removed_at = Set(Some(now));
            am.removal_confirmed_at = Set(None);
            if let Err(e) = am.update(&app.db).await {
                // The decision row landed, so the memo is stale even now.
                app.duplicates.invalidate(library_id);
                return internal(&e, "duplicates: soft-remove failed");
            }
            recompute_series_status(&app.db, series_id).await;
        }
        _ => {}
    }
    // A `keep` can retire a fully-reviewed group and a `remove` takes a
    // member out of every group it was in — the memo is stale either way.
    // After the last write, so a list that recomputed mid-handler can't
    // store the pre-write state.
    app.duplicates.invalidate(library_id);

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = match req.decision {
            DuplicateDecision::Keep => "admin.issue.duplicate.keep",
            DuplicateDecision::Remove => "admin.issue.duplicate.remove",
        },
        target = ("issue", issue_id),
        payload = serde_json::json!({
            "series_slug": series_slug,
            "issue_slug": issue_slug,
            "decision": req.decision.as_str(),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "duplicates_clear_decision",
    delete,
    path = "/series/{series_slug}/issues/{issue_slug}/duplicate-decision",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 204, description = "decision cleared; a duplicate soft-remove is undone"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn clear_decision(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let prior = match decision::Entity::find_by_id(row.id.clone())
        .one(&app.db)
        .await
    {
        Ok(p) => p,
        Err(e) => return internal(&e, "duplicates: decision lookup failed"),
    };
    if let Err(e) = decision::Entity::delete_by_id(row.id.clone())
        .exec(&app.db)
        .await
    {
        return internal(&e, "duplicates: decision delete failed");
    }
    let issue_id = row.id.clone();
    let library_id = row.library_id;
    let series_id = row.series_id;
    // Undo a Duplicates-page soft-remove. Only when the file is still on
    // disk — a missing file stays removed (the reconcile pass would
    // re-remove it anyway).
    let undo_remove = prior.as_ref().is_some_and(|p| p.decision == "remove")
        && row.removed_at.is_some()
        && std::path::Path::new(&row.file_path).exists();
    if undo_remove {
        let mut am: issue::ActiveModel = row.into();
        am.removed_at = Set(None);
        am.removal_confirmed_at = Set(None);
        if let Err(e) = am.update(&app.db).await {
            app.duplicates.invalidate(library_id);
            return internal(&e, "duplicates: undo soft-remove failed");
        }
        recompute_series_status(&app.db, series_id).await;
    }
    // After the last write (see `set_decision`).
    app.duplicates.invalidate(library_id);

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.issue.duplicate.clear",
        target = ("issue", issue_id),
        payload = serde_json::json!({
            "series_slug": series_slug,
            "issue_slug": issue_slug,
            "previous": prior.map(|p| p.decision),
            "restored": undo_remove,
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Issue counts moved — refresh the series' `total_issues` / status.
async fn recompute_series_status(db: &DatabaseConnection, series_id: Uuid) {
    if let Err(e) =
        crate::library::scanner::reconcile_status::reconcile_series_status(db, series_id, None)
            .await
    {
        tracing::warn!(error = %e, %series_id, "duplicates: series status recompute failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(kind: DuplicateKind, members: &[&str]) -> Group {
        Group {
            kind,
            series_id: Uuid::nil(),
            sort: SortKey {
                rank: kind.rank(),
                series: String::new(),
                num: 0.0,
                key: format!("{kind:?}:{}", members.join(",")),
            },
            members: members.iter().map(|s| (*s).to_owned()).collect(),
            max_distance: None,
        }
    }

    #[test]
    fn subset_of_stronger_kind_is_suppressed() {
        let groups = vec![
            group(DuplicateKind::Hash, &["a", "b"]),
            group(DuplicateKind::Number, &["a", "b"]),
            group(DuplicateKind::Number, &["a", "b", "c"]),
            group(DuplicateKind::Cover, &["b", "c"]),
            group(DuplicateKind::Cover, &["d", "e"]),
        ];
        let out: Vec<&Group> = suppress_subsets(&groups)
            .into_iter()
            .map(|i| &groups[i as usize])
            .collect();
        let kinds: Vec<(DuplicateKind, usize)> =
            out.iter().map(|g| (g.kind, g.members.len())).collect();
        assert_eq!(
            kinds,
            vec![
                (DuplicateKind::Hash, 2),
                (DuplicateKind::Number, 3),
                (DuplicateKind::Cover, 2),
            ]
        );
        assert_eq!(out[2].members, vec!["d", "e"]);
    }

    #[test]
    fn sort_key_orders_by_rank_series_number_key() {
        let k = |rank, series: &str, num, key: &str| SortKey {
            rank,
            series: series.into(),
            num,
            key: key.into(),
        };
        assert_eq!(
            k(0, "z", 9.0, "a").cmp(&k(1, "a", 1.0, "a")),
            Ordering::Less
        );
        assert_eq!(
            k(1, "a", 2.0, "a").cmp(&k(1, "a", 10.0, "a")),
            Ordering::Less
        );
        assert_eq!(
            k(1, "a", 2.0, "b").cmp(&k(1, "a", 2.0, "a")),
            Ordering::Greater
        );
    }
}
