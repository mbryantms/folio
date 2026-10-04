//! External (not-in-library) relationship targets and provider links
//! (WP-7.8).
//!
//! `series_external_relationship` holds "this series `kind` *a provider
//! series*" rows for series the library doesn't have — "Continued by: Saga
//! (2018), not in your library". Rows are one-directional (no inverse
//! half). Two writers:
//!
//! - [`record_provider_links`] — the series apply path stores a provider's
//!   linked series (Metron `associated`) as `set_by = 'provider'` rows,
//!   with a kind refined from the two series types ([`provider_kind`]).
//!   User rows and dismissed rows are never touched.
//! - [`create_user_link`] / [`remove_link`] — the admin API.
//!
//! **Promotion.** When the target provider series is matched to a local
//! series (an `external_ids` row with the same source and id, or through
//! the id bridge: the provider's cached detail lists the other providers'
//! ids), the external row becomes internal:
//!
//! - a `user` row becomes a manual [`super::create_pair_scoped`] edge and
//!   is deleted;
//! - a `provider` row is marked (`promoted_series_id`) and the suggestion
//!   engine's `provider_associated` source proposes the pair from then on
//!   (suggestions only: nothing is created without review).
//!
//! Promotion runs from [`crate::metadata::writers::set_external_id`] (the
//! moment a series gains a provider id — [`promote_for_provider_id`]) and
//! from every suggestion run ([`promote_library`]); readers also resolve
//! lazily ([`resolve_rows`]), so a missed hook degrades to "shown with a
//! link to the local series" until the next run.
//!
//! See `docs/dev/series-relationships.md` ("Provider links and external
//! targets").

use super::{PairError, RelationshipKind, RelationshipSource, Scope};
use crate::metadata::identifier::Source;
use crate::metadata::provider::ProviderSeriesRef;
use crate::metadata::title_norm::{FormatClass, classify_format, infer_format_from_title};
use chrono::Utc;
use entity::series_external_relationship as ext;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, ExprTrait, FromQueryResult,
    QueryFilter, Statement, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Who made an external row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSetBy {
    /// An admin added it.
    User,
    /// Provider data (Metron `associated`).
    Provider,
}

impl ExternalSetBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Provider => "provider",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "user" {
            Self::User
        } else {
            Self::Provider
        }
    }
}

/// The providers an external row may point at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSource {
    Metron,
    Comicvine,
    Gcd,
}

impl ExternalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metron => "metron",
            Self::Comicvine => "comicvine",
            Self::Gcd => "gcd",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Metron => "Metron",
            Self::Comicvine => "ComicVine",
            Self::Gcd => "GCD",
        }
    }

    pub fn source(self) -> Source {
        match self {
            Self::Metron => Source::Metron,
            Self::Comicvine => Source::ComicVine,
            Self::Gcd => Source::Gcd,
        }
    }

    pub fn from_source(s: Source) -> Option<Self> {
        match s {
            Source::Metron => Some(Self::Metron),
            Source::ComicVine => Some(Self::Comicvine),
            Source::Gcd => Some(Self::Gcd),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "metron" => Some(Self::Metron),
            "comicvine" => Some(Self::Comicvine),
            "gcd" => Some(Self::Gcd),
            _ => None,
        }
    }

    /// Canonical provider page for a series id.
    pub fn series_url(self, id: &str) -> Option<String> {
        crate::metadata::identifier::canonical_url(self.source(), "series", id)
    }
}

// ───── kind refinement ─────

/// Confidence of a plain `see_also` from an untyped provider link.
pub const ASSOCIATED_SEE_ALSO: f32 = 0.6;
/// … refined to `collects` / `annual_of` by the series types.
pub const ASSOCIATED_TYPED: f32 = 0.72;
/// … refined, but one side's type is only inferred (no type, name gives
/// no marker).
pub const ASSOCIATED_TYPED_ONE_SIDE: f32 = 0.66;

/// Coarse class of a series from its provider `series_type` (Metron:
/// "Trade Paperback", "Annual Series", "Ongoing Series", …), falling back
/// to name markers ("… TPB", "… Omnibus", "… Annual").
pub fn series_class(series_type: Option<&str>, name: &str) -> Option<FormatClass> {
    if let Some(c) = series_type.and_then(classify_format) {
        return Some(c);
    }
    if let Some(c) = infer_format_from_title(name, None).and_then(classify_format) {
        return Some(c);
    }
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        // A trailing year ("X Annual 2019") doesn't hide the marker.
        .filter(|w| !(w.len() == 4 && w.chars().all(|c| c.is_ascii_digit())))
        .map(str::to_owned)
        .collect();
    if words.len() >= 2 && matches!(words.last().map(String::as_str), Some("annual" | "annuals")) {
        return Some(FormatClass::Annual);
    }
    None
}

/// Kind and confidence for an untyped, symmetric provider link "A is
/// associated with B", read from A's side:
///
/// - A collected edition (TPB / HC / GN / omnibus) and B a periodical
///   (ongoing / limited / one-shot) → A `collects` B (the edition collects
///   the singles); the other way round → A `collected_in` B;
/// - A an annual series and B a periodical → A `annual_of` B (and
///   `has_annual` the other way);
/// - anything else (both editions, both periodicals, unknown) →
///   `see_also`.
///
/// A periodical side may be unknown (no type, no name marker): the refined
/// kind then gets [`ASSOCIATED_TYPED_ONE_SIDE`].
pub fn provider_kind(
    a_type: Option<&str>,
    a_name: &str,
    b_type: Option<&str>,
    b_name: &str,
) -> (RelationshipKind, f32) {
    use FormatClass as F;
    let a = series_class(a_type, a_name);
    let b = series_class(b_type, b_name);
    let conf = |other: Option<F>| {
        if other == Some(F::Single) {
            ASSOCIATED_TYPED
        } else {
            ASSOCIATED_TYPED_ONE_SIDE
        }
    };
    match (a, b) {
        (Some(F::Collected), Some(F::Single) | None) => (RelationshipKind::Collects, conf(b)),
        (Some(F::Single) | None, Some(F::Collected)) => (RelationshipKind::CollectedIn, conf(a)),
        (Some(F::Annual), Some(F::Single) | None) => (RelationshipKind::AnnualOf, conf(b)),
        (Some(F::Single) | None, Some(F::Annual)) => (RelationshipKind::HasAnnual, conf(a)),
        _ => (RelationshipKind::SeeAlso, ASSOCIATED_SEE_ALSO),
    }
}

// ───── provider writes ─────

#[derive(Debug, FromQueryResult)]
struct CachedType {
    series_type: Option<String>,
}

/// The provider series type of `(source, id)` from the metadata cache, when
/// that series' detail was ever fetched. No network call.
async fn cached_series_type<C: ConnectionTrait>(
    conn: &C,
    source: &str,
    id: &str,
) -> Result<Option<String>, DbErr> {
    Ok(
        CachedType::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT payload->>'series_type' AS series_type FROM metadata_cache \
          WHERE provider = $1 AND entity = 'series' AND external_id = $2",
            [source.into(), id.into()],
        ))
        .one(conn)
        .await?
        .and_then(|r| r.series_type),
    )
}

/// The `series_type` of the local series matched to `(source, id)`, if any.
async fn local_series_type<C: ConnectionTrait>(
    conn: &C,
    source: &str,
    id: &str,
) -> Result<Option<String>, DbErr> {
    Ok(
        CachedType::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT s.series_type FROM external_ids x \
           JOIN series s ON s.id::text = x.entity_id AND s.removed_at IS NULL \
          WHERE x.entity_type = 'series' AND x.source = $1 AND x.external_id = $2 \
          LIMIT 1",
            [source.into(), id.into()],
        ))
        .one(conn)
        .await?
        .and_then(|r| r.series_type),
    )
}

/// What [`record_provider_links`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProviderLinkReport {
    /// Rows inserted or refreshed.
    pub upserted: usize,
    /// Old provider rows of the same source the provider no longer lists.
    pub removed: usize,
    /// Rows promoted right away (their target is already local).
    pub promoted: usize,
    /// … of which became series relationships (WP-8.2: the caller
    /// invalidates the similar-series cache when non-zero).
    pub pairs_created: usize,
}

/// Store a provider's linked series for `series` as `set_by = 'provider'`
/// external rows (WP-7.8). `source` is the provider the links came from;
/// `own_id` is the series' own id at that provider (self-links are
/// skipped); `own_type` its provider series type.
///
/// Each link gets a kind from [`provider_kind`] (B's type comes from the
/// metadata cache when its detail was ever fetched, else its name).
/// Rows are upserted on `(from, kind, source, provider_series_id)`; a
/// conflicting **user** or **dismissed** row is left alone (user > provider;
/// a dismissal is rejection memory). Provider rows of the same source that
/// the provider no longer lists are deleted (not dismissed ones). Then the
/// rows are resolved against the library ([`promote_series_rows`]).
///
/// Not a ComicInfo / MetronInfo field: called from both apply paths
/// (DB-direct and sidecar writeback) after the series scalars, like other
/// metadata-only rows.
pub async fn record_provider_links<C: ConnectionTrait>(
    conn: &C,
    series: &entity::series::Model,
    source: Source,
    own_id: Option<&str>,
    own_type: Option<&str>,
    links: &[ProviderSeriesRef],
) -> Result<ProviderLinkReport, DbErr> {
    let mut report = ProviderLinkReport::default();
    let Some(src) = ExternalSource::from_source(source) else {
        return Ok(report);
    };
    let now = Utc::now().fixed_offset();
    let mut keep: HashSet<(String, String)> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    for link in links.iter().filter(|l| l.source == source) {
        let pid = link.id.trim();
        if pid.is_empty() || pid.len() > 64 || own_id == Some(pid) || !seen.insert(pid.to_owned()) {
            continue;
        }
        let b_type = match cached_series_type(conn, src.as_str(), pid).await? {
            Some(t) => Some(t),
            None => local_series_type(conn, src.as_str(), pid).await?,
        };
        let (kind, confidence) =
            provider_kind(own_type, &series.name, b_type.as_deref(), &link.name);
        keep.insert((kind.as_str().to_owned(), pid.to_owned()));
        let name: String = link.name.chars().take(300).collect();
        let url = link
            .url
            .clone()
            .or_else(|| src.series_url(pid))
            .filter(|u| u.len() <= 500);
        let ids: Vec<&str> = [own_id, Some(pid)].into_iter().flatten().collect();
        let evidence = serde_json::json!({
            "source": src.as_str(),
            "field": "associated",
            "ids": ids,
            "label": link.label,
            "series_type": b_type,
        });
        let am = ext::ActiveModel {
            id: Set(Uuid::now_v7()),
            from_series_id: Set(series.id),
            kind: Set(kind.as_str().to_owned()),
            qualifier: Set(None),
            source: Set(src.as_str().to_owned()),
            provider_series_id: Set(pid.to_owned()),
            provider_series_name: Set(Some(name).filter(|n| !n.is_empty())),
            provider_series_url: Set(url),
            provider_year: Set(link.year),
            set_by: Set(ExternalSetBy::Provider.as_str().to_owned()),
            confidence: Set(Some(confidence)),
            evidence: Set(evidence),
            created_by: Set(None),
            promoted_series_id: Set(None),
            dismissed_at: Set(None),
            dismissed_by: Set(None),
            first_set_at: Set(now),
            last_synced_at: Set(now),
        };
        // Refresh only a live provider row: a user row (the admin's
        // claim) and a dismissed row (rejection memory) keep their values.
        let res = ext::Entity::insert(am)
            .on_conflict(
                OnConflict::columns([
                    ext::Column::FromSeriesId,
                    ext::Column::Kind,
                    ext::Column::Source,
                    ext::Column::ProviderSeriesId,
                ])
                .update_columns([
                    ext::Column::ProviderSeriesName,
                    ext::Column::ProviderSeriesUrl,
                    ext::Column::ProviderYear,
                    ext::Column::Confidence,
                    ext::Column::Evidence,
                    ext::Column::LastSyncedAt,
                ])
                .action_and_where(
                    sea_orm::sea_query::Expr::col((ext::Entity, ext::Column::SetBy))
                        .eq("provider")
                        .and(
                            sea_orm::sea_query::Expr::col((ext::Entity, ext::Column::DismissedAt))
                                .is_null(),
                        ),
                )
                .to_owned(),
            )
            .exec_without_returning(conn)
            .await?;
        report.upserted += usize::try_from(res).unwrap_or(0);
    }
    // Drop provider rows of this source the provider stopped listing — the
    // `associated` ones only; coverage rows ([`record_coverage_links`])
    // have their own lifecycle. A live coverage row for a series the
    // provider now links itself is superseded (no duplicate target).
    let linked: HashSet<&str> = keep.iter().map(|(_, pid)| pid.as_str()).collect();
    let stale: Vec<Uuid> = ext::Entity::find()
        .filter(ext::Column::FromSeriesId.eq(series.id))
        .filter(ext::Column::Source.eq(src.as_str()))
        .filter(ext::Column::SetBy.eq("provider"))
        .filter(ext::Column::DismissedAt.is_null())
        .all(conn)
        .await?
        .into_iter()
        .filter(|r| {
            if is_coverage_row(r) {
                linked.contains(r.provider_series_id.as_str())
            } else {
                !keep.contains(&(r.kind.clone(), r.provider_series_id.clone()))
            }
        })
        .map(|r| r.id)
        .collect();
    if !stale.is_empty() {
        report.removed = usize::try_from(
            ext::Entity::delete_many()
                .filter(ext::Column::Id.is_in(stale))
                .exec(conn)
                .await?
                .rows_affected,
        )
        .unwrap_or(0);
    }
    let p = promote_series_rows(conn, series.id).await?;
    report.promoted = p.total();
    report.pairs_created = p.pairs_created;
    Ok(report)
}

// ───── coverage links (coverage tie-ins PR 4) ─────

/// `evidence.field` of rows written by [`record_coverage_links`].
pub const COVERAGE_FIELD: &str = "coverage";

/// Confidence of a coverage link: the local series really holds part of
/// that provider series (an accepted range), so it's firmer than an
/// untyped `associated` link.
pub const COVERAGE_LINK_CONFIDENCE: f32 = 0.7;

/// Was this row written from series coverage (not Metron `associated`)?
pub fn is_coverage_row(r: &ext::Model) -> bool {
    r.evidence.get("field").and_then(|v| v.as_str()) == Some(COVERAGE_FIELD)
}

/// One "not in your library" link from series coverage: a range of the
/// local series maps to a provider series that also has issues the local
/// series lacks.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageLink {
    pub provider_series_id: String,
    pub name: Option<String>,
    pub year: Option<i32>,
    pub url: Option<String>,
    /// `continued_by` (its extra issues all come after the range),
    /// `continues` (all before), else `see_also`.
    pub kind: RelationshipKind,
    /// "Has #612–645" — shown next to the row.
    pub note: String,
    /// How many of its issues the local series lacks.
    pub not_owned: usize,
    /// The local range(s) mapped to it, `"600–611"`.
    pub ranges: Vec<String>,
}

/// What [`record_coverage_links`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CoverageLinkReport {
    pub upserted: usize,
    /// Targets skipped because a row already points there: a Metron
    /// `associated` row, a user row, or a dismissed row (rejection memory).
    pub skipped_existing: usize,
    /// Live coverage rows of this source no longer backed by a link.
    pub removed: usize,
}

/// Store series `series_id`'s coverage links for `source` as
/// `set_by = 'provider'` external rows (`evidence.field = "coverage"`).
///
/// - A target that already has **any** row of this series and source — a
///   Metron `associated` row, a user row, or a dismissed row of any kind —
///   is skipped: no duplicate target, user > provider, and a dismissal is
///   rejection memory.
/// - A live coverage row whose kind changed is replaced; one this call no
///   longer lists (range removed, all issues now owned) is deleted.
/// - Then the rows are resolved against the library (promotion when the
///   provider series is in the library already).
pub async fn record_coverage_links<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
    source: Source,
    links: &[CoverageLink],
) -> Result<CoverageLinkReport, DbErr> {
    let mut report = CoverageLinkReport::default();
    let Some(src) = ExternalSource::from_source(source) else {
        return Ok(report);
    };
    let existing = ext::Entity::find()
        .filter(ext::Column::FromSeriesId.eq(series_id))
        .filter(ext::Column::Source.eq(src.as_str()))
        .all(conn)
        .await?;
    let now = Utc::now().fixed_offset();
    let mut keep: HashSet<String> = HashSet::new();
    for link in links {
        let pid = link.provider_series_id.trim();
        if pid.is_empty() || pid.len() > 64 || !keep.insert(pid.to_owned()) {
            continue;
        }
        let rows: Vec<&ext::Model> = existing
            .iter()
            .filter(|r| r.provider_series_id == pid)
            .collect();
        let blocked = rows
            .iter()
            .any(|r| !is_coverage_row(r) || r.set_by == "user" || r.dismissed_at.is_some());
        if blocked {
            report.skipped_existing += 1;
            continue;
        }
        // A live coverage row of another kind: replaced by this one.
        let other_kind: Vec<Uuid> = rows
            .iter()
            .filter(|r| r.kind != link.kind.as_str())
            .map(|r| r.id)
            .collect();
        if !other_kind.is_empty() {
            ext::Entity::delete_many()
                .filter(ext::Column::Id.is_in(other_kind))
                .exec(conn)
                .await?;
        }
        let name: Option<String> = link
            .name
            .as_deref()
            .map(|n| n.chars().take(300).collect::<String>())
            .filter(|n| !n.trim().is_empty());
        let url = link
            .url
            .clone()
            .or_else(|| src.series_url(pid))
            .filter(|u| u.len() <= 500);
        let evidence = serde_json::json!({
            "source": src.as_str(),
            "field": COVERAGE_FIELD,
            "ids": [pid],
            "ranges": link.ranges,
            "not_owned": link.not_owned,
            "note": link.note,
        });
        let am = ext::ActiveModel {
            id: Set(Uuid::now_v7()),
            from_series_id: Set(series_id),
            kind: Set(link.kind.as_str().to_owned()),
            qualifier: Set(None),
            source: Set(src.as_str().to_owned()),
            provider_series_id: Set(pid.to_owned()),
            provider_series_name: Set(name),
            provider_series_url: Set(url),
            provider_year: Set(link.year),
            set_by: Set(ExternalSetBy::Provider.as_str().to_owned()),
            confidence: Set(Some(COVERAGE_LINK_CONFIDENCE)),
            evidence: Set(evidence),
            created_by: Set(None),
            promoted_series_id: Set(None),
            dismissed_at: Set(None),
            dismissed_by: Set(None),
            first_set_at: Set(now),
            last_synced_at: Set(now),
        };
        let res = ext::Entity::insert(am)
            .on_conflict(
                OnConflict::columns([
                    ext::Column::FromSeriesId,
                    ext::Column::Kind,
                    ext::Column::Source,
                    ext::Column::ProviderSeriesId,
                ])
                .update_columns([
                    ext::Column::ProviderSeriesName,
                    ext::Column::ProviderSeriesUrl,
                    ext::Column::ProviderYear,
                    ext::Column::Confidence,
                    ext::Column::Evidence,
                    ext::Column::LastSyncedAt,
                ])
                .action_and_where(
                    sea_orm::sea_query::Expr::col((ext::Entity, ext::Column::SetBy))
                        .eq("provider")
                        .and(
                            sea_orm::sea_query::Expr::col((ext::Entity, ext::Column::DismissedAt))
                                .is_null(),
                        ),
                )
                .to_owned(),
            )
            .exec_without_returning(conn)
            .await?;
        report.upserted += usize::try_from(res).unwrap_or(0);
    }
    let gone: Vec<Uuid> = existing
        .iter()
        .filter(|r| {
            is_coverage_row(r)
                && r.set_by == "provider"
                && r.dismissed_at.is_none()
                && !keep.contains(&r.provider_series_id)
        })
        .map(|r| r.id)
        .collect();
    if !gone.is_empty() {
        report.removed = usize::try_from(
            ext::Entity::delete_many()
                .filter(ext::Column::Id.is_in(gone))
                .exec(conn)
                .await?
                .rows_affected,
        )
        .unwrap_or(0);
    }
    promote_series_rows(conn, series_id).await?;
    Ok(report)
}

// ───── resolution + promotion ─────

/// An external row whose provider series resolves to a local series.
#[derive(Debug, Clone, FromQueryResult)]
pub struct Resolved {
    pub ext_id: Uuid,
    pub series_id: Uuid,
}

/// Resolve external rows to local series. `filter` is a SQL predicate over
/// `e` (the external row), with `values` as its parameters (`$1…`). A row
/// resolves through a direct `external_ids` match (same source and id) or
/// the id bridge (the provider's cached detail of that series lists other
/// providers' ids, and a local series is matched under one of them).
/// Removed series and the row's own series don't count; with several
/// matches, a direct one wins, then the oldest series.
pub async fn resolve_where<C: ConnectionTrait>(
    conn: &C,
    filter: &str,
    values: Vec<Value>,
) -> Result<Vec<Resolved>, DbErr> {
    let sql = format!(
        "WITH e AS (SELECT * FROM series_external_relationship e WHERE {filter}), \
         m AS ( \
            SELECT e.id AS ext_id, x.entity_id, 0 AS pri FROM e \
              JOIN external_ids x ON x.entity_type = 'series' AND x.source = e.source \
                                 AND x.external_id = e.provider_series_id \
            UNION ALL \
            SELECT e.id, x.entity_id, 1 FROM e \
              JOIN metadata_cache c ON c.provider = e.source AND c.entity = 'series' \
                                   AND c.external_id = e.provider_series_id \
              CROSS JOIN LATERAL jsonb_array_elements( \
                  CASE WHEN jsonb_typeof(c.payload->'identifiers') = 'array' \
                       THEN c.payload->'identifiers' ELSE '[]'::jsonb END) AS ident(v) \
              JOIN external_ids x ON x.entity_type = 'series' \
                                 AND x.source = ident.v->>'source' \
                                 AND x.external_id = ident.v->>'id' \
             WHERE ident.v->>'source' IN ('metron', 'comicvine', 'gcd') \
         ) \
         SELECT DISTINCT ON (m.ext_id) m.ext_id, s.id AS series_id \
           FROM m \
           JOIN e ON e.id = m.ext_id \
           JOIN series s ON s.id::text = m.entity_id AND s.removed_at IS NULL \
                        AND s.id <> e.from_series_id \
          ORDER BY m.ext_id, m.pri, s.created_at, s.id"
    );
    Resolved::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        values,
    ))
    .all(conn)
    .await
}

/// Resolve these external rows (by id).
pub async fn resolve_rows<C: ConnectionTrait>(
    conn: &C,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Uuid>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(
        resolve_where(conn, "e.id = ANY($1)", vec![ids.to_vec().into()])
            .await?
            .into_iter()
            .map(|r| (r.ext_id, r.series_id))
            .collect(),
    )
}

/// What one promotion pass did (WP-8.2: split so callers holding an
/// `AppState` can tell whether a series relationship was created — that
/// changes similar-series scores — or only provider rows were marked).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Promoted {
    /// User rows turned into series relationship pairs.
    pub pairs_created: usize,
    /// Provider rows newly marked `promoted_series_id` (no edge yet: the
    /// suggestion engine proposes the pair).
    pub marked: usize,
}

impl Promoted {
    pub fn total(self) -> usize {
        self.pairs_created + self.marked
    }
}

/// Promote resolved rows: a `user` row becomes a manual pair (same kind and
/// qualifier, `created_by` kept) and is deleted; a `provider` row is marked
/// `promoted_series_id` (the suggestion engine proposes the pair). A user
/// row whose pair contradicts an existing edge (`PairError::Conflict`) is
/// left in place and logged. Dismissed rows are skipped.
///
/// Takes only a connection, so **callers that hold an `AppState` must call
/// `state.similarity.invalidate_all()` when `pairs_created > 0`** (the
/// metadata-apply job, the suggestion job, the scanner's folder-tag pass
/// and the external-id admin endpoints do).
async fn promote<C: ConnectionTrait>(conn: &C, resolved: Vec<Resolved>) -> Result<Promoted, DbErr> {
    let mut n = Promoted::default();
    if resolved.is_empty() {
        return Ok(n);
    }
    let target: HashMap<Uuid, Uuid> = resolved
        .into_iter()
        .map(|r| (r.ext_id, r.series_id))
        .collect();
    let rows = ext::Entity::find()
        .filter(ext::Column::Id.is_in(target.keys().copied().collect::<Vec<_>>()))
        .filter(ext::Column::DismissedAt.is_null())
        .all(conn)
        .await?;
    for row in rows {
        let Some(&to) = target.get(&row.id) else {
            continue;
        };
        if ExternalSetBy::parse(&row.set_by) == ExternalSetBy::Provider {
            if row.promoted_series_id != Some(to) {
                ext::Entity::update_many()
                    .col_expr(
                        ext::Column::PromotedSeriesId,
                        sea_orm::sea_query::Expr::value(to),
                    )
                    .filter(ext::Column::Id.eq(row.id))
                    .exec(conn)
                    .await?;
                n.marked += 1;
            }
            continue;
        }
        let Ok(kind) = row.kind.parse::<RelationshipKind>() else {
            continue;
        };
        let scope = Scope {
            qualifier: row.qualifier.as_deref().and_then(|q| q.parse().ok()),
            ..Scope::default()
        };
        match super::create_pair_scoped(
            conn,
            row.from_series_id,
            to,
            kind,
            RelationshipSource::Manual,
            None,
            row.created_by,
            &scope,
        )
        .await
        {
            Ok(_) => {
                ext::Entity::delete_by_id(row.id).exec(conn).await?;
                tracing::info!(
                    external_id = %row.id,
                    from_series_id = %row.from_series_id,
                    to_series_id = %to,
                    kind = row.kind,
                    "external relationship promoted to a series relationship"
                );
                n.pairs_created += 1;
            }
            Err(PairError::Db(e)) => return Err(e),
            Err(e) => {
                tracing::warn!(
                    external_id = %row.id,
                    from_series_id = %row.from_series_id,
                    to_series_id = %to,
                    error = %e,
                    "external relationship not promoted"
                );
            }
        }
    }
    Ok(n)
}

/// Promote the external rows of one series (after [`record_provider_links`]).
pub async fn promote_series_rows<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
) -> Result<Promoted, DbErr> {
    let resolved = resolve_where(conn, "e.from_series_id = $1", vec![series_id.into()]).await?;
    promote(conn, resolved).await
}

/// Promotion hook for [`crate::metadata::writers::set_external_id`]: local
/// series `series_id` was just matched to `(source, id)`; promote every
/// external row pointing at that provider series — directly, or through
/// the id bridge (the row points at another provider whose cached detail
/// lists `(source, id)`).
pub async fn promote_for_provider_id<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
    source: &str,
    id: &str,
) -> Result<Promoted, DbErr> {
    if ExternalSource::parse(source).is_none() {
        return Ok(Promoted::default());
    }
    let filter = "e.dismissed_at IS NULL AND ((e.source = $1 AND e.provider_series_id = $2) \
         OR EXISTS (SELECT 1 FROM metadata_cache c \
                     WHERE c.provider = e.source AND c.entity = 'series' \
                       AND c.external_id = e.provider_series_id \
                       AND jsonb_typeof(c.payload->'identifiers') = 'array' \
                       AND c.payload->'identifiers' @> jsonb_build_array( \
                           jsonb_build_object('source', $1::text, 'id', $2::text))))";
    let resolved: Vec<Resolved> = resolve_where(conn, filter, vec![source.into(), id.into()])
        .await?
        .into_iter()
        .filter(|r| r.series_id == series_id)
        .collect();
    promote(conn, resolved).await
}

/// What [`promote_library`] did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PromotionReport {
    pub promoted: usize,
    /// … of which became series relationships (WP-8.2).
    pub pairs_created: usize,
    /// Provider rows whose local match went away (mark cleared).
    pub unmarked: usize,
    /// Label-only reprints resolved to a local issue.
    pub reprints_resolved: u64,
}

/// The per-library promotion pass, run before every suggestion run: promote
/// every resolvable external row of the library's series, clear the mark on
/// provider rows that no longer resolve, and resolve label-only reprints
/// (`writers::resolve_pending_reprints`).
pub async fn promote_library<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<PromotionReport, DbErr> {
    let in_library =
        "e.from_series_id IN (SELECT id FROM series WHERE library_id = $1 AND removed_at IS NULL)";
    let resolved = resolve_where(conn, in_library, vec![library_id.into()]).await?;
    let resolved_ids: HashSet<Uuid> = resolved.iter().map(|r| r.ext_id).collect();
    let promoted = promote(conn, resolved).await?;
    let marked: Vec<Uuid> = ext::Entity::find()
        .filter(ext::Column::PromotedSeriesId.is_not_null())
        .filter(sea_orm::sea_query::Expr::cust_with_values(
            "from_series_id IN (SELECT id FROM series WHERE library_id = $1)",
            [library_id],
        ))
        .all(conn)
        .await?
        .into_iter()
        .filter(|r| !resolved_ids.contains(&r.id))
        .map(|r| r.id)
        .collect();
    let unmarked = if marked.is_empty() {
        0
    } else {
        usize::try_from(
            ext::Entity::update_many()
                .col_expr(
                    ext::Column::PromotedSeriesId,
                    sea_orm::sea_query::Expr::value(Option::<Uuid>::None),
                )
                .filter(ext::Column::Id.is_in(marked))
                .exec(conn)
                .await?
                .rows_affected,
        )
        .unwrap_or(0)
    };
    let reprints_resolved =
        crate::metadata::writers::resolve_pending_reprints(conn, Some(library_id), None).await?;
    Ok(PromotionReport {
        promoted: promoted.total(),
        pairs_created: promoted.pairs_created,
        unmarked,
        reprints_resolved,
    })
}

// ───── admin writes ─────

/// A manual external link (`POST /series/{slug}/external-relationships`).
#[derive(Debug, Clone)]
pub struct UserLink {
    pub kind: RelationshipKind,
    pub qualifier: Option<super::RelationshipQualifier>,
    pub source: ExternalSource,
    pub provider_series_id: String,
    pub name: String,
    pub year: Option<i32>,
}

/// What [`create_user_link`] did.
#[derive(Debug, Clone)]
pub enum UserLinkOutcome {
    /// A new row, or a provider row the admin now claims (`set_by` flipped
    /// to `user`, un-dismissed).
    Created(ext::Model),
    /// The same user row already existed; unchanged.
    Existing(ext::Model),
    /// The target is already a local series: the link was created as a
    /// manual pair instead (no external row is kept).
    Promoted(Box<super::PairOutcome>),
}

/// Add an admin's external link. Idempotent on `(from, kind, source,
/// provider id)`. When the provider series already resolves to a local
/// series the pair is created right away ([`super::create_pair_scoped`];
/// its `PairError`s propagate). Run it in a transaction.
pub async fn create_user_link<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    link: &UserLink,
    actor: Uuid,
) -> Result<UserLinkOutcome, PairError> {
    let now = Utc::now().fixed_offset();
    let existing = ext::Entity::find()
        .filter(ext::Column::FromSeriesId.eq(from))
        .filter(ext::Column::Kind.eq(link.kind.as_str()))
        .filter(ext::Column::Source.eq(link.source.as_str()))
        .filter(ext::Column::ProviderSeriesId.eq(&link.provider_series_id))
        .one(conn)
        .await?;
    let row = match existing {
        Some(r) if r.set_by == "user" => return Ok(UserLinkOutcome::Existing(r)),
        Some(r) => {
            // The admin claims a provider row: it becomes theirs (and is
            // un-dismissed).
            ext::Entity::update_many()
                .col_expr(ext::Column::SetBy, sea_orm::sea_query::Expr::value("user"))
                .col_expr(
                    ext::Column::Qualifier,
                    sea_orm::sea_query::Expr::value(link.qualifier.map(|q| q.as_str())),
                )
                .col_expr(
                    ext::Column::ProviderSeriesName,
                    sea_orm::sea_query::Expr::value(link.name.clone()),
                )
                .col_expr(
                    ext::Column::ProviderYear,
                    sea_orm::sea_query::Expr::value(link.year),
                )
                .col_expr(
                    ext::Column::Confidence,
                    sea_orm::sea_query::Expr::value(Option::<f32>::None),
                )
                .col_expr(
                    ext::Column::CreatedBy,
                    sea_orm::sea_query::Expr::value(actor),
                )
                .col_expr(
                    ext::Column::DismissedAt,
                    sea_orm::sea_query::Expr::value(
                        Option::<chrono::DateTime<chrono::FixedOffset>>::None,
                    ),
                )
                .col_expr(
                    ext::Column::DismissedBy,
                    sea_orm::sea_query::Expr::value(Option::<Uuid>::None),
                )
                .col_expr(
                    ext::Column::LastSyncedAt,
                    sea_orm::sea_query::Expr::value(now),
                )
                .filter(ext::Column::Id.eq(r.id))
                .exec(conn)
                .await?;
            r.id
        }
        None => {
            let id = Uuid::now_v7();
            ext::Entity::insert(ext::ActiveModel {
                id: Set(id),
                from_series_id: Set(from),
                kind: Set(link.kind.as_str().to_owned()),
                qualifier: Set(link.qualifier.map(|q| q.as_str().to_owned())),
                source: Set(link.source.as_str().to_owned()),
                provider_series_id: Set(link.provider_series_id.clone()),
                provider_series_name: Set(Some(link.name.clone())),
                provider_series_url: Set(link.source.series_url(&link.provider_series_id)),
                provider_year: Set(link.year),
                set_by: Set("user".to_owned()),
                confidence: Set(None),
                evidence: Set(serde_json::json!({})),
                created_by: Set(Some(actor)),
                promoted_series_id: Set(None),
                dismissed_at: Set(None),
                dismissed_by: Set(None),
                first_set_at: Set(now),
                last_synced_at: Set(now),
            })
            .exec_without_returning(conn)
            .await?;
            id
        }
    };
    // Already local? Then it's an ordinary relationship.
    if let Some(&to) = resolve_rows(conn, &[row]).await?.get(&row) {
        let scope = Scope {
            qualifier: link.qualifier,
            ..Scope::default()
        };
        let out = super::create_pair_scoped(
            conn,
            from,
            to,
            link.kind,
            RelationshipSource::Manual,
            None,
            Some(actor),
            &scope,
        )
        .await?;
        ext::Entity::delete_by_id(row).exec(conn).await?;
        return Ok(UserLinkOutcome::Promoted(Box::new(out)));
    }
    let model = ext::Entity::find_by_id(row)
        .one(conn)
        .await?
        .ok_or_else(|| PairError::Db(DbErr::RecordNotFound("external relationship".into())))?;
    Ok(UserLinkOutcome::Created(model))
}

/// Remove an external link: a `user` row is deleted; a `provider` row is
/// **dismissed** (hidden, never re-created by a later apply, ignored by the
/// suggestion engine). Returns the row as it was.
pub async fn remove_link<C: ConnectionTrait>(
    conn: &C,
    row: &ext::Model,
    actor: Uuid,
) -> Result<(), DbErr> {
    if row.set_by == "user" {
        ext::Entity::delete_by_id(row.id).exec(conn).await?;
    } else {
        ext::Entity::update_many()
            .col_expr(
                ext::Column::DismissedAt,
                sea_orm::sea_query::Expr::value(Utc::now().fixed_offset()),
            )
            .col_expr(
                ext::Column::DismissedBy,
                sea_orm::sea_query::Expr::value(actor),
            )
            .filter(ext::Column::Id.eq(row.id))
            .exec(conn)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use RelationshipKind as K;

    #[test]
    fn provider_kind_refines_by_series_type() {
        // Edition vs periodical → collects, from either side.
        assert_eq!(
            provider_kind(
                Some("Trade Paperback"),
                "Saga",
                Some("Ongoing Series"),
                "Saga"
            ),
            (K::Collects, ASSOCIATED_TYPED)
        );
        assert_eq!(
            provider_kind(Some("Ongoing Series"), "Saga", Some("Hardcover"), "Saga"),
            (K::CollectedIn, ASSOCIATED_TYPED)
        );
        assert_eq!(
            provider_kind(Some("Omnibus"), "X", Some("Limited Series"), "X").0,
            K::Collects
        );
        assert_eq!(
            provider_kind(Some("Graphic Novel"), "X", Some("One-Shot"), "X").0,
            K::Collects
        );
        // Annuals.
        assert_eq!(
            provider_kind(Some("Annual Series"), "X", Some("Ongoing Series"), "X"),
            (K::AnnualOf, ASSOCIATED_TYPED)
        );
        assert_eq!(
            provider_kind(Some("Ongoing Series"), "X", Some("Annual Series"), "X").0,
            K::HasAnnual
        );
        // Unknown other side: still refined, lower confidence.
        assert_eq!(
            provider_kind(Some("Trade Paperback"), "Saga", None, "Saga"),
            (K::Collects, ASSOCIATED_TYPED_ONE_SIDE)
        );
        // Name markers stand in for a missing type.
        assert_eq!(
            provider_kind(None, "Saga", None, "Saga TPB").0,
            K::CollectedIn
        );
        assert_eq!(
            provider_kind(None, "Hellboy Annual 2017", None, "Hellboy").0,
            K::AnnualOf
        );
        // Same class or nothing known → see_also.
        assert_eq!(
            provider_kind(Some("Ongoing Series"), "A", Some("Limited Series"), "B"),
            (K::SeeAlso, ASSOCIATED_SEE_ALSO)
        );
        assert_eq!(
            provider_kind(Some("Trade Paperback"), "A", Some("Omnibus"), "B").0,
            K::SeeAlso
        );
        assert_eq!(provider_kind(None, "A", None, "B").0, K::SeeAlso);
    }

    #[test]
    fn source_round_trips() {
        for s in [
            ExternalSource::Metron,
            ExternalSource::Comicvine,
            ExternalSource::Gcd,
        ] {
            assert_eq!(ExternalSource::parse(s.as_str()), Some(s));
            assert_eq!(ExternalSource::from_source(s.source()), Some(s));
            assert!(s.series_url("12").is_some());
        }
        assert_eq!(ExternalSource::parse("marvel"), None);
    }
}
