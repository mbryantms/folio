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
use crate::util::search::{contains_pattern, literal_like, prefix_pattern, suffix_pattern};
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

    // Remediation B9: whether the caller counts annuals / specials toward
    // completion. One LEFT JOIN to the caller's `users` row (the account
    // setting) and one to their per-series override, both on primary keys
    // — never a per-row subquery. Only for `collection_completeness`.
    if needs_completion_pref_join(input.dsl) {
        q.join_as(
            JoinType::LeftJoin,
            Alias::new("users"),
            completion_user_alias(),
            Expr::cust_with_values("cpu.id = $1", [input.user_id]),
        );
        q.join_as(
            JoinType::LeftJoin,
            Alias::new("user_series_completion_prefs"),
            completion_override_alias(),
            Expr::cust_with_values(
                "cpo.user_id = $1 AND cpo.series_id = series.id",
                [input.user_id],
            ),
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

    let (sort_expr, order_sea) =
        sort_expression(input.sort_field, input.sort_order, input.user_id)?;
    apply_cursor(&mut q, input, sort_expr.clone(), order_sea.clone());
    // The sort key rides along as text (`sort_value`) so the results
    // endpoint can encode a real keyset cursor for every sort — including
    // the per-user ones that aren't columns of `series`
    // (filter-parity-and-builtin-views-1.0; previously those fell back to
    // an id-only cursor and page 2 could skip rows).
    q.expr_as(
        sort_expr.clone().cast_as(Alias::new("text")),
        Alias::new("sort_value"),
    );
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
    pub fn for_row(sort: SortField, row: &IssueCursorRow<'_>) -> Self {
        let ts = |t: &Option<chrono::DateTime<chrono::FixedOffset>>| {
            serde_json::json!(t.as_ref().map(|t| t.to_rfc3339()))
        };
        let keys = match sort {
            SortField::Name => vec![
                Value::from(row.series_name),
                serde_json::json!(row.sort_number),
            ],
            SortField::Year => vec![
                serde_json::json!(row.year),
                serde_json::json!(row.month),
                serde_json::json!(row.day),
            ],
            SortField::CreatedAt => vec![Value::from(row.created_at.to_rfc3339())],
            SortField::UpdatedAt => vec![Value::from(row.updated_at.to_rfc3339())],
            // Per-user keys (filter-parity-and-builtin-views-1.0 M1) come
            // from the caller's joined `progress_records` / `user_ratings`
            // row; `null` when the caller never touched / rated the issue.
            SortField::LastRead => vec![ts(&row.last_read)],
            SortField::ReadProgress => vec![serde_json::json!(row.read_progress)],
            SortField::FinishedAt => vec![ts(&row.finished_at)],
            SortField::Rating => vec![serde_json::json!(row.rating)],
            SortField::PageCount => vec![serde_json::json!(row.page_count)],
            SortField::Number => vec![serde_json::json!(row.sort_number)],
        };
        Self {
            keys,
            id: row.id.to_owned(),
        }
    }
}

/// Every column an issue-view sort key can read, as the results endpoint
/// projects them (see `ISSUE_CARD_COLUMNS` + the per-user sort joins).
#[derive(Debug, Clone, Copy)]
pub struct IssueCursorRow<'a> {
    pub id: &'a str,
    pub series_name: &'a str,
    pub sort_number: Option<f64>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub page_count: Option<i32>,
    pub created_at: &'a chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: &'a chrono::DateTime<chrono::FixedOffset>,
    /// `sort_last_read` — the caller's `progress_records.updated_at`.
    pub last_read: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// `sort_read_progress` — the caller's `progress_records.percent`.
    pub read_progress: Option<f64>,
    /// `sort_finished_at` — the caller's `progress_records.finished_at`.
    pub finished_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// `sort_rating` — the caller's `user_ratings.rating`.
    pub rating: Option<f64>,
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

/// "Release date" of an issue: `(year, month, day)`, each nullable and
/// NULLS LAST, so the label is true for issues (post-audit T2.3). Shared
/// with the cross-library `GET /issues?sort=year` list.
const RELEASE_DATE_KEYS: &[IssueSortKey] = &[
    IssueSortKey {
        sql: "issues.year",
        ty: KeyType::Int,
        nullable: true,
    },
    IssueSortKey {
        sql: "issues.month",
        ty: KeyType::Int,
        nullable: true,
    },
    IssueSortKey {
        sql: "issues.day",
        ty: KeyType::Int,
        nullable: true,
    },
];

/// Keyset predicate "strictly after `(year, month, day, id)`" for the
/// release-date sort, for callers outside the view compiler (the
/// cross-library issue list). `asc` is the list direction; NULLs sort
/// last either way, matching [`release_date_order_sql`].
pub fn release_date_keyset(
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
    id: &str,
    asc: bool,
) -> SimpleExpr {
    let cursor = IssueCursor {
        keys: vec![
            serde_json::json!(year),
            serde_json::json!(month),
            serde_json::json!(day),
        ],
        id: id.to_owned(),
    };
    let order = if asc { SortOrder::Asc } else { SortOrder::Desc };
    // Key count and types are fixed here, so the only error paths
    // (`InvalidCursor`) are unreachable.
    issue_keyset_predicate(RELEASE_DATE_KEYS, &cursor, order).unwrap_or_else(|_| Expr::val(false))
}

/// `ORDER BY` terms for the release-date sort, NULLS LAST on every tier,
/// ending with the `issues.id` tiebreaker. Expressed as `(expr, asc)`
/// pairs so a sea-orm `Select` caller can chain `order_by`.
pub fn release_date_order_sql(asc: bool) -> Vec<(&'static str, bool)> {
    let mut out = Vec::with_capacity(RELEASE_DATE_KEYS.len() * 2 + 1);
    for k in RELEASE_DATE_KEYS {
        // `x IS NULL` ascending puts NULLs last regardless of direction.
        out.push((k.sql, asc));
    }
    out.push(("issues.id", asc));
    out
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
    const YEAR: &[IssueSortKey] = RELEASE_DATE_KEYS;
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
    // Per-user keys read the LEFT JOINs `compile_issues` adds on the
    // caller's `progress_records` (`pr`) / `user_ratings` (`ur`) rows.
    const LAST_READ: &[IssueSortKey] = &[IssueSortKey {
        sql: "pr.updated_at",
        ty: KeyType::Timestamp,
        nullable: true,
    }];
    const READ_PROGRESS: &[IssueSortKey] = &[IssueSortKey {
        sql: "pr.percent",
        ty: KeyType::Float,
        nullable: true,
    }];
    const FINISHED_AT: &[IssueSortKey] = &[IssueSortKey {
        sql: "pr.finished_at",
        ty: KeyType::Timestamp,
        nullable: true,
    }];
    const RATING: &[IssueSortKey] = &[IssueSortKey {
        sql: "ur.rating",
        ty: KeyType::Float,
        nullable: true,
    }];
    const PAGE_COUNT: &[IssueSortKey] = &[IssueSortKey {
        sql: "issues.page_count",
        ty: KeyType::Int,
        nullable: true,
    }];
    const NUMBER: &[IssueSortKey] = &[IssueSortKey {
        sql: "issues.sort_number",
        ty: KeyType::Float,
        nullable: true,
    }];
    match sort {
        SortField::Name => Ok(NAME),
        SortField::Year => Ok(YEAR),
        SortField::CreatedAt => Ok(CREATED),
        SortField::UpdatedAt => Ok(UPDATED),
        SortField::LastRead => Ok(LAST_READ),
        SortField::ReadProgress => Ok(READ_PROGRESS),
        SortField::FinishedAt => Ok(FINISHED_AT),
        SortField::Rating => Ok(RATING),
        SortField::PageCount => Ok(PAGE_COUNT),
        SortField::Number => Ok(NUMBER),
    }
}

/// Whether an issue-view sort reads the caller's `progress_records` row.
fn issue_sort_needs_progress(sort: SortField) -> bool {
    matches!(
        sort,
        SortField::LastRead | SortField::ReadProgress | SortField::FinishedAt
    )
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
    issue::Column::Month,
    issue::Column::Day,
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
    // Per-user sort keys (filter-parity-and-builtin-views-1.0 M1): join
    // the caller's own progress / rating row so the keyset can read it.
    // Both are PK lookups per issue; joined only for the sorts that read
    // them. The projected `sort_*` columns are always present (NULL when
    // unjoined) so the row projection stays one shape.
    if issue_sort_needs_progress(input.sort_field) {
        q.join_as(
            JoinType::LeftJoin,
            entity::progress_record::Entity,
            Alias::new("pr"),
            Expr::cust_with_values(
                "pr.user_id = $1 AND pr.issue_id = issues.id",
                [input.user_id],
            ),
        );
        q.expr_as(Expr::cust("pr.updated_at"), Alias::new("sort_last_read"));
        q.expr_as(Expr::cust("pr.percent"), Alias::new("sort_read_progress"));
        q.expr_as(Expr::cust("pr.finished_at"), Alias::new("sort_finished_at"));
    } else {
        q.expr_as(
            Expr::cust("NULL::timestamptz"),
            Alias::new("sort_last_read"),
        );
        q.expr_as(Expr::cust("NULL::float8"), Alias::new("sort_read_progress"));
        q.expr_as(
            Expr::cust("NULL::timestamptz"),
            Alias::new("sort_finished_at"),
        );
    }
    if input.sort_field == SortField::Rating {
        q.join_as(
            JoinType::LeftJoin,
            entity::user_rating::Entity,
            Alias::new("ur"),
            Expr::cust_with_values(
                "ur.user_id = $1 AND ur.target_type = 'issue' AND ur.target_id = issues.id",
                [input.user_id],
            ),
        );
        q.expr_as(Expr::cust("ur.rating"), Alias::new("sort_rating"));
    } else {
        q.expr_as(Expr::cust("NULL::float8"), Alias::new("sort_rating"));
    }
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
    // Soft-deleted series never surface on a rail, admin or not — the
    // same rule `api::series::list` applies (`access::active_series`).
    q.and_where(crate::library::access::active_series());
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

/// Whether a filter references `collection_completeness` — gates the two
/// completion-preference joins (remediation B9).
fn needs_completion_pref_join(dsl: &FilterDsl) -> bool {
    dsl.conditions.iter().any(|c| {
        matches!(
            registry::spec_for(c.field).source,
            Some(Source::SeriesComputed("collection_completeness")),
        )
    })
}

/// The caller's `users` row (account-level `count_specials_in_completeness`).
fn completion_user_alias() -> Alias {
    Alias::new("cpu")
}

/// The caller's `user_series_completion_prefs` row for the series.
fn completion_override_alias() -> Alias {
    Alias::new("cpo")
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
/// special_type IS NULL) AS main_count, COUNT(*) FILTER (WHERE
/// special_type IS NOT NULL) AS special_count FROM issues WHERE state =
/// 'active' AND removed_at IS NULL GROUP BY series_id`. Joined LEFT so
/// series with zero on-disk issues land as NULL → COALESCE'd to 0.
/// `main_count` is the main run alone — what `series.total_issues`
/// describes — matching `SeriesView.main_issue_count`; the completeness
/// predicate adds `special_count` only when the caller counts specials
/// (B9).
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
        .expr_as(
            Expr::cust("COUNT(*) FILTER (WHERE special_type IS NOT NULL)"),
            Alias::new("special_count"),
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
        Source::IssueReading(col) => issue_reading_predicate(cond, spec.kind, col, ctx),
        Source::ReadingAgg(tag) => reading_agg_predicate(cond, spec.kind, tag, ctx),
        Source::LookupJunctionExists {
            table,
            fk_col,
            lookup,
        } => lookup_junction_predicate(cond, table, fk_col, lookup, ctx),
        Source::ProviderMatch => {
            let lhs = provider_match_expr(ctx);
            Ok(SeaCondition::all().add(scalar_predicate(cond, spec.kind, lhs)?))
        }
        Source::RawExpr(sql) => {
            Ok(SeaCondition::all().add(scalar_predicate(cond, spec.kind, Expr::cust(sql))?))
        }
    }
}

/// filter-parity-and-builtin-views-1.0 M1: the caller's own
/// `progress_records` column for the issue, as a scalar subquery (one row
/// per `(user_id, issue_id)`). Numbers COALESCE to 0 so an unopened issue
/// compares false rather than NULL — "read progress < 50" keeps unread
/// issues, matching the series-level rule. Dates stay NULL (`is_empty`).
/// `read_count` is the completed-read count: `run` re-reads already
/// finished plus the current run when it is finished.
fn issue_reading_predicate(
    cond: &Condition,
    kind: FieldKind,
    col: &'static str,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let value = match col {
        "read_count" => "pr.run + CASE WHEN pr.finished THEN 1 ELSE 0 END",
        // `progress_records.percent` is a 0.0–1.0 fraction; the series
        // rollup (`user_series_progress.percent`) is 0–100, so scale to
        // keep one vocabulary ("read progress ≥ 50") on both entities.
        "percent" => "pr.percent * 100",
        "updated_at" => "pr.updated_at",
        "finished_at" => "pr.finished_at",
        _ => {
            return Err(CompileError::Internal(format!(
                "unknown IssueReading column `{col}`"
            )));
        }
    };
    let sub = format!(
        "(SELECT {value} FROM progress_records pr \
         WHERE pr.user_id = $1 AND pr.issue_id = issues.id)"
    );
    let raw: SimpleExpr = Expr::cust_with_values(sub, [ctx.user_id]);
    let lhs: SimpleExpr = match kind {
        FieldKind::Number => Func::coalesce([raw, Expr::val(0_f64)]).into(),
        FieldKind::Date => raw,
        _ => {
            return Err(CompileError::Internal(format!(
                "issue reading source mapped to unsupported kind {kind:?}",
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

/// Per-user aggregate over a series' active issues (series views). Tag
/// `finished_at` is the caller's most recent finish in the series — NULL
/// until any issue is finished, so `is_empty` means "never finished one".
fn reading_agg_predicate(
    cond: &Condition,
    kind: FieldKind,
    tag: &'static str,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = match tag {
        "finished_at" => Expr::cust_with_values(
            "(SELECT MAX(pr.finished_at) FROM progress_records pr \
              JOIN issues pi ON pi.id = pr.issue_id \
               AND pi.state = 'active' AND pi.removed_at IS NULL \
              WHERE pr.user_id = $1 AND pi.series_id = series.id)",
            [ctx.user_id],
        ),
        _ => {
            return Err(CompileError::Internal(format!(
                "unknown ReadingAgg tag `{tag}`"
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
}

/// `matched` / `unmatched`: whether the row has a provider id in
/// `external_ids` for its entity type, over the same source list the
/// admin metadata dashboard and the grid's `provider_match` facet use.
fn provider_match_expr(ctx: &Ctx) -> SimpleExpr {
    let (entity_type, entity_id) = match ctx.entity {
        ViewEntity::Series => ("series", "series.id::text"),
        ViewEntity::Issue => ("issue", "issues.id"),
    };
    let sources = crate::api::series::PROVIDER_SERIES_ID_SOURCES_SQL;
    Expr::cust(format!(
        "(CASE WHEN EXISTS (SELECT 1 FROM external_ids e \
            WHERE e.entity_type = '{entity_type}' AND e.entity_id = {entity_id} \
              AND e.source IN ({sources})) THEN 'matched' ELSE 'unmatched' END)"
    ))
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
        // Per-issue the metadata tier is binary: the issue either meets
        // the core criteria (or was accepted by an operator) or it
        // doesn't. `partial` only exists as a series rollup, so an issue
        // view filtering on it matches nothing — the web builder hides
        // that value on issue views.
        "metadata_completeness" => Expr::cust(format!(
            "(CASE WHEN {} THEN 'complete' ELSE 'needs_metadata' END)",
            crate::metadata::completeness::issue_metadata_satisfied_sql("issues")
        )),
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
            // main-run count, NULL when zero — COALESCE'd to 0). The
            // series' specials (`aic.special_count`) are added when the
            // caller counts them (B9): their per-series override `cpo`,
            // else their account setting `cpu`, else false.
            let lhs: SimpleExpr = Expr::cust(
                "CASE \
                 WHEN series.total_issues IS NULL THEN 'unknown' \
                 WHEN COALESCE(aic.main_count, 0) \
                   + CASE WHEN COALESCE(cpo.count_specials, \
                       cpu.count_specials_in_completeness, FALSE) \
                     THEN COALESCE(aic.special_count, 0) ELSE 0 END \
                   >= series.total_issues THEN 'complete' \
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
        // Text ops match the value literally (E9): the `*_pattern` helpers
        // escape `%` / `_` / `\` in the user's value and `literal_like`
        // adds `ESCAPE '\'`. Case-sensitive `LIKE`, as they always were.
        Op::Contains => Ok(lhs.like(literal_like(contains_pattern(&as_text(v, &bad)?)))),
        Op::NotContains => {
            // NULL handling: `column NOT LIKE 'pattern'` returns NULL on
            // NULL inputs, so rows with the text column NULL are
            // excluded. Mirrors `Op::NotEquals` semantics — by design.
            // Documented in docs/dev/saved-views.md (M5 of
            // library-filters-richer-1.0).
            Ok(lhs.not_like(literal_like(contains_pattern(&as_text(v, &bad)?))))
        }
        Op::StartsWith => Ok(lhs.like(literal_like(prefix_pattern(&as_text(v, &bad)?)))),
        Op::EndsWith => Ok(lhs.like(literal_like(suffix_pattern(&as_text(v, &bad)?)))),
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
        Op::NotRelative => {
            // "Older than N days". Strict `<` keeps three-valued semantics:
            // a NULL date (never read) is excluded, like `not_equals`.
            let n = v.as_i64().ok_or_else(|| bad("expected integer days"))?;
            if n <= 0 {
                return Err(bad("relative days must be positive"));
            }
            let cutoff = Expr::cust_with_values("NOW() - ($1 || ' days')::interval", [n]);
            Ok(lhs.lt(cutoff))
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
    // Whole SQL fragment is built from compile-time-static identifiers
    // (table, column, role come from the registry, not user input). User
    // values are bound through `cust_with_values`.
    let root_id = ctx.root_id_sql();
    let key = ctx.junction_key();
    let role_clause = role
        .map(|r| format!(" AND {table}.role = '{r}'"))
        .unwrap_or_default();
    let any_row = format!("SELECT 1 FROM {table} WHERE {table}.{key} = {root_id}{role_clause}");
    multi_predicate(cond, &any_row, &format!("{table}.{value_col}"))
}

/// Junction whose values live in a lookup table (story arcs / universes /
/// concepts / objects): the junction carries the lookup id, the lookup's
/// `name` is what the user picks. Same ops and NULL rules as
/// [`junction_predicate`].
fn lookup_junction_predicate(
    cond: &Condition,
    table: &'static str,
    fk_col: &'static str,
    lookup: &'static str,
    ctx: &Ctx,
) -> Result<SeaCondition, CompileError> {
    let root_id = ctx.root_id_sql();
    let key = ctx.junction_key();
    let any_row = format!(
        "SELECT 1 FROM {table} j JOIN {lookup} l ON l.id = j.{fk_col} WHERE j.{key} = {root_id}"
    );
    multi_predicate(cond, &any_row, "l.name")
}

/// Shared body of the multi-set ops over an `EXISTS (any_row …)` probe:
/// `is_empty` / `is_not_empty` need no value; `includes_any` / `excludes`
/// bind the whole array; `includes_all` ANDs one EXISTS per value.
fn multi_predicate(
    cond: &Condition,
    any_row: &str,
    value_expr: &str,
) -> Result<SeaCondition, CompileError> {
    let bad = |reason: &str| CompileError::BadValue {
        field: cond.field,
        op: cond.op,
        reason: reason.to_owned(),
    };
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
            let sql = format!("EXISTS ({any_row} AND {value_expr} = ANY($1))");
            Ok(SeaCondition::all().add(Expr::cust_with_values(sql, [strs])))
        }
        Op::Excludes => {
            let sql = format!("NOT EXISTS ({any_row} AND {value_expr} = ANY($1))");
            Ok(SeaCondition::all().add(Expr::cust_with_values(sql, [strs])))
        }
        Op::IncludesAll => {
            let mut all = SeaCondition::all();
            for s in strs {
                let sql = format!("EXISTS ({any_row} AND {value_expr} = $1)");
                all = all.add(Expr::cust_with_values(sql, [s]));
            }
            Ok(all)
        }
        _ => Err(bad(
            "multi field only supports includes_any, includes_all, excludes",
        )),
    }
}

fn sort_expression(
    field: SortField,
    order: SortOrder,
    user_id: Uuid,
) -> Result<(SimpleExpr, Order), CompileError> {
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
        // filter-parity-and-builtin-views-1.0: per-user scalar subqueries,
        // COALESCE'd like `last_read` so "nothing yet" sorts last on DESC.
        SortField::FinishedAt => Func::coalesce([
            Expr::cust_with_values(
                "(SELECT MAX(pr.finished_at) FROM progress_records pr \
                  JOIN issues pi ON pi.id = pr.issue_id \
                   AND pi.state = 'active' AND pi.removed_at IS NULL \
                  WHERE pr.user_id = $1 AND pi.series_id = series.id)",
                [user_id],
            ),
            Expr::cust("TIMESTAMPTZ '1970-01-01'"),
        ])
        .into(),
        SortField::Rating => Func::coalesce([
            Expr::cust_with_values(
                "(SELECT ur.rating FROM user_ratings ur \
                  WHERE ur.user_id = $1 AND ur.target_type = 'series' \
                    AND ur.target_id = series.id::text)",
                [user_id],
            ),
            Expr::val(-1_f64),
        ])
        .into(),
        SortField::PageCount | SortField::Number => {
            return Err(CompileError::SortNotAvailable {
                sort: field,
                entity: ViewEntity::Series,
            });
        }
    };
    Ok((expr, order_sea))
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
        SortField::LastRead | SortField::FinishedAt => {
            Expr::val(raw.to_owned()).cast_as(Alias::new("timestamptz"))
        }
        SortField::Rating | SortField::ReadProgress => raw
            .parse::<f64>()
            .map(Expr::val)
            .unwrap_or_else(|_| Expr::val(raw.to_owned())),
        // Issue-only sorts; `compile` rejects them before reaching here.
        SortField::PageCount | SortField::Number => Expr::val(raw.to_owned()),
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
        // Bound as text, cast to timestamptz: a bare text parameter
        // against a timestamptz / date column raises `operator does not
        // exist` in Postgres (the same reason `cursor_value_expr` casts).
        FieldKind::Date => Ok(Expr::val(as_text(v, bad)?).cast_as(Alias::new("timestamptz"))),
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

    /// B9: `collection_completeness` joins the caller's `users` row and
    /// per-series override once each (no per-row subquery), and the
    /// specials only enter the count through the resolved flag.
    #[test]
    fn collection_completeness_joins_the_callers_completion_prefs() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::CollectionCompleteness,
            op: Op::Is,
            value: json!("complete"),
        }]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#"LEFT JOIN "users" AS "cpu""#)
                && sql.contains(r#"LEFT JOIN "user_series_completion_prefs" AS "cpo""#),
            "SQL: {sql}"
        );
        assert!(
            sql.contains("cpo.user_id = '") && sql.contains("cpo.series_id = series.id"),
            "override joined on the caller's PK prefix: {sql}"
        );
        assert!(
            sql.contains("COALESCE(cpo.count_specials,")
                && sql.contains("THEN COALESCE(aic.special_count, 0) ELSE 0 END"),
            "SQL: {sql}"
        );
        assert!(!sql.contains("(SELECT count_specials"), "SQL: {sql}");
    }

    #[test]
    fn completion_pref_joins_absent_when_unreferenced() {
        let d = dsl_all(vec![Condition {
            group_id: 0,
            field: Field::UnreadIssues,
            op: Op::Gt,
            value: json!(5),
        }]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(!sql.contains("user_series_completion_prefs"), "SQL: {sql}");
        assert!(!sql.contains(r#""users""#), "SQL: {sql}");
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
    fn issue_only_sort_rejected_on_series_views() {
        let d = dsl_all(vec![]);
        let mut input = make(d);
        input.sort_field = SortField::PageCount;
        assert!(matches!(
            compile(&input).unwrap_err(),
            CompileError::SortNotAvailable { .. }
        ));
    }

    // filter-parity-and-builtin-views-1.0 M1: the per-user sorts now
    // work on issue views through the caller's joined progress / rating
    // row, projected as `sort_*` columns for the keyset cursor.
    #[test]
    fn per_user_issue_sorts_join_the_callers_rows() {
        let sql = issues_sql_sorted(vec![], SortField::LastRead).unwrap();
        assert!(
            sql.contains("LEFT JOIN \"progress_records\" AS \"pr\""),
            "SQL: {sql}"
        );
        assert!(sql.contains("\"sort_last_read\""), "SQL: {sql}");
        assert!(sql.contains("ORDER BY pr.updated_at"), "SQL: {sql}");
        let sql = issues_sql_sorted(vec![], SortField::Rating).unwrap();
        assert!(
            sql.contains("LEFT JOIN \"user_ratings\" AS \"ur\""),
            "SQL: {sql}"
        );
        assert!(sql.contains("ORDER BY ur.rating"), "SQL: {sql}");
        // Unjoined sorts still project the columns (NULL) so the row
        // shape is constant.
        let sql = issues_sql_sorted(vec![], SortField::Name).unwrap();
        assert!(!sql.contains("progress_records"), "SQL: {sql}");
        assert!(sql.contains("\"sort_rating\""), "SQL: {sql}");
    }

    #[test]
    fn issue_reading_fields_read_the_callers_progress_row() {
        let sql = issues_sql(vec![cond(Field::ReadProgress, Op::Gte, json!(50))]).unwrap();
        assert!(
            sql.contains("SELECT pr.percent * 100 FROM progress_records pr"),
            "SQL: {sql}"
        );
        assert!(sql.contains("COALESCE("), "SQL: {sql}");
        let sql = issues_sql(vec![cond(Field::ReadCount, Op::Gte, json!(2))]).unwrap();
        assert!(sql.contains("pr.run + CASE WHEN pr.finished"), "SQL: {sql}");
        let sql = issues_sql(vec![cond(Field::FinishedAt, Op::Relative, json!(7))]).unwrap();
        assert!(
            sql.contains("SELECT pr.finished_at FROM progress_records pr"),
            "SQL: {sql}"
        );
        assert!(sql.contains("NOW() -"), "SQL: {sql}");
        let sql = issues_sql(vec![cond(
            Field::MetadataCompleteness,
            Op::Is,
            json!("needs_metadata"),
        )])
        .unwrap();
        assert!(
            sql.contains("THEN 'complete' ELSE 'needs_metadata' END"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn series_finished_at_aggregates_over_the_series() {
        let d = dsl_all(vec![cond(Field::FinishedAt, Op::NotRelative, json!(30))]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(sql.contains("SELECT MAX(pr.finished_at)"), "SQL: {sql}");
        assert!(sql.contains("< (NOW() -"), "SQL: {sql}");
    }

    #[test]
    fn lookup_junctions_join_the_name_table() {
        let d = dsl_all(vec![cond(
            Field::StoryArcs,
            Op::IncludesAny,
            json!(["Knightfall"]),
        )]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("FROM series_arcs j JOIN story_arc l ON l.id = j.arc_id"),
            "SQL: {sql}"
        );
        assert!(sql.contains("l.name = ANY("), "SQL: {sql}");
        let sql = issues_sql(vec![cond(
            Field::Universes,
            Op::Excludes,
            json!(["Earth-616"]),
        )])
        .unwrap();
        assert!(
            sql.contains("NOT EXISTS (SELECT 1 FROM issue_universes j"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn provider_match_and_release_date_compile_on_both_entities() {
        let d = dsl_all(vec![
            cond(Field::ProviderMatch, Op::Is, json!("unmatched")),
            cond(Field::ReleaseDate, Op::After, json!("2020-01-01T00:00:00Z")),
        ]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(sql.contains("e.entity_type = 'series'"), "SQL: {sql}");
        assert!(sql.contains("make_date(series.year, 1, 1)"), "SQL: {sql}");
        let sql = issues_sql(vec![
            cond(Field::ProviderMatch, Op::Is, json!("matched")),
            cond(Field::ReleaseDate, Op::Relative, json!(30)),
        ])
        .unwrap();
        assert!(sql.contains("e.entity_type = 'issue'"), "SQL: {sql}");
        assert!(sql.contains("make_date(issues.year, 1, 1)"), "SQL: {sql}");
    }

    #[test]
    fn date_values_bind_as_timestamptz() {
        let d = dsl_all(vec![cond(
            Field::CreatedAt,
            Op::Before,
            json!("2024-01-01T00:00:00Z"),
        )]);
        let sql = compile(&make(d)).unwrap().to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("CAST('2024-01-01T00:00:00Z' AS timestamptz)"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn series_sort_value_is_projected_for_every_sort() {
        for sort in [
            SortField::Name,
            SortField::LastRead,
            SortField::Rating,
            SortField::FinishedAt,
        ] {
            let d = dsl_all(vec![]);
            let mut input = make(d);
            input.sort_field = sort;
            let sql = compile(&input).unwrap().to_string(PostgresQueryBuilder);
            assert!(
                sql.contains("AS text) AS \"sort_value\""),
                "{sort:?}: {sql}"
            );
        }
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

    /// UX-8: `ends_with` parses off the wire, compiles to a suffix
    /// `LIKE` (same case-sensitive form as `contains` / `starts_with`), and
    /// is offered on exactly the fields that offer `starts_with`.
    #[test]
    fn ends_with_parses_and_compiles_to_suffix_like() {
        let parsed: Condition = serde_json::from_value(
            json!({ "field": "name", "op": "ends_with", "value": "Annual" }),
        )
        .expect("ends_with parses");
        assert_eq!(parsed.op, Op::EndsWith);
        assert_eq!(
            serde_json::to_value(Op::EndsWith).unwrap(),
            json!("ends_with")
        );
        assert!(
            serde_json::from_value::<Condition>(
                json!({ "field": "name", "op": "ends_with_ci", "value": "x" })
            )
            .is_err(),
            "unknown ops stay a deserialize error"
        );

        let sql = compile(&make(dsl_all(vec![parsed])))
            .unwrap()
            .to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#""series"."name" LIKE '%Annual'"#),
            "SQL: {sql}"
        );
        let sql = issues_sql(vec![cond(Field::Title, Op::EndsWith, json!("Part One"))]).unwrap();
        assert!(
            sql.contains(r#""issues"."title" LIKE '%Part One'"#),
            "SQL: {sql}"
        );

        for spec in registry::all_specs() {
            assert_eq!(
                spec.allowed_ops.contains(&Op::EndsWith),
                spec.allowed_ops.contains(&Op::StartsWith),
                "{:?}: ends_with must ride with starts_with",
                spec.field
            );
        }
        let err = compile(&make(dsl_all(vec![cond(
            Field::Year,
            Op::EndsWith,
            json!("9"),
        )])))
        .unwrap_err();
        assert!(matches!(
            err,
            CompileError::OpNotAllowedForField(Field::Year, Op::EndsWith)
        ));
        let err = compile(&make(dsl_all(vec![cond(
            Field::Name,
            Op::EndsWith,
            json!(5),
        )])))
        .unwrap_err();
        assert!(matches!(err, CompileError::BadValue { .. }));
    }

    /// E9: every text op escapes `\` / `%` / `_` in the value, names the
    /// escape character, and binds the pattern — the user's string never
    /// lands in the SQL text. Plain values keep their old pattern.
    #[test]
    fn text_ops_escape_like_metacharacters_and_bind_the_pattern() {
        let cases = [
            (Op::Contains, "X_Men", r"%X\_Men%"),
            (Op::NotContains, "100%", r"%100\%%"),
            (Op::StartsWith, "100%", r"100\%%"),
            (Op::EndsWith, r"\", r"%\\"),
            (Op::Contains, "Origin", "%Origin%"),
        ];
        for (op, value, pattern) in cases {
            let (sql, values) = compile(&make(dsl_all(vec![cond(Field::Name, op, json!(value))])))
                .unwrap()
                .build(PostgresQueryBuilder);
            let like = if op == Op::NotContains {
                "NOT LIKE"
            } else {
                "LIKE"
            };
            assert!(
                sql.contains(&format!(r#""series"."name" {like} $"#))
                    && sql.contains(r"ESCAPE E'\\'"),
                "{op:?}: SQL: {sql}"
            );
            // (A lone `\` also appears in the ESCAPE clause itself.)
            if value.len() > 1 {
                assert!(!sql.contains(value), "{op:?}: value inlined: {sql}");
            }
            assert!(
                values
                    .0
                    .iter()
                    .any(|v| *v == sea_orm::Value::from(pattern.to_owned())),
                "{op:?}: pattern {pattern} not bound: {values:?}"
            );
        }
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
    fn year_keyset_is_lexicographic_over_year_month_day_id() {
        // Post-audit T2.3: "Release date" orders by (year, month, day).
        let sql = keyset_sql(
            SortField::Year,
            SortOrder::Asc,
            IssueCursor {
                keys: vec![json!(2020), json!(3), json!(null)],
                id: "abc".into(),
            },
        );
        assert!(
            sql.contains("(issues.year > 2020 OR issues.year IS NULL)"),
            "SQL: {sql}"
        );
        assert!(
            sql.contains("issues.year = 2020 AND (issues.month > 3 OR issues.month IS NULL)"),
            "SQL: {sql}"
        );
        // A NULL day boundary only advances by id within the NULL-day tail.
        assert!(
            sql.contains("issues.year = 2020 AND issues.month = 3 AND issues.day IS NULL AND issues.id > 'abc'"),
            "SQL: {sql}"
        );
        assert!(!sql.contains("issues.day >"), "SQL: {sql}");
        assert!(
            sql.contains("\"issues\".\"month\""),
            "month must be in ORDER BY / SELECT: {sql}"
        );
    }

    #[test]
    fn release_date_keyset_helper_matches_the_view_compiler() {
        let expr = release_date_keyset(Some(2020), None, None, "abc", false);
        let sql = Query::select().expr(expr).to_string(PostgresQueryBuilder);
        assert!(
            sql.contains("(issues.year < 2020 OR issues.year IS NULL)"),
            "SQL: {sql}"
        );
        assert!(
            sql.contains("issues.year = 2020 AND issues.month IS NULL AND issues.day IS NULL AND issues.id < 'abc'"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn null_cursor_key_only_advances_within_the_null_tail() {
        // Year DESC NULLS LAST: after a NULL year only later NULL-year ids
        // remain — never a non-NULL year again.
        let sql = keyset_sql(
            SortField::Year,
            SortOrder::Desc,
            IssueCursor {
                keys: vec![json!(null), json!(null), json!(null)],
                id: "abc".into(),
            },
        );
        assert!(
            sql.contains(
                "(issues.year IS NULL AND issues.month IS NULL AND issues.day IS NULL AND issues.id < 'abc')"
            ),
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
            keys: vec![json!(12), json!(1.0), json!(null)],
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
        let c = IssueCursor::for_row(
            SortField::Name,
            &IssueCursorRow {
                id: "id1",
                series_name: "Batman",
                sort_number: Some(1.5),
                year: None,
                month: None,
                day: None,
                page_count: None,
                created_at: &ts,
                updated_at: &ts,
                last_read: None,
                read_progress: None,
                finished_at: None,
                rating: None,
            },
        );
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
