//! Compile a validated filter DSL into a `sea_query::SelectStatement`.
//!
//! The compiler is the only place that:
//!   - validates `(field, op, value)` triples against the registry,
//!   - decides whether the reading-state LEFT JOIN is needed,
//!   - emits junction-backed `EXISTS` / `NOT EXISTS` for multi conditions,
//!   - and stitches together filters / sort / cursor / limit.
//!
//! Returned statements are pure sea_query — caller drives the binder and
//! result projection. The result endpoint in `api::saved_views` then
//! reads each row as a [`series::Model`] and reuses the existing
//! `SeriesView::from(model)` projection so the wire shape matches
//! `GET /series` exactly.
//!
//! Two roots (WP-5.4): [`compile`] selects `series` rows for
//! `filter_series` views; [`compile_issues`] selects `issues` rows (joined
//! to their parent `series`) for `filter_issues` views. Both share the
//! per-condition compiler; the registry says which SQL a field maps to on
//! each root, and a field with no mapping for the view's entity is a
//! [`CompileError::FieldNotAvailable`].

use super::dsl::{Condition, Field, FilterDsl, MatchMode, Op, SortField, SortOrder, ViewEntity};
use super::registry::{self, FieldKind, Source};
use crate::library::access::VisibleLibraries;
use crate::reading::series_progress;
use entity::{issue, series};
use sea_orm::{
    Condition as SeaCondition, Iterable,
    sea_query::{
        Alias, BinOper, Expr, ExprTrait, Func, JoinType, NullOrdering, Order, Query,
        SelectStatement, SimpleExpr,
    },
};
use serde_json::Value;
use uuid::Uuid;

/// Wire form: opaque base64 string handed to the client; encodes
/// `(sort_value, id)`. Empty `sort_value` is valid (used when sorting by
/// a nullable column whose boundary row was NULL).
#[derive(Debug, Clone)]
pub struct Cursor {
    pub sort_value: String,
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct CompileInput<'a> {
    pub dsl: &'a FilterDsl,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub limit: u64,
    pub cursor: Option<Cursor>,
    pub user_id: Uuid,
    pub visible_libraries: VisibleLibraries,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("field `{0:?}` does not support op `{1:?}`")]
    OpNotAllowedForField(Field, Op),
    #[error("value for field `{field:?}` op `{op:?}` is invalid: {reason}")]
    BadValue {
        field: Field,
        op: Op,
        reason: String,
    },
    #[error("field `{field:?}` is not available on {} views", entity.as_str())]
    FieldNotAvailable { field: Field, entity: ViewEntity },
    #[error("sort `{}` is not available on {} views", sort.as_str(), entity.as_str())]
    SortNotAvailable { sort: SortField, entity: ViewEntity },
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("internal: {0}")]
    Internal(String),
}

/// Compile to a single `SelectStatement`. The statement projects every
/// column on `series` (so the result endpoint can hydrate `series::Model`)
/// and uses the same column for sort tiebreaking. Pagination fetches
/// `limit + 1` rows; the caller pops the trailing row to compute
/// `next_cursor`.
pub fn compile(input: &CompileInput<'_>) -> Result<SelectStatement, CompileError> {
    let mut q = Query::select();

    // Project full series row. SeaORM's `Iterable` walks every Column
    // variant in order, so this stays in sync with the entity automatically.
    for col in series::Column::iter() {
        q.column((series::Entity, col));
    }

    q.from(series::Entity);

    apply_visibility(&mut q, &input.visible_libraries);

    let needs_join = needs_reading_join(input.dsl, input.sort_field);
    if needs_join {
        let usp = series_progress::subquery_alias();
        q.join_subquery(
            JoinType::LeftJoin,
            series_progress::subquery_for(input.user_id),
            usp.clone(),
            Expr::col((usp, Alias::new("series_id"))).equals((series::Entity, series::Column::Id)),
        );
    }

    // library-filters-richer-1.0 M4: only emit the active-issue-count
    // aggregate join when the filter references `collection_completeness`.
    // Keeps the baseline filter cost untouched for the 95% of filters
    // that don't need this.
    if needs_active_issue_count_join(input.dsl) {
        let alias = active_issue_count_alias();
        q.join_subquery(
            JoinType::LeftJoin,
            active_issue_count_subquery(),
            alias.clone(),
            Expr::col((alias, Alias::new("series_id")))
                .equals((series::Entity, series::Column::Id)),
        );
    }

    // Metadata-completeness aggregate (active_count + complete_count per
    // series). Same join-gating discipline: only emitted when a filter
    // references `metadata_completeness`, so baseline filters never pay for
    // the EXISTS-per-issue scan.
    if needs_metadata_completeness_join(input.dsl) {
        let alias = metadata_completeness_alias();
        q.join_subquery(
            JoinType::LeftJoin,
            metadata_completeness_subquery(),
            alias.clone(),
            Expr::col((alias, Alias::new("series_id")))
                .equals((series::Entity, series::Column::Id)),
        );
    }

    let ctx = Ctx {
        entity: ViewEntity::Series,
        user_id: input.user_id,
    };
    apply_conditions(&mut q, input.dsl, &ctx)?;

    let (sort_expr, order_sea) = sort_expression(input.sort_field, input.sort_order);
    apply_cursor(&mut q, input, sort_expr.clone(), order_sea.clone());
    q.order_by_expr(sort_expr, order_sea.clone());
    q.order_by((series::Entity, series::Column::Id), order_sea);

    q.limit(input.limit + 1);

    Ok(q)
}

/// Inputs for [`compile_issues`]. Issue views paginate with an opaque
/// keyset cursor ([`IssueCursor`]) over the view's sort keys plus the
/// issue id — the same pattern as the series root's `(sort value, id)`
/// cursor, extended to the multi-key `name` sort (series name → issue
/// number → id). A keyset never skips or repeats rows when issues are
/// added or removed between page fetches.
#[derive(Debug, Clone)]
pub struct IssueCompileInput<'a> {
    pub dsl: &'a FilterDsl,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub limit: u64,
    pub cursor: Option<IssueCursor>,
    pub user_id: Uuid,
    pub visible_libraries: VisibleLibraries,
}

/// Keyset position for issue-view pagination: the last returned row's
/// sort-key values (JSON-typed; `null` = SQL NULL) and its id. Encoded as
/// opaque base64url JSON; callers never interpret it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IssueCursor {
    pub keys: Vec<Value>,
    pub id: String,
}

impl IssueCursor {
    pub fn encode(&self) -> String {
        use base64::Engine;
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    /// `None` on any malformed token (the handler answers 400).
    pub fn decode(s: &str) -> Option<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The cursor for a result row, given the row's sort-key columns.
    /// Key order must match [`issue_sort_keys`].
    #[allow(clippy::too_many_arguments)]
    pub fn for_row(
        sort: SortField,
        id: &str,
        series_name: &str,
        sort_number: Option<f64>,
        year: Option<i32>,
        created_at: &chrono::DateTime<chrono::FixedOffset>,
        updated_at: &chrono::DateTime<chrono::FixedOffset>,
    ) -> Self {
        let keys = match sort {
            SortField::Name => vec![Value::from(series_name), serde_json::json!(sort_number)],
            SortField::Year => vec![serde_json::json!(year)],
            SortField::CreatedAt => vec![Value::from(created_at.to_rfc3339())],
            SortField::UpdatedAt => vec![Value::from(updated_at.to_rfc3339())],
            SortField::LastRead | SortField::ReadProgress => Vec::new(),
        };
        Self {
            keys,
            id: id.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum KeyType {
    Text,
    Float,
    Int,
    Timestamp,
}

/// One sort key of an issue view: SQL expression, bind type, nullability.
/// Nullable keys order NULLS LAST in both directions.
#[derive(Debug, Clone, Copy)]
struct IssueSortKey {
    sql: &'static str,
    ty: KeyType,
    nullable: bool,
}

/// Sort keys (before the `issues.id` tiebreaker) for each issue-view sort.
fn issue_sort_keys(sort: SortField) -> Result<&'static [IssueSortKey], CompileError> {
    const NAME: &[IssueSortKey] = &[
        IssueSortKey {
            sql: "series.name",
            ty: KeyType::Text,
            nullable: false,
        },
        IssueSortKey {
            sql: "issues.sort_number",
            ty: KeyType::Float,
            nullable: true,
        },
    ];
    const YEAR: &[IssueSortKey] = &[IssueSortKey {
        sql: "issues.year",
        ty: KeyType::Int,
        nullable: true,
    }];
    const CREATED: &[IssueSortKey] = &[IssueSortKey {
        sql: "issues.created_at",
        ty: KeyType::Timestamp,
        nullable: false,
    }];
    const UPDATED: &[IssueSortKey] = &[IssueSortKey {
        sql: "issues.updated_at",
        ty: KeyType::Timestamp,
        nullable: false,
    }];
    match sort {
        SortField::Name => Ok(NAME),
        SortField::Year => Ok(YEAR),
        SortField::CreatedAt => Ok(CREATED),
        SortField::UpdatedAt => Ok(UPDATED),
        SortField::LastRead | SortField::ReadProgress => Err(CompileError::SortNotAvailable {
            sort,
            entity: ViewEntity::Issue,
        }),
    }
}

/// Bind a cursor key value with the column's type; `Ok(None)` = SQL NULL.
fn cursor_bind(ty: KeyType, v: &Value) -> Result<Option<sea_orm::Value>, CompileError> {
    let bad = || CompileError::InvalidCursor;
    if v.is_null() {
        return Ok(None);
    }
    Ok(Some(match ty {
        KeyType::Text => v.as_str().ok_or_else(bad)?.to_owned().into(),
        KeyType::Float => v.as_f64().ok_or_else(bad)?.into(),
        KeyType::Int => i32::try_from(v.as_i64().ok_or_else(bad)?)
            .map_err(|_| bad())?
            .into(),
        KeyType::Timestamp => chrono::DateTime::parse_from_rfc3339(v.as_str().ok_or_else(bad)?)
            .map_err(|_| bad())?
            .into(),
    }))
}

/// Lexicographic "strictly after the cursor" predicate over
/// `(keys…, issues.id)` in the view's direction, NULLS LAST:
/// `OR_i (k_0 = v_0 AND … AND k_{i-1} = v_{i-1} AND k_i after v_i)`,
/// where equality on a NULL cursor value is `IS NULL`, "after a NULL" is
/// impossible (NULLs sort last), and "after a value" on a nullable key
/// also admits NULLs.
fn issue_keyset_predicate(
    keys: &[IssueSortKey],
    cursor: &IssueCursor,
    order: SortOrder,
) -> Result<SimpleExpr, CompileError> {
    if cursor.keys.len() != keys.len() {
        return Err(CompileError::InvalidCursor);
    }
    let op = match order {
        SortOrder::Asc => ">",
        SortOrder::Desc => "<",
    };
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let bind = |v: sea_orm::Value, values: &mut Vec<sea_orm::Value>| {
        values.push(v);
        format!("${}", values.len())
    };
    let mut bound: Vec<(IssueSortKey, Option<sea_orm::Value>)> = Vec::new();
    for (k, v) in keys.iter().zip(&cursor.keys) {
        bound.push((*k, cursor_bind(k.ty, v)?));
    }
    bound.push((
        IssueSortKey {
            sql: "issues.id",
            ty: KeyType::Text,
            nullable: false,
        },
        Some(cursor.id.clone().into()),
    ));
    let mut disjuncts: Vec<String> = Vec::new();
    for i in 0..bound.len() {
        let (key, val) = &bound[i];
        let Some(val) = val else {
            continue; // nothing sorts after NULL (NULLS LAST)
        };
        let mut parts: Vec<String> = Vec::new();
        for (pk, pv) in &bound[..i] {
            parts.push(match pv {
                None => format!("{} IS NULL", pk.sql),
                Some(v) => format!("{} = {}", pk.sql, bind(v.clone(), &mut values)),
            });
        }
        let p = bind(val.clone(), &mut values);
        parts.push(if key.nullable {
            format!("({k} {op} {p} OR {k} IS NULL)", k = key.sql)
        } else {
            format!("{} {op} {p}", key.sql)
        });
        disjuncts.push(format!("({})", parts.join(" AND ")));
    }
    if disjuncts.is_empty() {
        return Ok(Expr::val(false));
    }
    Ok(Expr::cust_with_values(
        format!("({})", disjuncts.join(" OR ")),
        values,
    ))
}

/// Columns projected by [`compile_issues`] — the `IssueSummaryView` card
/// set plus `library_id` / `age_rating` (mirrors
/// `api::issue_card::IssueCardRow`; never the wide `comic_info_raw` /
/// `pages` JSON). `series_slug` + `series_name` ride along from the join.
const ISSUE_CARD_COLUMNS: &[issue::Column] = &[
    issue::Column::Id,
    issue::Column::Slug,
    issue::Column::SeriesId,
    issue::Column::LibraryId,
    issue::Column::Title,
    issue::Column::NumberRaw,
    issue::Column::SortNumber,
    issue::Column::Year,
    issue::Column::PageCount,
    issue::Column::State,
    issue::Column::SpecialType,
    issue::Column::CreatedAt,
    issue::Column::UpdatedAt,
    issue::Column::AgeRating,
];

/// `FROM issues JOIN series WHERE <active, visible, conditions>` — shared
/// by the page query and the first-page count.
fn issue_base(input: &IssueCompileInput<'_>) -> Result<SelectStatement, CompileError> {
    let mut q = Query::select();
    q.from(issue::Entity);
    q.inner_join(
        series::Entity,
        Expr::col((series::Entity, series::Column::Id))
            .equals((issue::Entity, issue::Column::SeriesId)),
    );
    q.and_where(Expr::col((issue::Entity, issue::Column::State)).eq("active"));
    q.and_where(Expr::col((issue::Entity, issue::Column::RemovedAt)).is_null());
    apply_issue_visibility(&mut q, &input.visible_libraries);
    let ctx = Ctx {
        entity: ViewEntity::Issue,
        user_id: input.user_id,
    };
    apply_conditions(&mut q, input.dsl, &ctx)?;
    Ok(q)
}

/// Compile an issue-level (`filter_issues`) view. Selects active,
/// non-removed issues the caller can see, joined to their parent series
/// (series-column fields like `name` / `status` evaluate against it).
/// Fetches `limit + 1` rows after `cursor`; the caller pops the extra row
/// and encodes `next_cursor` from the last row it returns.
pub fn compile_issues(input: &IssueCompileInput<'_>) -> Result<SelectStatement, CompileError> {
    let keys = issue_sort_keys(input.sort_field)?;
    let mut q = issue_base(input)?;
    for col in ISSUE_CARD_COLUMNS {
        q.column((issue::Entity, *col));
    }
    q.expr_as(
        Expr::col((series::Entity, series::Column::Slug)),
        Alias::new("series_slug"),
    );
    q.expr_as(
        Expr::col((series::Entity, series::Column::Name)),
        Alias::new("series_name"),
    );
    if let Some(c) = &input.cursor {
        q.and_where(issue_keyset_predicate(keys, c, input.sort_order)?);
    }
    let order = match input.sort_order {
        SortOrder::Asc => Order::Asc,
        SortOrder::Desc => Order::Desc,
    };
    for k in keys {
        let expr: SimpleExpr = Expr::cust(k.sql);
        if k.nullable {
            q.order_by_expr_with_nulls(expr, order.clone(), NullOrdering::Last);
        } else {
            q.order_by_expr(expr, order.clone());
        }
    }
    q.order_by((issue::Entity, issue::Column::Id), order);
    q.limit(input.limit + 1);
    Ok(q)
}

/// `SELECT COUNT(*) AS total` over the same filtered set — the issue-view
/// results endpoint runs it on the first page only.
pub fn compile_issues_count(
    input: &IssueCompileInput<'_>,
) -> Result<SelectStatement, CompileError> {
    let mut q = issue_base(input)?;
    q.expr_as(
        Func::count(Expr::col((issue::Entity, issue::Column::Id))),
        Alias::new("total"),
    );
    Ok(q)
}

/// Validate a DSL + sort for a view of `entity` without running it — the
/// create / update handlers call this before persisting.
pub fn validate(
    dsl: &FilterDsl,
    entity: ViewEntity,
    sort_field: SortField,
    sort_order: SortOrder,
) -> Result<(), CompileError> {
    match entity {
        ViewEntity::Series => compile(&CompileInput {
            dsl,
            sort_field,
            sort_order,
            limit: 12,
            cursor: None,
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        })
        .map(|_| ()),
        ViewEntity::Issue => compile_issues(&IssueCompileInput {
            dsl,
            sort_field,
            sort_order,
            limit: 12,
            cursor: None,
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        })
        .map(|_| ()),
    }
}

/// Per-compile context threaded into every condition: which root the SQL
/// targets and whose per-user state (ratings, progress) to read.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    entity: ViewEntity,
    user_id: Uuid,
}

impl Ctx {
    /// `series.id` / `issues.id` — the row a junction or per-user
    /// subquery correlates against.
    fn root_id_sql(self) -> &'static str {
        match self.entity {
            ViewEntity::Series => "series.id",
            ViewEntity::Issue => "issues.id",
        }
    }

    /// The junction-table column that points at the root row.
    fn junction_key(self) -> &'static str {
        match self.entity {
            ViewEntity::Series => "series_id",
            ViewEntity::Issue => "issue_id",
        }
    }
}

fn apply_conditions(
    q: &mut SelectStatement,
    dsl: &FilterDsl,
    ctx: &Ctx,
) -> Result<(), CompileError> {
    let mut combined = match dsl.match_mode {
        MatchMode::All => SeaCondition::all(),
        MatchMode::Any => SeaCondition::any(),
    };
    for cond in &dsl.conditions {
        combined = combined.add(compile_condition(cond, ctx)?);
    }
    if !dsl.conditions.is_empty() {
        q.cond_where(combined);
    }
    Ok(())
}

fn apply_issue_visibility(q: &mut SelectStatement, vis: &VisibleLibraries) {
    if vis.unrestricted {
        return;
    }
    if vis.allowed.is_empty() {
        q.and_where(Expr::val(false));
        return;
    }
    let allowed: Vec<Uuid> = vis.allowed.iter().copied().collect();
    q.and_where(Expr::col((issue::Entity, issue::Column::LibraryId)).is_in(allowed));
    // WP-2.7: age-rating cap on the issue's own rating, series fallback.
    if let Some(cap) = vis.issue_cap_condition() {
        q.cond_where(cap);
    }
}

fn apply_visibility(q: &mut SelectStatement, vis: &VisibleLibraries) {
    if vis.unrestricted {
        return;
    }
    if vis.allowed.is_empty() {
        q.and_where(Expr::val(false));
        return;
    }
    let allowed: Vec<Uuid> = vis.allowed.iter().copied().collect();
    q.and_where(Expr::col((series::Entity, series::Column::LibraryId)).is_in(allowed));
    // WP-2.7: age-rating cap on `series.age_rating` (unrated rows pass).
    if let Some(cap) = vis.series_cap_condition() {
        q.cond_where(cap);
    }
}

fn needs_reading_join(dsl: &FilterDsl, sort: SortField) -> bool {
    if matches!(sort, SortField::LastRead | SortField::ReadProgress) {
        return true;
    }
    dsl.conditions.iter().any(|c| {
        let spec = registry::spec_for(c.field);
        matches!(
            spec.source,
            Some(Source::Reading(_) | Source::ReadingComputed(_))
        )
    })
}

/// library-filters-richer-1.0: the active-issue-count aggregate is
/// joined whenever a filter references a field that needs the
/// series-level total. M4's `collection_completeness` uses it
/// directly; M3's `unread_issues` falls back to it for series the user
/// has never started (where `user_series_progress` has no row).
fn needs_active_issue_count_join(dsl: &FilterDsl) -> bool {
    dsl.conditions.iter().any(|c| {
        matches!(
            registry::spec_for(c.field).source,
            Some(
                Source::SeriesComputed("collection_completeness")
                    | Source::ReadingComputed("unread_issues")
            ),
        )
    })
}

/// Alias for the active-issue-count subquery; centralized so
/// `series_computed_predicate` doesn't sprinkle string literals.
fn active_issue_count_alias() -> Alias {
    Alias::new("aic")
}

/// Whether any filter references `metadata_completeness` — gates the
/// metadata-completeness aggregate join (mirrors
/// [`needs_active_issue_count_join`]).
fn needs_metadata_completeness_join(dsl: &FilterDsl) -> bool {
    dsl.conditions.iter().any(|c| {
        matches!(
            registry::spec_for(c.field).source,
            Some(Source::SeriesComputed("metadata_completeness")),
        )
    })
}

/// Alias for the metadata-completeness aggregate subquery.
fn metadata_completeness_alias() -> Alias {
    Alias::new("mca")
}

/// `SELECT series_id, COUNT(*) AS active_count, COUNT(*) FILTER (issue core
/// met) AS complete_count FROM issues … GROUP BY series_id`. The FILTER
/// predicate mirrors `api::series::compute_metadata_completeness_summary` and
/// `assess_issue_view` exactly — credits from the per-role CSV columns, the
/// provider match from a CV/Metron `external_ids` row — so the saved-view
/// filter, the series rollup, and the per-issue detail report never disagree.
fn metadata_completeness_subquery() -> SelectStatement {
    use entity::issue;
    Query::select()
        .column(issue::Column::SeriesId)
        .expr_as(
            Func::count(Expr::col((issue::Entity, issue::Column::Id))),
            Alias::new("active_count"),
        )
        .expr_as(
            Expr::cust(
                // An operator "mark complete" acknowledgement (B4) counts as
                // satisfied, so an all-accepted series leaves the
                // needs-metadata worklist — keeps this in lockstep with the
                // two rollups in `api::series`. `title` intentionally excluded.
                "COUNT(*) FILTER (WHERE \
                 issues.metadata_review_accepted_at IS NOT NULL OR ( \
                 issues.year IS NOT NULL AND issues.year >= 1800 \
                 AND COALESCE(btrim(issues.summary), '') <> '' \
                 AND issues.page_count IS NOT NULL AND issues.page_count > 0 \
                 AND (COALESCE(issues.writer, '') <> '' \
                   OR COALESCE(issues.penciller, '') <> '' \
                   OR COALESCE(issues.inker, '') <> '' \
                   OR COALESCE(issues.colorist, '') <> '' \
                   OR COALESCE(issues.letterer, '') <> '' \
                   OR COALESCE(issues.cover_artist, '') <> '' \
                   OR COALESCE(issues.editor, '') <> '' \
                   OR COALESCE(issues.translator, '') <> '') \
                 AND EXISTS (SELECT 1 FROM external_ids x \
                   WHERE x.entity_type = 'issue' AND x.entity_id = issues.id \
                   AND x.source IN ('comicvine', 'metron'))))",
            ),
            Alias::new("complete_count"),
        )
        .from(issue::Entity)
        .and_where(Expr::col(issue::Column::State).eq("active"))
        .and_where(Expr::col(issue::Column::RemovedAt).is_null())
        .add_group_by([Expr::col(issue::Column::SeriesId)])
        .to_owned()
}

/// `SELECT series_id, COUNT(*) AS active_count, COUNT(*) FILTER (WHERE
/// special_type IS NULL) AS main_count FROM issues WHERE state = 'active'
/// AND removed_at IS NULL GROUP BY series_id`. Joined LEFT so series with
/// zero on-disk issues land as NULL → COALESCE'd to 0. `main_count` is the
/// main run alone — what `series.total_issues` describes — so the
/// completeness predicate ignores annuals / specials, matching
/// `SeriesView.main_issue_count`.
fn active_issue_count_subquery() -> SelectStatement {
    use entity::issue;
    Query::select()
        .column(issue::Column::SeriesId)
        .expr_as(
            Func::count(Expr::col((issue::Entity, issue::Column::Id))),
            Alias::new("active_count"),
        )
        .expr_as(
            Expr::cust("COUNT(*) FILTER (WHERE special_type IS NULL)"),
            Alias::new("main_count"),
        )
        .from(issue::Entity)
        .and_where(Expr::col(issue::Column::State).eq("active"))
        .and_where(Expr::col(issue::Column::RemovedAt).is_null())
        .add_group_by([Expr::col(issue::Column::SeriesId)])
        .to_owned()
}

fn compile_condition(cond: &Condition, ctx: &Ctx) -> Result<SeaCondition, CompileError> {
    let spec = registry::spec_for(cond.field);
    let Some(source) = registry::source_for(spec, ctx.entity) else {
        return Err(CompileError::FieldNotAvailable {
            field: cond.field,
            entity: ctx.entity,
        });
    };
    if !spec.allowed_ops.contains(&cond.op) {
        return Err(CompileError::OpNotAllowedForField(cond.field, cond.op));
    }
    match source {
        Source::Series(col) => series_predicate(cond, spec.kind, col),
        Source::Issue(col) => issue_predicate(cond, spec.kind, col),
        Source::Reading(col) => reading_predicate(cond, spec.kind, col),
        Source::ReadingComputed(tag) => reading_computed_predicate(cond, spec.kind, tag),
        Source::SeriesComputed(tag) => series_computed_predicate(cond, spec.kind, tag),
        Source::IssueComputed(tag) => issue_computed_predicate(cond, spec.kind, tag, ctx),
        Source::UserRating => {
            let lhs = user_rating_expr(ctx);
            Ok(SeaCondition::all().add(scalar_predicate(cond, spec.kind, lhs)?))
        }
        Source::JunctionExists {
            table,
            value_col,
            role,
        } => junction_predicate(cond, table, value_col, role, ctx),
        Source::MarkerExists(kind) => marker_predicate(cond, kind, ctx),
    }
}

/// WP-5.7: "has my notes / bookmarks / highlights". Markers are per-user,
/// so the EXISTS is always scoped to the viewing user (`$1`) — another
/// user's notes never make a row match. Series views match a series with
/// such a marker on any of its issues; markers on removed issues don't
/// count (they're unreachable from the library). `kind` comes from the
/// registry, never from user input.
fn marker_predicate(
    cond: &Condition,
    kind: &'static str,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let owner = match ctx.entity {
        ViewEntity::Series => "m.series_id = series.id",
        ViewEntity::Issue => "m.issue_id = issues.id",
    };
    // `favorite` (WP-8.4) is both a kind and a flag on any marker; match
    // either, like the /bookmarks "Favorites" chip.
    let kind_clause = if kind == "favorite" {
        "(m.kind = 'favorite' OR m.is_favorite)".to_owned()
    } else {
        format!("m.kind = '{kind}'")
    };
    let exists = format!(
        "EXISTS (SELECT 1 FROM markers m \
         JOIN issues mi ON mi.id = m.issue_id AND mi.removed_at IS NULL \
         WHERE m.user_id = $1 AND {kind_clause} AND {owner})"
    );
    let expr = match cond.op {
        Op::IsTrue => Expr::cust_with_values(exists, [ctx.user_id]),
        Op::IsFalse => Expr::cust_with_values(format!("NOT {exists}"), [ctx.user_id]),
        _ => return Err(CompileError::OpNotAllowedForField(cond.field, cond.op)),
    };
    Ok(SeaCondition::all().add(expr))
}

fn series_predicate(
    cond: &Condition,
    kind: FieldKind,
    col: &'static str,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = Expr::col((series::Entity, Alias::new(col)));
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

fn issue_predicate(
    cond: &Condition,
    kind: FieldKind,
    col: &'static str,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = Expr::col((issue::Entity, Alias::new(col)));
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

/// The caller's own star rating for the root row — a scalar subquery so
/// an unrated row is NULL (`is_empty`) and every comparison op drops it.
/// Series ratings key `target_id` on the UUID rendered as text; issue
/// ratings on the BLAKE3 id (`entity::user_rating`).
fn user_rating_expr(ctx: &Ctx) -> SimpleExpr {
    let (target_type, target_id) = match ctx.entity {
        ViewEntity::Series => ("series", "series.id::text"),
        ViewEntity::Issue => ("issue", "issues.id"),
    };
    Expr::cust_with_values(
        format!(
            "(SELECT ur.rating FROM user_ratings ur \
             WHERE ur.user_id = $1 AND ur.target_type = '{target_type}' \
             AND ur.target_id = {target_id})"
        ),
        [ctx.user_id],
    )
}

/// WP-5.4: derived per-user fields on issue views. `read_status` is the
/// per-issue three-state rollup `GET /issues?read_status=` uses verbatim
/// (`api::issues::apply_issue_read_status_filter`): `finished` → read,
/// otherwise `last_page > 0` → in_progress, else (including no progress
/// row at all) unread.
fn issue_computed_predicate(
    cond: &Condition,
    kind: FieldKind,
    tag: &'static str,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = match tag {
        "read_status" => Expr::cust_with_values(
            "COALESCE((SELECT CASE \
                WHEN pr.finished THEN 'read' \
                WHEN pr.last_page > 0 THEN 'in_progress' \
                ELSE 'unread' END \
              FROM progress_records pr \
              WHERE pr.user_id = $1 AND pr.issue_id = issues.id), 'unread')",
            [ctx.user_id],
        ),
        _ => {
            return Err(CompileError::Internal(format!(
                "unknown IssueComputed tag `{tag}`"
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

fn reading_predicate(
    cond: &Condition,
    kind: FieldKind,
    col: &'static str,
) -> Result<SeaCondition, CompileError> {
    // Numeric reading columns COALESCE to 0 so unstarted series compare
    // false (not NULL) — keeps "Read Progress >= 50" excluding them
    // without surprising three-valued logic at the predicate boundary.
    // Dates stay nullable: `lt`/`gt`/etc. naturally drop NULL rows.
    let usp = series_progress::subquery_alias();
    let raw: SimpleExpr = Expr::col((usp, Alias::new(col)));
    let lhs: SimpleExpr = match kind {
        FieldKind::Number => Func::coalesce([raw, Expr::val(0_i64)]).into(),
        FieldKind::Date => raw,
        _ => {
            return Err(CompileError::Internal(format!(
                "reading source mapped to unsupported kind {kind:?}",
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

/// library-filters-richer-1.0 M2 + M3: derived per-user fields over the
/// `user_series_progress` LEFT JOIN. The `tag` selects which expression
/// to evaluate as the LHS — `read_status` is the three-state CASE rollup,
/// `unread_issues` is `total_count - finished_count`. Both COALESCE the
/// missing-row case (series the user never started → no row in usp) to
/// the "unread" / "all-remaining" end of the spectrum.
fn reading_computed_predicate(
    cond: &Condition,
    kind: FieldKind,
    tag: &'static str,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = match tag {
        "read_status" => {
            // Three-state rollup. The `user_series_progress` view only
            // has a row when the user has at least one `progress_record`
            // for the series, so `usp.total_count IS NULL` (LEFT JOIN
            // miss) means "never touched" → 'unread'. The row's
            // existence implies the user has started at least one
            // issue, so when nothing is finished yet we report
            // 'in_progress' — not 'unread'.
            //
            // Raw SQL via `Expr::cust` rather than sea_query's case
            // builder because the latter doesn't compose with the
            // multi-column predicate we want for the WHERE clause.
            // The CASE is self-contained — no user-controlled input
            // substitution.
            Expr::cust(
                "CASE \
                 WHEN usp.total_count IS NULL THEN 'unread' \
                 WHEN usp.total_count = 0 THEN 'unread' \
                 WHEN usp.finished_count >= usp.total_count THEN 'read' \
                 ELSE 'in_progress' END",
            )
        }
        "unread_issues" => {
            // `total - finished`, with `total` falling back to the
            // series-level active-issue count when `user_series_progress`
            // has no row for this (user, series) pair. Result: filtering
            // for "unread_issues > 5" includes series the user has
            // never touched if the series itself has > 5 issues.
            Expr::cust(
                "(COALESCE(usp.total_count, aic.active_count, 0) \
                  - COALESCE(usp.finished_count, 0))",
            )
        }
        _ => {
            return Err(CompileError::Internal(format!(
                "unknown ReadingComputed tag `{tag}`"
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

/// library-filters-richer-1.0 M4: derived series-level fields backed by
/// the active-issue-count aggregate (`aic`). The `tag` selects which
/// predicate to emit — currently only `collection_completeness`. NULL
/// handling: `series.total_issues IS NULL` always maps to `unknown`
/// regardless of how many issues are on disk.
fn series_computed_predicate(
    cond: &Condition,
    kind: FieldKind,
    tag: &'static str,
) -> Result<SeaCondition, CompileError> {
    match tag {
        "collection_completeness" => {
            // Evaluates against `series.total_issues` (canonical expected
            // count from ComicInfo) and `aic.main_count` (current on-disk
            // main-run count — specials excluded, NULL when zero —
            // COALESCE'd to 0).
            let lhs: SimpleExpr = Expr::cust(
                "CASE \
                 WHEN series.total_issues IS NULL THEN 'unknown' \
                 WHEN COALESCE(aic.main_count, 0) >= series.total_issues THEN 'complete' \
                 ELSE 'incomplete' END",
            );
            Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
        }
        "metadata_completeness" => {
            // Evaluates against the metadata-completeness aggregate (`mca`):
            // `complete` when every active issue meets the issue core
            // criteria, `needs_metadata` when none do (or the series has no
            // active issues), `partial` otherwise. Mirrors the per-issue
            // `CompletenessTier` rollup.
            let lhs: SimpleExpr = Expr::cust(
                "CASE \
                 WHEN COALESCE(mca.active_count, 0) = 0 THEN 'needs_metadata' \
                 WHEN COALESCE(mca.complete_count, 0) >= mca.active_count THEN 'complete' \
                 WHEN COALESCE(mca.complete_count, 0) = 0 THEN 'needs_metadata' \
                 ELSE 'partial' END",
            );
            Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
        }
        _ => Err(CompileError::Internal(format!(
            "unknown SeriesComputed tag `{tag}`"
        ))),
    }
}

fn scalar_predicate(
    cond: &Condition,
    kind: FieldKind,
    lhs: SimpleExpr,
) -> Result<SimpleExpr, CompileError> {
    let v = &cond.value;
    let bad = |reason: &str| CompileError::BadValue {
        field: cond.field,
        op: cond.op,
        reason: reason.to_owned(),
    };
    match cond.op {
        // WP-5.4: NULL-ness. Text treats blank as empty too — ComicInfo
        // round-trips often leave `<Format></Format>`-style empty strings.
        Op::IsEmpty => Ok(match kind {
            FieldKind::Text => btrim_or_empty(lhs).eq(""),
            _ => lhs.is_null(),
        }),
        Op::IsNotEmpty => Ok(match kind {
            FieldKind::Text => btrim_or_empty(lhs).ne(""),
            _ => lhs.is_not_null(),
        }),
        Op::Equals | Op::Is => Ok(lhs.eq(scalar_value(v, kind, &bad)?)),
        Op::NotEquals | Op::IsNot => Ok(lhs.ne(scalar_value(v, kind, &bad)?)),
        Op::Contains => Ok(lhs.like(format!("%{}%", as_text(v, &bad)?))),
        Op::NotContains => {
            // NULL handling: `column NOT LIKE 'pattern'` returns NULL on
            // NULL inputs, so rows with the text column NULL are
            // excluded. Mirrors `Op::NotEquals` semantics — by design.
            // Documented in docs/dev/saved-views.md (M5 of
            // library-filters-richer-1.0).
            Ok(lhs.not_like(format!("%{}%", as_text(v, &bad)?)))
        }
        Op::StartsWith => Ok(lhs.like(format!("{}%", as_text(v, &bad)?))),
        Op::Gt | Op::After => Ok(lhs.gt(scalar_value(v, kind, &bad)?)),
        Op::Gte => Ok(lhs.gte(scalar_value(v, kind, &bad)?)),
        Op::Lt | Op::Before => Ok(lhs.lt(scalar_value(v, kind, &bad)?)),
        Op::Lte => Ok(lhs.lte(scalar_value(v, kind, &bad)?)),
        Op::Between => {
            let arr = v.as_array().ok_or_else(|| bad("expected [lo, hi] array"))?;
            if arr.len() != 2 {
                return Err(bad("between expects exactly 2 elements"));
            }
            let lo = scalar_value(&arr[0], kind, &bad)?;
            let hi = scalar_value(&arr[1], kind, &bad)?;
            Ok(SimpleExpr::Binary(
                Box::new(lhs.clone().gte(lo)),
                BinOper::And,
                Box::new(lhs.lte(hi)),
            ))
        }
        Op::In => Ok(lhs.is_in(scalar_array(v, kind, &bad)?)),
        Op::NotIn => Ok(lhs.is_not_in(scalar_array(v, kind, &bad)?)),
        Op::Relative => {
            let n = v.as_i64().ok_or_else(|| bad("expected integer days"))?;
            if n <= 0 {
                return Err(bad("relative days must be positive"));
            }
            // Postgres: `NOW() - INTERVAL 'N days'`. Interval composed at
            // SQL layer with a bound integer.
            let cutoff = Expr::cust_with_values("NOW() - ($1 || ' days')::interval", [n]);
            Ok(lhs.gte(cutoff))
        }
        Op::IsTrue => Ok(lhs.eq(true)),
        Op::IsFalse => Ok(lhs.eq(false)),
        Op::IncludesAny | Op::IncludesAll | Op::Excludes => Err(bad(
            "multi-set ops belong on junction-backed fields, not scalars",
        )),
    }
}

/// `btrim(COALESCE(lhs, ''))` — the text-emptiness probe.
fn btrim_or_empty(lhs: SimpleExpr) -> SimpleExpr {
    Func::cust(Alias::new("btrim"))
        .arg(Func::coalesce([lhs, Expr::val("")]))
        .into()
}

fn junction_predicate(
    cond: &Condition,
    table: &'static str,
    value_col: &'static str,
    role: Option<&'static str>,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let bad = |reason: &str| CompileError::BadValue {
        field: cond.field,
        op: cond.op,
        reason: reason.to_owned(),
    };
    // Whole SQL fragment is built from compile-time-static identifiers
    // (table, column, role come from the registry, not user input). User
    // values are bound through `cust_with_values`.
    let root_id = ctx.root_id_sql();
    let key = ctx.junction_key();
    let role_clause = role
        .map(|r| format!(" AND {table}.role = '{r}'"))
        .unwrap_or_default();
    let any_row = format!("SELECT 1 FROM {table} WHERE {table}.{key} = {root_id}{role_clause}");
    // WP-5.4: "has no genres" / "has any writer" — no value needed.
    match cond.op {
        Op::IsEmpty => {
            return Ok(SeaCondition::all().add(Expr::cust(format!("NOT EXISTS ({any_row})"))));
        }
        Op::IsNotEmpty => {
            return Ok(SeaCondition::all().add(Expr::cust(format!("EXISTS ({any_row})"))));
        }
        _ => {}
    }
    let values = cond
        .value
        .as_array()
        .ok_or_else(|| bad("expected array of strings"))?;
    if values.is_empty() {
        return Err(bad("at least one value required"));
    }
    let strs: Vec<String> = values
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| bad("array elements must be strings"))
                .map(str::to_owned)
        })
        .collect::<Result<_, _>>()?;

    match cond.op {
        Op::IncludesAny => {
            let sql = format!("EXISTS ({any_row} AND {table}.{value_col} = ANY($1))");
            Ok(SeaCondition::all().add(Expr::cust_with_values(sql, [strs])))
        }
        Op::Excludes => {
            let sql = format!("NOT EXISTS ({any_row} AND {table}.{value_col} = ANY($1))");
            Ok(SeaCondition::all().add(Expr::cust_with_values(sql, [strs])))
        }
        Op::IncludesAll => {
            let mut all = SeaCondition::all();
            for s in strs {
                let sql = format!("EXISTS ({any_row} AND {table}.{value_col} = $1)");
                all = all.add(Expr::cust_with_values(sql, [s]));
            }
            Ok(all)
        }
        _ => Err(bad(
            "multi field only supports includes_any, includes_all, excludes",
        )),
    }
}

fn sort_expression(field: SortField, order: SortOrder) -> (SimpleExpr, Order) {
    let order_sea = match order {
        SortOrder::Asc => Order::Asc,
        SortOrder::Desc => Order::Desc,
    };
    let expr: SimpleExpr = match field {
        SortField::Name => Expr::col((series::Entity, series::Column::Name)),
        SortField::Year => Expr::col((series::Entity, series::Column::Year)),
        SortField::CreatedAt => Expr::col((series::Entity, series::Column::CreatedAt)),
        SortField::UpdatedAt => Expr::col((series::Entity, series::Column::UpdatedAt)),
        // `last_read_at` from `user_series_progress` sources from
        // `reading_sessions.last_heartbeat_at` only — so a series
        // touched solely by bulk-mark / sync writes (which don't
        // emit sessions) has `last_read_at = NULL`. Postgres' default
        // NULLs-first ordering on DESC would push every fully-
        // bulk-marked series to the *top* of "Just Finished", which is
        // precisely the misleading-stats case the backfill scope
        // exists to address. COALESCE to epoch so those series sort
        // last in DESC (and first in ASC), naturally matching the
        // user's mental model of "no real reading activity = least
        // recent". The original column value returned to the client
        // is unaffected.
        SortField::LastRead => Func::coalesce([
            Expr::col((
                series_progress::subquery_alias(),
                Alias::new("last_read_at"),
            )),
            Expr::cust("TIMESTAMPTZ '1970-01-01'"),
        ])
        .into(),
        SortField::ReadProgress => Func::coalesce([
            Expr::col((series_progress::subquery_alias(), Alias::new("percent"))),
            Expr::val(0_i64),
        ])
        .into(),
    };
    (expr, order_sea)
}

fn apply_cursor(
    q: &mut SelectStatement,
    input: &CompileInput<'_>,
    sort_expr: SimpleExpr,
    order: Order,
) {
    let Some(c) = input.cursor.as_ref() else {
        return;
    };
    let id_col: SimpleExpr = Expr::col((series::Entity, series::Column::Id));
    let id_op = match order {
        Order::Asc => BinOper::GreaterThan,
        _ => BinOper::SmallerThan,
    };
    if c.sort_value.is_empty() {
        q.and_where(SimpleExpr::Binary(
            Box::new(id_col),
            id_op,
            Box::new(Expr::val(c.id)),
        ));
        return;
    }
    // The encoded cursor value is always wire-text. Bind it with the
    // type the sort column expects, otherwise Postgres raises
    // `operator does not exist: timestamp with time zone < text` (or
    // the integer equivalent for `year`).
    let cursor_value = cursor_value_expr(input.sort_field, &c.sort_value);
    let composite = SeaCondition::any()
        .add(SimpleExpr::Binary(
            Box::new(sort_expr.clone()),
            id_op,
            Box::new(cursor_value.clone()),
        ))
        .add(
            SeaCondition::all()
                .add(sort_expr.eq(cursor_value))
                .add(SimpleExpr::Binary(
                    Box::new(id_col),
                    id_op,
                    Box::new(Expr::val(c.id)),
                )),
        );
    q.cond_where(composite);
}

fn cursor_value_expr(field: SortField, raw: &str) -> SimpleExpr {
    match field {
        SortField::Name => Expr::val(raw.to_owned()),
        SortField::Year => raw
            .parse::<i32>()
            .map(Expr::val)
            .unwrap_or_else(|_| Expr::val(raw.to_owned())),
        SortField::CreatedAt | SortField::UpdatedAt => {
            Expr::val(raw.to_owned()).cast_as(Alias::new("timestamptz"))
        }
        // sort_value is empty for these — `apply_cursor`'s empty-string
        // branch handles them and never reaches this helper.
        SortField::LastRead | SortField::ReadProgress => Expr::val(raw.to_owned()),
    }
}

// ───── value coercion helpers ─────

fn scalar_value(
    v: &Value,
    kind: FieldKind,
    bad: &dyn Fn(&str) -> CompileError,
) -> Result<SimpleExpr, CompileError> {
    match kind {
        FieldKind::Text | FieldKind::Enum => Ok(Expr::val(as_text(v, bad)?)),
        FieldKind::Number => Ok(Expr::val(as_number(v, bad)?)),
        FieldKind::Date => Ok(Expr::val(as_text(v, bad)?)),
        FieldKind::Uuid => Ok(Expr::val(as_uuid(v, bad)?)),
        FieldKind::Multi => Err(bad("multi field requires array op")),
        FieldKind::Bool => Err(bad("boolean field takes is_true / is_false")),
    }
}

fn scalar_array(
    v: &Value,
    kind: FieldKind,
    bad: &dyn Fn(&str) -> CompileError,
) -> Result<Vec<SimpleExpr>, CompileError> {
    let arr = v.as_array().ok_or_else(|| bad("expected array"))?;
    if arr.is_empty() {
        return Err(bad("array must be non-empty"));
    }
    arr.iter().map(|el| scalar_value(el, kind, bad)).collect()
}

fn as_text(v: &Value, bad: &dyn Fn(&str) -> CompileError) -> Result<String, CompileError> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| bad("expected string"))
}

fn as_number(v: &Value, bad: &dyn Fn(&str) -> CompileError) -> Result<f64, CompileError> {
    v.as_f64().ok_or_else(|| bad("expected number"))
}

fn as_uuid(v: &Value, bad: &dyn Fn(&str) -> CompileError) -> Result<Uuid, CompileError> {
    let s = v.as_str().ok_or_else(|| bad("expected UUID string"))?;
    Uuid::parse_str(s).map_err(|e| bad(&format!("bad UUID: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::sea_query::PostgresQueryBuilder;
    use serde_json::json;

    fn make(d: FilterDsl) -> CompileInput<'static> {
        let leaked: &'static FilterDsl = Box::leak(Box::new(d));
        CompileInput {
            dsl: leaked,
            sort_field: SortField::CreatedAt,
            sort_order: SortOrder::Desc,
            limit: 12,
            cursor: None,
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        }
    }

    fn dsl_all(conditions: Vec<Condition>) -> FilterDsl {
        FilterDsl {
            match_mode: MatchMode::All,
            conditions,
        }
    }

    #[test]
    fn rejects_op_not_allowed_for_field() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Genres,
            op: Op::Gt,
            value: json!(5),
        }]);
        assert!(matches!(
            compile(&make(d)).unwrap_err(),
            CompileError::OpNotAllowedForField(_, _)
        ));
    }

    #[test]
    fn rejects_bad_value_shape_for_between() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Year,
            op: Op::Between,
            value: json!(2020),
        }]);
        assert!(matches!(
            compile(&make(d)).unwrap_err(),
            CompileError::BadValue { .. }
        ));
    }

    #[test]
    fn rejects_empty_array_for_in() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Status,
            op: Op::In,
            value: json!([]),
        }]);
        assert!(matches!(
            compile(&make(d)).unwrap_err(),
            CompileError::BadValue { .. }
        ));
    }

    #[test]
    fn empty_dsl_compiles_to_visibility_only_query() {
        let d = dsl_all(vec![]);
        let stmt = compile(&make(d)).expect("compiles");
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(sql.contains("ORDER BY"), "SQL: {sql}");
        assert!(sql.contains("LIMIT 13"), "SQL: {sql}");
    }

    #[test]
    fn includes_any_emits_exists_against_junction_table() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Genres,
            op: Op::IncludesAny,
            value: json!(["Horror", "Sci-Fi"]),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("EXISTS") && sql.contains("series_genres"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn excludes_emits_not_exists() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Tags,
            op: Op::Excludes,
            value: json!(["dnf"]),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("NOT EXISTS") && sql.contains("series_tags"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn characters_teams_locations_emit_exists_against_per_field_junction_table() {
        for (field, table) in [
            (Field::Characters, "series_characters"),
            (Field::Teams, "series_teams"),
            (Field::Locations, "series_locations"),
        ] {
            let d = dsl_all(vec![Condition {
                group_id: 0,
                field,
                op: Op::IncludesAny,
                value: json!(["Spider-Man"]),
            }]);
            let stmt = compile(&make(d)).unwrap();
            let sql = stmt.to_string(PostgresQueryBuilder);
            assert!(
                sql.contains("EXISTS") && sql.contains(table),
                "{field:?} should compile against {table}; got SQL: {sql}",
            );
        }
    }

    #[test]
    fn writer_filter_scopes_to_role_in_credits_table() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Writer,
            op: Op::IncludesAny,
            value: json!(["Brian K. Vaughan"]),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("series_credits") && sql.contains("'writer'"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn read_progress_join_added_when_referenced() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::ReadProgress,
            op: Op::Gte,
            value: json!(50),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("user_series_progress") && sql.contains("LEFT JOIN"),
            "SQL: {sql}"
        );
        assert!(sql.contains("COALESCE"), "SQL: {sql}");
    }

    #[test]
    fn no_reading_join_when_unreferenced() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::Year,
            op: Op::Gte,
            value: json!(2020),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(!sql.contains("user_series_progress"), "SQL: {sql}");
    }

    #[test]
    fn match_mode_any_combines_with_or() {
        let d = FilterDsl {
            match_mode: MatchMode::Any,
            conditions: vec![
                Condition {
                    group_id: 0,
                    field: Field::Year,
                    op: Op::Gte,
                    value: json!(2020),
                },
                Condition {
                    group_id: 0,
                    field: Field::Publisher,
                    op: Op::Equals,
                    value: json!("Image"),
                },
            ],
        };
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(sql.contains(" OR "), "SQL: {sql}");
    }

    #[test]
    fn relative_date_uses_now_minus_interval() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::CreatedAt,
            op: Op::Relative,
            value: json!(7),
        }]);
        let stmt = compile(&make(d)).unwrap();
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(sql.contains("NOW()"), "SQL: {sql}");
        assert!(sql.contains("interval"), "SQL: {sql}");
    }

    #[test]
    fn updated_at_cursor_casts_value_to_timestamptz() {
        // Cursor sort_value is encoded as RFC3339 text. Without an
        // explicit cast, Postgres rejects `timestamptz < text`.
        let leaked: &'static FilterDsl = Box::leak(Box::new(dsl_all(vec![])));
        let input = CompileInput {
            dsl: leaked,
            sort_field: SortField::UpdatedAt,
            sort_order: SortOrder::Desc,
            limit: 12,
            cursor: Some(Cursor {
                sort_value: "2026-05-08T22:42:18.758666+00:00".to_owned(),
                id: Uuid::nil(),
            }),
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        let stmt = compile(&input).expect("compiles");
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(sql.contains("CAST("), "expected CAST in cursor SQL: {sql}");
        assert!(sql.contains("AS timestamptz"), "SQL: {sql}");
    }

    #[test]
    fn year_cursor_binds_integer_value() {
        // Year is stored as integer; binding the cursor as a string
        // would raise `operator does not exist: integer < text`.
        let leaked: &'static FilterDsl = Box::leak(Box::new(dsl_all(vec![])));
        let input = CompileInput {
            dsl: leaked,
            sort_field: SortField::Year,
            sort_order: SortOrder::Asc,
            limit: 12,
            cursor: Some(Cursor {
                sort_value: "2024".to_owned(),
                id: Uuid::nil(),
            }),
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        let stmt = compile(&input).expect("compiles");
        let sql = stmt.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("> 2024") || sql.contains("= 2024"),
            "expected unquoted integer 2024 in cursor SQL: {sql}"
        );
        assert!(!sql.contains("'2024'"), "year should not be quoted: {sql}");
    }

    // ───── WP-5.4: issue-level views + is_empty / is_not_empty ─────

    fn cond(field: Field, op: Op, value: serde_json::Value) -> Condition {
        Condition {
            group_id: 0,
            field,
            op,
            value,
        }
    }

    fn issues_sql(conditions: Vec<Condition>) -> Result<String, CompileError> {
        issues_sql_sorted(conditions, SortField::CreatedAt)
    }

    fn issues_sql_sorted(
        conditions: Vec<Condition>,
        sort_field: SortField,
    ) -> Result<String, CompileError> {
        let dsl = dsl_all(conditions);
        let input = IssueCompileInput {
            dsl: &dsl,
            sort_field,
            sort_order: SortOrder::Desc,
            limit: 12,
            cursor: None,
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        compile_issues(&input).map(|s| s.to_string(PostgresQueryBuilder))
    }

    #[test]
    fn issue_root_selects_active_issues_joined_to_series() {
        let sql = issues_sql(vec![]).unwrap();
        assert!(sql.contains(r#"FROM "issues""#), "SQL: {sql}");
        assert!(sql.contains(r#"INNER JOIN "series""#), "SQL: {sql}");
        assert!(sql.contains(r#""issues"."state" = 'active'"#), "SQL: {sql}");
        assert!(
            sql.contains(r#""issues"."removed_at" IS NULL"#),
            "SQL: {sql}"
        );
        assert!(sql.contains("series_slug") && sql.contains("series_name"));
        // Card projection only — never the wide JSON columns.
        assert!(!sql.contains("comic_info_raw"), "SQL: {sql}");
        assert!(sql.contains("LIMIT 13"), "SQL: {sql}");
    }

    #[test]
    fn unread_annuals_2019_compiles_against_issue_columns() {
        let sql = issues_sql(vec![
            cond(Field::SpecialType, Op::Is, json!("Annual")),
            cond(Field::Year, Op::Equals, json!(2019)),
            cond(Field::ReadStatus, Op::Is, json!("unread")),
        ])
        .unwrap();
        assert!(
            sql.contains(r#""issues"."special_type" = 'Annual'"#),
            "SQL: {sql}"
        );
        assert!(sql.contains(r#""issues"."year" = 2019"#), "SQL: {sql}");
        assert!(
            sql.contains("progress_records") && sql.contains("'unread'"),
            "SQL: {sql}"
        );
        // Per-issue read status never needs the per-series progress view.
        assert!(!sql.contains("user_series_progress"), "SQL: {sql}");
    }

    #[test]
    fn issue_only_field_rejected_on_series_views() {
        for field in [Field::SpecialType, Field::Format, Field::StoryArc] {
            let d = dsl_all(vec![cond(field, Op::IsEmpty, json!(null))]);
            assert_eq!(
                compile(&make(d)).unwrap_err(),
                CompileError::FieldNotAvailable {
                    field,
                    entity: ViewEntity::Series
                }
            );
        }
    }

    #[test]
    fn series_rollup_rejected_on_issue_views() {
        let err = issues_sql(vec![cond(Field::UnreadIssues, Op::Gt, json!(3))]).unwrap_err();
        assert_eq!(
            err,
            CompileError::FieldNotAvailable {
                field: Field::UnreadIssues,
                entity: ViewEntity::Issue
            }
        );
    }

    #[test]
    fn per_user_sort_rejected_on_issue_views() {
        let err = issues_sql_sorted(vec![], SortField::ReadProgress).unwrap_err();
        assert!(matches!(err, CompileError::SortNotAvailable { .. }));
    }

    #[test]
    fn issue_name_sort_orders_by_series_then_number() {
        let sql = issues_sql_sorted(vec![], SortField::Name).unwrap();
        let by_name = sql.find("series.name DESC").expect("series name");
        let by_num = sql
            .find("issues.sort_number DESC NULLS LAST")
            .expect("number");
        let by_id = sql.find(r#""issues"."id" DESC"#).expect("id");
        assert!(by_name < by_num && by_num < by_id, "SQL: {sql}");
    }

    #[test]
    fn series_column_fields_evaluate_against_parent_series_on_issue_views() {
        let sql = issues_sql(vec![cond(Field::Status, Op::Is, json!("ended"))]).unwrap();
        assert!(sql.contains(r#""series"."status" = 'ended'"#), "SQL: {sql}");
    }

    #[test]
    fn issue_junction_fields_correlate_on_issue_id() {
        let sql = issues_sql(vec![cond(
            Field::Writer,
            Op::IncludesAny,
            json!(["Ed Brubaker"]),
        )])
        .unwrap();
        assert!(
            sql.contains("issue_credits.issue_id = issues.id")
                && sql.contains("issue_credits.role = 'writer'"),
            "SQL: {sql}"
        );
        assert!(!sql.contains("series_credits"), "SQL: {sql}");
    }

    #[test]
    fn is_empty_on_text_treats_blank_as_empty() {
        let sql = issues_sql(vec![cond(Field::StoryArc, Op::IsEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains(r#"btrim(COALESCE("issues"."story_arc", '')) = ''"#),
            "SQL: {sql}"
        );
        let sql = issues_sql(vec![cond(Field::Format, Op::IsNotEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains(r#"btrim(COALESCE("issues"."format", '')) <> ''"#),
            "SQL: {sql}"
        );
    }

    #[test]
    fn is_empty_on_scalar_is_null_check() {
        let sql = issues_sql(vec![cond(Field::SpecialType, Op::IsEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains(r#""issues"."special_type" IS NULL"#),
            "SQL: {sql}"
        );
        let d = dsl_all(vec![cond(Field::Imprint, Op::IsNotEmpty, json!(null))]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#"btrim(COALESCE("series"."imprint", '')) <> ''"#),
            "SQL: {sql}"
        );
        let d = dsl_all(vec![cond(Field::TotalIssues, Op::IsEmpty, json!(null))]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#""series"."total_issues" IS NULL"#),
            "SQL: {sql}"
        );
    }

    #[test]
    fn is_empty_on_junction_field_is_not_exists_any_row() {
        let d = dsl_all(vec![cond(Field::Genres, Op::IsEmpty, json!(null))]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(
                "NOT EXISTS (SELECT 1 FROM series_genres WHERE series_genres.series_id = series.id)"
            ),
            "SQL: {sql}"
        );
        let sql = issues_sql(vec![cond(Field::Characters, Op::IsNotEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains(
                "EXISTS (SELECT 1 FROM issue_characters WHERE issue_characters.issue_id = issues.id)"
            ) && !sql.contains("NOT EXISTS"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn is_empty_not_offered_on_computed_fields() {
        let d = dsl_all(vec![cond(Field::ReadStatus, Op::IsEmpty, json!(null))]);
        assert!(matches!(
            compile(&make(d)).unwrap_err(),
            CompileError::OpNotAllowedForField(Field::ReadStatus, Op::IsEmpty)
        ));
    }

    #[test]
    fn rating_reads_callers_own_rating_per_entity() {
        let d = dsl_all(vec![cond(Field::Rating, Op::Gte, json!(4))]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("ur.target_type = 'series'") && sql.contains("series.id::text"),
            "SQL: {sql}"
        );
        let sql = issues_sql(vec![cond(Field::Rating, Op::IsEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains("ur.target_type = 'issue'") && sql.contains("IS NULL"),
            "SQL: {sql}"
        );
    }

    // ───── WP-5.7: has_notes / has_bookmarks / has_highlights ─────

    #[test]
    fn has_notes_is_user_scoped_exists_on_both_entities() {
        let uid = Uuid::from_u128(0xabc);
        let dsl = dsl_all(vec![cond(Field::HasNotes, Op::IsTrue, json!(null))]);
        let series = compile(&CompileInput {
            user_id: uid,
            ..make(dsl.clone())
        })
        .unwrap()
        .to_string(PostgresQueryBuilder);
        assert!(
            series.contains("EXISTS (SELECT 1 FROM markers m")
                && series.contains("m.kind = 'note'")
                && series.contains("m.series_id = series.id")
                && series.contains(&uid.to_string()),
            "SQL: {series}"
        );
        assert!(!series.contains("NOT EXISTS"), "SQL: {series}");
        let input = IssueCompileInput {
            dsl: &dsl,
            sort_field: SortField::CreatedAt,
            sort_order: SortOrder::Desc,
            limit: 12,
            cursor: None,
            user_id: uid,
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        let issue = compile_issues(&input)
            .unwrap()
            .to_string(PostgresQueryBuilder);
        assert!(
            issue.contains("m.issue_id = issues.id") && issue.contains(&uid.to_string()),
            "SQL: {issue}"
        );
    }

    #[test]
    fn has_bookmarks_false_and_highlights_map_to_marker_kinds() {
        let sql = issues_sql(vec![
            cond(Field::HasBookmarks, Op::IsFalse, json!(null)),
            cond(Field::HasHighlights, Op::IsTrue, json!(null)),
        ])
        .unwrap();
        assert!(
            sql.contains("NOT EXISTS (SELECT 1 FROM markers m")
                && sql.contains("m.kind = 'bookmark'")
                && sql.contains("m.kind = 'highlight'"),
            "SQL: {sql}"
        );
        // Removed issues' markers never count.
        assert!(sql.contains("mi.removed_at IS NULL"), "SQL: {sql}");
    }

    #[test]
    fn has_favorites_matches_the_kind_or_the_flag() {
        let sql = issues_sql(vec![cond(Field::HasFavorites, Op::IsTrue, json!(null))]).unwrap();
        assert!(
            sql.contains("(m.kind = 'favorite' OR m.is_favorite)")
                && sql.contains("m.issue_id = issues.id")
                && sql.contains("mi.removed_at IS NULL"),
            "SQL: {sql}"
        );
        let uid = Uuid::from_u128(0xfa7);
        let dsl = dsl_all(vec![cond(Field::HasFavorites, Op::IsFalse, json!(null))]);
        let series = compile(&CompileInput {
            user_id: uid,
            ..make(dsl)
        })
        .unwrap()
        .to_string(PostgresQueryBuilder);
        assert!(
            series.contains("NOT EXISTS (SELECT 1 FROM markers m")
                && series.contains("m.series_id = series.id")
                && series.contains(&uid.to_string()),
            "SQL: {series}"
        );
    }

    #[test]
    fn marker_fields_reject_non_bool_ops() {
        let err = issues_sql(vec![cond(Field::HasNotes, Op::Equals, json!(true))]).unwrap_err();
        assert!(matches!(
            err,
            CompileError::OpNotAllowedForField(Field::HasNotes, Op::Equals)
        ));
    }

    #[test]
    fn title_field_filters_issue_title_on_issue_views_only() {
        let sql = issues_sql(vec![cond(Field::Title, Op::Contains, json!("Origin"))]).unwrap();
        assert!(
            sql.contains(r#""issues"."title" LIKE '%Origin%'"#),
            "SQL: {sql}"
        );
        let sql = issues_sql(vec![cond(Field::Title, Op::IsEmpty, json!(null))]).unwrap();
        assert!(
            sql.contains(r#"btrim(COALESCE("issues"."title", '')) = ''"#),
            "SQL: {sql}"
        );
        let d = dsl_all(vec![cond(Field::Title, Op::Contains, json!("x"))]);
        assert!(matches!(
            compile(&make(d)).unwrap_err(),
            CompileError::FieldNotAvailable {
                field: Field::Title,
                ..
            }
        ));
    }

    fn keyset_sql(sort_field: SortField, sort_order: SortOrder, cursor: IssueCursor) -> String {
        let dsl = dsl_all(vec![]);
        let input = IssueCompileInput {
            dsl: &dsl,
            sort_field,
            sort_order,
            limit: 2,
            cursor: Some(cursor),
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        compile_issues(&input)
            .unwrap()
            .to_string(PostgresQueryBuilder)
    }

    #[test]
    fn name_keyset_is_lexicographic_over_series_number_id() {
        let sql = keyset_sql(
            SortField::Name,
            SortOrder::Asc,
            IssueCursor {
                keys: vec![json!("Batman"), json!(2.0)],
                id: "abc".into(),
            },
        );
        assert!(sql.contains("series.name > 'Batman'"), "SQL: {sql}");
        assert!(
            sql.contains(
                "series.name = 'Batman' AND (issues.sort_number > 2 OR issues.sort_number IS NULL)"
            ),
            "SQL: {sql}"
        );
        assert!(
            sql.contains("series.name = 'Batman' AND issues.sort_number = 2 AND issues.id > 'abc'"),
            "SQL: {sql}"
        );
        assert!(!sql.contains("OFFSET"), "SQL: {sql}");
    }

    #[test]
    fn null_cursor_key_only_advances_within_the_null_tail() {
        // Year DESC NULLS LAST: after a NULL year only later NULL-year ids
        // remain — never a non-NULL year again.
        let sql = keyset_sql(
            SortField::Year,
            SortOrder::Desc,
            IssueCursor {
                keys: vec![json!(null)],
                id: "abc".into(),
            },
        );
        assert!(
            sql.contains("(issues.year IS NULL AND issues.id < 'abc')"),
            "SQL: {sql}"
        );
        assert!(!sql.contains("issues.year <"), "SQL: {sql}");
    }

    #[test]
    fn timestamp_cursor_binds_as_timestamptz() {
        let dsl = dsl_all(vec![]);
        let input = IssueCompileInput {
            dsl: &dsl,
            sort_field: SortField::CreatedAt,
            sort_order: SortOrder::Desc,
            limit: 2,
            cursor: Some(IssueCursor {
                keys: vec![json!("2026-05-08T22:42:18.758666+00:00")],
                id: "abc".into(),
            }),
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        let (_, values) = compile_issues(&input).unwrap().build(PostgresQueryBuilder);
        assert!(
            values
                .iter()
                .any(|v| matches!(v, sea_orm::Value::ChronoDateTimeWithTimeZone(Some(_)))),
            "{values:?}"
        );
    }

    #[test]
    fn malformed_cursor_is_invalid_cursor() {
        let dsl = dsl_all(vec![]);
        let mut input = IssueCompileInput {
            dsl: &dsl,
            sort_field: SortField::Name,
            sort_order: SortOrder::Asc,
            limit: 2,
            cursor: Some(IssueCursor {
                keys: vec![json!("only-one-key")],
                id: "abc".into(),
            }),
            user_id: Uuid::nil(),
            visible_libraries: VisibleLibraries::unrestricted(),
        };
        assert_eq!(
            compile_issues(&input).unwrap_err(),
            CompileError::InvalidCursor
        );
        input.cursor = Some(IssueCursor {
            keys: vec![json!(12), json!(1.0)],
            id: "abc".into(),
        });
        assert_eq!(
            compile_issues(&input).unwrap_err(),
            CompileError::InvalidCursor
        );
    }

    #[test]
    fn issue_cursor_round_trips_opaquely() {
        let ts = chrono::DateTime::parse_from_rfc3339("2026-05-08T22:42:18.758666+00:00").unwrap();
        let c = IssueCursor::for_row(SortField::Name, "id1", "Batman", Some(1.5), None, &ts, &ts);
        let token = c.encode();
        assert!(!token.contains("Batman"));
        assert_eq!(IssueCursor::decode(&token), Some(c));
        assert_eq!(IssueCursor::decode("not base64 json"), None);
    }

    #[test]
    fn validate_dispatches_on_entity() {
        let d = dsl_all(vec![cond(Field::SpecialType, Op::Is, json!("TPB"))]);
        assert!(validate(&d, ViewEntity::Issue, SortField::Year, SortOrder::Desc).is_ok());
        assert!(validate(&d, ViewEntity::Series, SortField::Year, SortOrder::Desc).is_err());
    }
}
