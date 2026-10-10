//! `GET /admin/metadata/coverage` — the **Coverage** tab of `/admin/metadata`.
//!
//! Visibility only: how much of the library carries provider identifiers
//! (ComicVine / Metron / GCD, series- and issue-level), how complete the
//! issue metadata is under the same tiers the series Collection grid uses
//! ([`crate::metadata::completeness`]), where the gaps sit (per library,
//! per series), which tagging tool the files' sidecars came from, the
//! state of the tools that close the gap over time (weekly refresh, issue
//! refresh, strong-match auto-apply, coverage analyses), the live provider
//! budgets, and what closing each gap costs by route. Nothing here starts
//! work; every number links to the surface that does.
//!
//! The snapshot walks every active issue several times, so it is cached in
//! Redis for [`SNAPSHOT_TTL_SECS`]; `?refresh=true` recomputes. The
//! identifier counts use the same `external_ids` sources the dashboard and
//! the library grid's `provider_match` facet count, so the three agree.

use axum::{
    Json,
    extract::{Query, State},
    response::{IntoResponse, Response},
};
use redis::AsyncCommands;
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::admin_metadata::{ProviderView, provider_views};
use crate::auth::RequireAdmin;
use crate::metadata::completeness;
use crate::metadata::rate_limit;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(coverage))
}

/// How long a computed snapshot is served before it is recomputed.
pub const SNAPSHOT_TTL_SECS: u64 = 6 * 3600;
const SNAPSHOT_KEY: &str = "metadata:coverage:snapshot";

/// The provider sources the tab reports, in display order.
const SOURCES: [(&str, &str); 3] = [
    ("comicvine", "ComicVine"),
    ("metron", "Metron"),
    ("gcd", "GCD"),
];

/// Series the "most unmatched issues" lists show per provider.
const TOP_SERIES: i64 = 8;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct CoverageQuery {
    /// Recompute instead of serving the cached snapshot.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MetadataCoverageResp {
    /// When the snapshot was computed (RFC 3339).
    pub computed_at: String,
    /// `true` when this response came from the Redis cache.
    pub cached: bool,
    pub series_total: i64,
    pub issues_total: i64,
    pub libraries_total: i64,
    /// One row per provider: how many series / issues carry its id.
    pub identifiers: Vec<IdentifierCoverage>,
    /// Series / issues that carry all three provider ids.
    pub all_sources_series: i64,
    pub all_sources_issues: i64,
    /// Issue completeness tiers, library-wide.
    pub tiers: TierCounts,
    /// Core fields absent among the incomplete issues (not complete, not
    /// accepted), by field key.
    pub missing_core: Vec<FieldCount>,
    /// Recommended (non-gating) fields present, whole library.
    pub recommended: Vec<FieldCount>,
    pub series_fields: SeriesFieldCounts,
    /// Per library: issues without each provider's id. Libraries with no
    /// active issues are omitted.
    pub by_library: Vec<LibraryGap>,
    /// Per provider: the series with the most issues lacking its id.
    pub top_series: Vec<TopSeriesForSource>,
    /// Which tagging tool wrote the files' sidecars, and which ids the
    /// scanner read from them.
    pub tag_sources: Vec<TagSourceRow>,
    /// The state of the tools that close the gap over time.
    pub tools: FillTools,
    /// Live provider quota / budget views (same shape as the dashboard).
    /// Live state: filled on every response (the cached snapshot carries
    /// an empty list).
    pub providers: Vec<ProviderView>,
    /// What closing each provider's issue-id gap costs, by route.
    pub fill_cost: Vec<FillCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IdentifierCoverage {
    pub source: String,
    pub label: String,
    pub series_with_id: i64,
    pub issues_with_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TierCounts {
    pub complete: i64,
    pub partial: i64,
    pub needs_metadata: i64,
    pub accepted: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FieldCount {
    /// [`crate::metadata::MetadataField::key`] or
    /// [`completeness::EXTERNAL_ID_KEY`].
    pub field: String,
    pub issues: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SeriesFieldCounts {
    pub with_summary: i64,
    pub with_series_type: i64,
    pub synced: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LibraryGap {
    pub library_id: String,
    pub name: String,
    pub slug: String,
    pub issues_total: i64,
    pub without_comicvine: i64,
    pub without_metron: i64,
    pub without_gcd: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TopSeriesForSource {
    pub source: String,
    pub series: Vec<TopSeriesRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TopSeriesRow {
    pub slug: String,
    pub name: String,
    pub year: Option<i32>,
    pub issues_without_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagSourceRow {
    /// `folio` | `comictagger_comicvine` | `comictagger_metron` |
    /// `comictagger_gcd` | `comictagger_other` | `mylar` | `scraper` |
    /// `untagged`.
    pub tool: String,
    pub issues: i64,
    pub with_comicvine: i64,
    pub with_metron: i64,
    pub with_gcd: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FillTools {
    pub weekly_refresh_enabled: bool,
    pub weekly_refresh_cron: String,
    pub stale_after_days: u32,
    pub issue_refresh_enabled: bool,
    pub issue_refresh_per_provider_cap: u32,
    /// Libraries with `metadata_auto_apply_strong_matches` on.
    pub auto_apply_libraries: i64,
    pub libraries_total: i64,
    /// Series with a coverage analysis still on record (24 h).
    pub coverage_analysed_series: i64,
    /// Series / issues ever synced with a provider through Folio.
    pub series_synced: i64,
    pub issues_synced: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FillCost {
    pub source: String,
    /// Active issues without this provider's id.
    pub issues_without_id: i64,
    /// Series without this provider's series id.
    pub series_without_id: i64,
    /// Requests to link ids from the provider's issue lists: one series
    /// search per unmatched series plus one list page per
    /// `list_page_size` issues of every series.
    pub via_lists_requests: i64,
    /// Requests to fetch each missing issue's detail record instead.
    pub via_detail_requests: i64,
    /// The provider's documented daily budget for detail requests.
    pub daily_budget: i64,
    pub list_page_size: i64,
}

#[utoipa::path(
    operation_id = "admin_metadata_coverage",
    get,
    path = "/admin/metadata/coverage",
    params(CoverageQuery),
    responses(
        (status = 200, body = MetadataCoverageResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn coverage(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<CoverageQuery>,
) -> Response {
    let mut redis = app.jobs.redis.clone();
    if !q.refresh
        && let Ok(Some(raw)) = redis.get::<_, Option<String>>(SNAPSHOT_KEY).await
        && let Ok(mut snap) = serde_json::from_str::<MetadataCoverageResp>(&raw)
    {
        snap.cached = true;
        // Budgets are live state, not part of the snapshot.
        snap.providers = provider_views(&app).await;
        return Json(snap).into_response();
    }
    match compute(&app).await {
        Ok(mut snap) => {
            if let Ok(raw) = serde_json::to_string(&snap) {
                let _: Result<(), _> = redis.set_ex(SNAPSHOT_KEY, raw, SNAPSHOT_TTL_SECS).await;
            }
            snap.providers = provider_views(&app).await;
            Json(snap).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "metadata coverage: snapshot failed");
            super::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "couldn't compute the coverage snapshot",
            )
        }
    }
}

// ───────── SQL fragments ─────────

/// Active, non-removed issues: the denominator everywhere.
const ACTIVE: &str = "i.removed_at IS NULL AND i.state = 'active'";

fn issue_has_source(alias: &str, source: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM external_ids e WHERE e.entity_type = 'issue' \
           AND e.entity_id = {alias}.id AND e.source = '{source}')"
    )
}

fn series_has_source(alias: &str, source: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM external_ids e WHERE e.entity_type = 'series' \
           AND e.entity_id = {alias}.id::text AND e.source = '{source}')"
    )
}

/// The completeness predicates, pieced the same way
/// [`completeness::issue_metadata_satisfied_sql`] is built so the tier split
/// here agrees with the Collection grid's.
struct CorePredicates {
    matched: String,
    date: String,
    summary: String,
    pages: String,
    credit: String,
}

fn core_predicates(a: &str) -> CorePredicates {
    CorePredicates {
        matched: format!(
            "EXISTS (SELECT 1 FROM external_ids x WHERE x.entity_type = 'issue' \
               AND x.entity_id = {a}.id AND x.source IN ('comicvine', 'metron'))"
        ),
        date: format!("({a}.year IS NOT NULL AND {a}.year >= 1800)"),
        summary: format!("(COALESCE(btrim({a}.summary), '') <> '')"),
        pages: format!("({a}.page_count IS NOT NULL AND {a}.page_count > 0)"),
        credit: format!(
            "(COALESCE({a}.writer, '') <> '' OR COALESCE({a}.penciller, '') <> '' \
              OR COALESCE({a}.inker, '') <> '' OR COALESCE({a}.colorist, '') <> '' \
              OR COALESCE({a}.letterer, '') <> '' OR COALESCE({a}.cover_artist, '') <> '' \
              OR COALESCE({a}.editor, '') <> '' OR COALESCE({a}.translator, '') <> '')"
        ),
    }
}

// ───────── computation ─────────

async fn compute(app: &AppState) -> Result<MetadataCoverageResp, sea_orm::DbErr> {
    let db = &app.db;
    let cfg = app.cfg();

    #[derive(FromQueryResult)]
    struct Totals {
        series_total: i64,
        issues_total: i64,
        libraries_total: i64,
    }
    let totals = one::<Totals>(
        db,
        "SELECT (SELECT count(*) FROM series WHERE removed_at IS NULL)::bigint AS series_total, \
                (SELECT count(*) FROM issues i WHERE i.removed_at IS NULL AND i.state = 'active')::bigint AS issues_total, \
                (SELECT count(*) FROM libraries)::bigint AS libraries_total",
    )
    .await?;

    // Identifier coverage per source.
    let mut identifiers = Vec::with_capacity(SOURCES.len());
    for (source, label) in SOURCES {
        #[derive(FromQueryResult)]
        struct Row {
            series_with_id: i64,
            issues_with_id: i64,
        }
        let sql = format!(
            "SELECT (SELECT count(*) FROM series s WHERE s.removed_at IS NULL AND {ss})::bigint AS series_with_id, \
                    (SELECT count(*) FROM issues i WHERE {ACTIVE} AND {is})::bigint AS issues_with_id",
            ss = series_has_source("s", source),
            is = issue_has_source("i", source),
        );
        let r = one::<Row>(db, &sql).await?;
        identifiers.push(IdentifierCoverage {
            source: source.to_owned(),
            label: label.to_owned(),
            series_with_id: r.series_with_id,
            issues_with_id: r.issues_with_id,
        });
    }

    #[derive(FromQueryResult)]
    struct AllThree {
        series_all: i64,
        issues_all: i64,
    }
    let all_three = one::<AllThree>(
        db,
        &format!(
            "SELECT (SELECT count(*) FROM series s WHERE s.removed_at IS NULL \
                       AND (SELECT count(DISTINCT e.source) FROM external_ids e \
                            WHERE e.entity_type = 'series' AND e.entity_id = s.id::text \
                              AND e.source IN ('comicvine','metron','gcd')) = 3)::bigint AS series_all, \
                    (SELECT count(*) FROM issues i WHERE {ACTIVE} \
                       AND (SELECT count(DISTINCT e.source) FROM external_ids e \
                            WHERE e.entity_type = 'issue' AND e.entity_id = i.id \
                              AND e.source IN ('comicvine','metron','gcd')) = 3)::bigint AS issues_all"
        ),
    )
    .await?;

    // Tiers + missing core + recommended, one pass over active issues.
    let p = core_predicates("i");
    #[derive(FromQueryResult)]
    struct TierRow {
        complete: i64,
        partial: i64,
        needs_metadata: i64,
        accepted: i64,
        miss_id: i64,
        miss_summary: i64,
        miss_credit: i64,
        miss_date: i64,
        miss_pages: i64,
        rec_title: i64,
        rec_teams: i64,
        rec_genre: i64,
        rec_arc: i64,
        rec_cover: i64,
    }
    let sql = format!(
        "WITH t AS (SELECT \
             {matched} AS matched, {date} AS has_date, {summary} AS has_summary, \
             {pages} AS has_pages, {credit} AS has_credit, \
             i.metadata_review_accepted_at IS NOT NULL AS accepted, \
             COALESCE(i.title, '') <> '' AS has_title, COALESCE(i.teams, '') <> '' AS has_teams, \
             COALESCE(i.genre, '') <> '' AS has_genre, COALESCE(i.story_arc, '') <> '' AS has_arc, \
             EXISTS (SELECT 1 FROM issue_cover c WHERE c.issue_id = i.id AND c.is_active) AS has_cover \
           FROM issues i WHERE {ACTIVE}), \
         u AS (SELECT *, (matched AND has_date AND has_summary AND has_pages AND has_credit) AS core_ok, \
             ((NOT has_date)::int + (NOT has_summary)::int + (NOT has_pages)::int + (NOT has_credit)::int) AS missing_n \
           FROM t) \
         SELECT count(*) FILTER (WHERE core_ok)::bigint AS complete, \
                count(*) FILTER (WHERE NOT core_ok AND accepted)::bigint AS accepted, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND (NOT matched OR missing_n >= 4))::bigint AS needs_metadata, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND matched AND missing_n < 4)::bigint AS partial, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND NOT matched)::bigint AS miss_id, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND NOT has_summary)::bigint AS miss_summary, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND NOT has_credit)::bigint AS miss_credit, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND NOT has_date)::bigint AS miss_date, \
                count(*) FILTER (WHERE NOT core_ok AND NOT accepted AND NOT has_pages)::bigint AS miss_pages, \
                count(*) FILTER (WHERE has_title)::bigint AS rec_title, \
                count(*) FILTER (WHERE has_teams)::bigint AS rec_teams, \
                count(*) FILTER (WHERE has_genre)::bigint AS rec_genre, \
                count(*) FILTER (WHERE has_arc)::bigint AS rec_arc, \
                count(*) FILTER (WHERE has_cover)::bigint AS rec_cover \
           FROM u",
        matched = p.matched,
        date = p.date,
        summary = p.summary,
        pages = p.pages,
        credit = p.credit,
    );
    let t = one::<TierRow>(db, &sql).await?;
    let tiers = TierCounts {
        complete: t.complete,
        partial: t.partial,
        needs_metadata: t.needs_metadata,
        accepted: t.accepted,
    };
    use crate::metadata::MetadataField;
    let missing_core = vec![
        FieldCount {
            field: completeness::EXTERNAL_ID_KEY.to_owned(),
            issues: t.miss_id,
        },
        FieldCount {
            field: MetadataField::Summary.key(),
            issues: t.miss_summary,
        },
        FieldCount {
            field: MetadataField::Credits.key(),
            issues: t.miss_credit,
        },
        FieldCount {
            field: MetadataField::CoverDate.key(),
            issues: t.miss_date,
        },
        FieldCount {
            field: MetadataField::PageCount.key(),
            issues: t.miss_pages,
        },
    ];
    let recommended = vec![
        FieldCount {
            field: MetadataField::Title.key(),
            issues: t.rec_title,
        },
        FieldCount {
            field: MetadataField::Teams.key(),
            issues: t.rec_teams,
        },
        FieldCount {
            field: MetadataField::Genres.key(),
            issues: t.rec_genre,
        },
        FieldCount {
            field: MetadataField::StoryArcs.key(),
            issues: t.rec_arc,
        },
        FieldCount {
            field: MetadataField::CoverPrimary.key(),
            issues: t.rec_cover,
        },
    ];

    #[derive(FromQueryResult)]
    struct SeriesFields {
        with_summary: i64,
        with_series_type: i64,
        synced: i64,
    }
    let sf = one::<SeriesFields>(
        db,
        "SELECT count(*) FILTER (WHERE COALESCE(btrim(summary), '') <> '')::bigint AS with_summary, \
                count(*) FILTER (WHERE COALESCE(btrim(series_type), '') <> '')::bigint AS with_series_type, \
                count(*) FILTER (WHERE last_metadata_sync_at IS NOT NULL)::bigint AS synced \
           FROM series WHERE removed_at IS NULL",
    )
    .await?;
    let series_fields = SeriesFieldCounts {
        with_summary: sf.with_summary,
        with_series_type: sf.with_series_type,
        synced: sf.synced,
    };

    // Per library.
    #[derive(FromQueryResult)]
    struct LibRow {
        library_id: String,
        name: String,
        slug: String,
        issues_total: i64,
        without_comicvine: i64,
        without_metron: i64,
        without_gcd: i64,
    }
    let sql = format!(
        "SELECT l.id::text AS library_id, l.name, l.slug, count(*)::bigint AS issues_total, \
                count(*) FILTER (WHERE NOT {cv})::bigint AS without_comicvine, \
                count(*) FILTER (WHERE NOT {m})::bigint AS without_metron, \
                count(*) FILTER (WHERE NOT {g})::bigint AS without_gcd \
           FROM libraries l JOIN issues i ON i.library_id = l.id \
          WHERE {ACTIVE} \
          GROUP BY l.id, l.name, l.slug \
          ORDER BY without_comicvine DESC, l.name ASC",
        cv = issue_has_source("i", "comicvine"),
        m = issue_has_source("i", "metron"),
        g = issue_has_source("i", "gcd"),
    );
    let by_library = LibRow::find_by_statement(stmt(db, &sql))
        .all(db)
        .await?
        .into_iter()
        .map(|r| LibraryGap {
            library_id: r.library_id,
            name: r.name,
            slug: r.slug,
            issues_total: r.issues_total,
            without_comicvine: r.without_comicvine,
            without_metron: r.without_metron,
            without_gcd: r.without_gcd,
        })
        .collect();

    // Top series per source.
    let mut top_series = Vec::with_capacity(SOURCES.len());
    for (source, _) in SOURCES {
        #[derive(FromQueryResult)]
        struct Row {
            slug: String,
            name: String,
            year: Option<i32>,
            issues_without_id: i64,
        }
        let sql = format!(
            "SELECT s.slug, s.name, s.year, count(*)::bigint AS issues_without_id \
               FROM issues i JOIN series s ON s.id = i.series_id \
              WHERE {ACTIVE} AND s.removed_at IS NULL AND NOT {has} \
              GROUP BY s.id, s.slug, s.name, s.year \
              ORDER BY issues_without_id DESC, s.name ASC \
              LIMIT {TOP_SERIES}",
            has = issue_has_source("i", source),
        );
        let rows = Row::find_by_statement(stmt(db, &sql)).all(db).await?;
        top_series.push(TopSeriesForSource {
            source: source.to_owned(),
            series: rows
                .into_iter()
                .map(|r| TopSeriesRow {
                    slug: r.slug,
                    name: r.name,
                    year: r.year,
                    issues_without_id: r.issues_without_id,
                })
                .collect(),
        });
    }

    // Tag sources.
    #[derive(FromQueryResult)]
    struct TagRow {
        tool: String,
        issues: i64,
        with_comicvine: i64,
        with_metron: i64,
        with_gcd: i64,
    }
    let sql = format!(
        "SELECT {tool} AS tool, \
           count(*)::bigint AS issues, \
           count(*) FILTER (WHERE {cv})::bigint AS with_comicvine, \
           count(*) FILTER (WHERE {m})::bigint AS with_metron, \
           count(*) FILTER (WHERE {g})::bigint AS with_gcd \
         FROM (SELECT i.id, COALESCE(i.comic_info_raw->>'notes', '') AS n, \
                      COALESCE(i.comic_info_raw->>'genre', '') AS g, \
                      COALESCE(i.comic_info_raw->>'web', '') AS w \
                 FROM issues i WHERE {ACTIVE}) i \
         GROUP BY 1 ORDER BY issues DESC",
        tool = crate::metadata::tag_tool::sql_case("n", "g", "w"),
        cv = issue_has_source("i", "comicvine"),
        m = issue_has_source("i", "metron"),
        g = issue_has_source("i", "gcd"),
    );
    let tag_sources = TagRow::find_by_statement(stmt(db, &sql))
        .all(db)
        .await?
        .into_iter()
        .map(|r| TagSourceRow {
            tool: r.tool,
            issues: r.issues,
            with_comicvine: r.with_comicvine,
            with_metron: r.with_metron,
            with_gcd: r.with_gcd,
        })
        .collect();

    // Tools.
    #[derive(FromQueryResult)]
    struct ToolRow {
        auto_apply_libraries: i64,
        libraries_total: i64,
        series_synced: i64,
        issues_synced: i64,
    }
    let tr = one::<ToolRow>(
        db,
        &format!(
            "SELECT (SELECT count(*) FROM libraries WHERE metadata_auto_apply_strong_matches)::bigint AS auto_apply_libraries, \
                    (SELECT count(*) FROM libraries)::bigint AS libraries_total, \
                    (SELECT count(*) FROM series WHERE removed_at IS NULL AND last_metadata_sync_at IS NOT NULL)::bigint AS series_synced, \
                    (SELECT count(*) FROM issues i WHERE {ACTIVE} AND i.last_metadata_sync_at IS NOT NULL)::bigint AS issues_synced"
        ),
    )
    .await?;
    let coverage_analysed_series = count_coverage_records(&app.jobs.redis).await;
    let tools = FillTools {
        weekly_refresh_enabled: cfg.metadata_weekly_refresh_enabled,
        weekly_refresh_cron: cfg.metadata_weekly_refresh_cron.clone(),
        stale_after_days: cfg.metadata_stale_after_days,
        issue_refresh_enabled: cfg.metadata_issue_refresh_enabled,
        issue_refresh_per_provider_cap: cfg.metadata_issue_refresh_per_provider_cap,
        auto_apply_libraries: tr.auto_apply_libraries,
        libraries_total: tr.libraries_total,
        coverage_analysed_series,
        series_synced: tr.series_synced,
        issues_synced: tr.issues_synced,
    };

    // Fill cost: list pages per page size, over every series.
    #[derive(FromQueryResult)]
    struct Pages {
        pages_100: i64,
        pages_50: i64,
    }
    let pg = one::<Pages>(
        db,
        &format!(
            "SELECT COALESCE(sum(ceil(n / 100.0)), 0)::bigint AS pages_100, \
                    COALESCE(sum(ceil(n / 50.0)), 0)::bigint AS pages_50 \
               FROM (SELECT count(*) AS n FROM issues i WHERE {ACTIVE} GROUP BY i.series_id) t"
        ),
    )
    .await?;
    let fill_cost = identifiers
        .iter()
        .map(|id| {
            let (page_size, pages, daily) = match id.source.as_str() {
                "comicvine" => (
                    100,
                    pg.pages_100,
                    i64::from(rate_limit::COMICVINE_HOUR_CAPACITY) * 24,
                ),
                "metron" => (
                    100,
                    pg.pages_100,
                    i64::from(rate_limit::METRON_DAY.capacity),
                ),
                _ => (50, pg.pages_50, i64::from(rate_limit::GCD_DAY.capacity)),
            };
            let series_without_id = (totals.series_total - id.series_with_id).max(0);
            let issues_without_id = (totals.issues_total - id.issues_with_id).max(0);
            FillCost {
                source: id.source.clone(),
                issues_without_id,
                series_without_id,
                via_lists_requests: series_without_id + pages,
                via_detail_requests: issues_without_id,
                daily_budget: daily,
                list_page_size: page_size,
            }
        })
        .collect();

    Ok(MetadataCoverageResp {
        computed_at: chrono::Utc::now().to_rfc3339(),
        cached: false,
        series_total: totals.series_total,
        issues_total: totals.issues_total,
        libraries_total: totals.libraries_total,
        identifiers,
        all_sources_series: all_three.series_all,
        all_sources_issues: all_three.issues_all,
        tiers,
        missing_core,
        recommended,
        series_fields,
        by_library,
        top_series,
        tag_sources,
        tools,
        providers: Vec::new(),
        fill_cost,
    })
}

fn stmt<C: ConnectionTrait>(db: &C, sql: &str) -> Statement {
    Statement::from_string(db.get_database_backend(), sql.to_owned())
}

async fn one<T: FromQueryResult>(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
) -> Result<T, sea_orm::DbErr> {
    T::find_by_statement(stmt(db, sql))
        .one(db)
        .await?
        .ok_or_else(|| sea_orm::DbErr::RecordNotFound("coverage aggregate".into()))
}

/// Series with a coverage job record on file (`coverage:series:*`).
async fn count_coverage_records(redis: &redis::aio::ConnectionManager) -> i64 {
    let mut conn = redis.clone();
    let mut cursor: u64 = 0;
    let mut n: i64 = 0;
    loop {
        let res: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("coverage:series:*")
            .arg("COUNT")
            .arg(500)
            .query_async(&mut conn)
            .await;
        match res {
            Ok((next, keys)) => {
                n += keys.len() as i64;
                if next == 0 {
                    break;
                }
                cursor = next;
            }
            Err(e) => {
                tracing::warn!(error = %e, "metadata coverage: redis scan failed");
                break;
            }
        }
    }
    n
}
