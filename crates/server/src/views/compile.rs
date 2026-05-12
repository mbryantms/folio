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

use super::dsl::{Condition, Field, FilterDsl, MatchMode, Op, SortField, SortOrder};
use super::registry::{self, FieldKind, Source};
use crate::library::access::VisibleLibraries;
use crate::reading::series_progress;
use entity::series;
use sea_orm::{
    Condition as SeaCondition, EntityName, Iterable,
    sea_query::{
        Alias, BinOper, Expr, ExprTrait, Func, JoinType, Order, Query, SelectStatement, SimpleExpr,
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

    let mut combined = match input.dsl.match_mode {
        MatchMode::All => SeaCondition::all(),
        MatchMode::Any => SeaCondition::any(),
    };
    for cond in &input.dsl.conditions {
        combined = combined.add(compile_condition(cond)?);
    }
    if !input.dsl.conditions.is_empty() {
        q.cond_where(combined);
    }

    let (sort_expr, order_sea) = sort_expression(input.sort_field, input.sort_order);
    apply_cursor(&mut q, input, sort_expr.clone(), order_sea.clone());
    q.order_by_expr(sort_expr, order_sea.clone());
    q.order_by((series::Entity, series::Column::Id), order_sea);

    q.limit(input.limit + 1);

    Ok(q)
}

fn apply_visibility(q: &mut SelectStatement, vis: &VisibleLibraries) {
    if vis.unrestricted {
        return;
    }
    if vis.allowed.is_empty() {
        q.and_where(Expr::val(false).into());
        return;
    }
    let allowed: Vec<Uuid> = vis.allowed.iter().copied().collect();
    q.and_where(Expr::col((series::Entity, series::Column::LibraryId)).is_in(allowed));
}

fn needs_reading_join(dsl: &FilterDsl, sort: SortField) -> bool {
    if matches!(sort, SortField::LastRead | SortField::ReadProgress) {
        return true;
    }
    dsl.conditions.iter().any(|c| {
        let spec = registry::spec_for(c.field);
        matches!(spec.source, Source::Reading(_))
    })
}

fn compile_condition(cond: &Condition) -> Result<SeaCondition, CompileError> {
    let spec = registry::spec_for(cond.field);
    if !spec.allowed_ops.contains(&cond.op) {
        return Err(CompileError::OpNotAllowedForField(cond.field, cond.op));
    }
    match spec.source {
        Source::Series(col) => series_predicate(cond, spec.kind, col),
        Source::Reading(col) => reading_predicate(cond, spec.kind, col),
        Source::JunctionExists {
            table,
            value_col,
            role,
        } => junction_predicate(cond, table, value_col, role),
    }
}

fn series_predicate(
    cond: &Condition,
    kind: FieldKind,
    col: &'static str,
) -> Result<SeaCondition, CompileError> {
    let lhs: SimpleExpr = Expr::col((series::Entity, Alias::new(col))).into();
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
    let raw: SimpleExpr = Expr::col((usp, Alias::new(col))).into();
    let lhs: SimpleExpr = match kind {
        FieldKind::Number => Func::coalesce([raw, Expr::val(0_i64).into()]).into(),
        FieldKind::Date => raw,
        _ => {
            return Err(CompileError::Internal(format!(
                "reading source mapped to unsupported kind {kind:?}",
            )));
        }
    };
    Ok(SeaCondition::all().add(scalar_predicate(cond, kind, lhs)?))
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
        Op::Equals | Op::Is => Ok(lhs.eq(scalar_value(v, kind, &bad)?)),
        Op::NotEquals | Op::IsNot => Ok(lhs.ne(scalar_value(v, kind, &bad)?)),
        Op::Contains => Ok(lhs.like(format!("%{}%", as_text(v, &bad)?))),
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

fn junction_predicate(
    cond: &Condition,
    table: &'static str,
    value_col: &'static str,
    role: Option<&'static str>,
) -> Result<SeaCondition, CompileError> {
    let bad = |reason: &str| CompileError::BadValue {
        field: cond.field,
        op: cond.op,
        reason: reason.to_owned(),
    };
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

    // Whole SQL fragment is built from compile-time-static identifiers
    // (table, column, role come from the registry, not user input). User
    // values are bound through `cust_with_values`.
    let series_table = series::Entity.table_name();
    let role_clause = role
        .map(|r| format!(" AND {table}.role = '{r}'"))
        .unwrap_or_default();

    match cond.op {
        Op::IncludesAny => {
            let sql = format!(
                "EXISTS (SELECT 1 FROM {table} WHERE {table}.series_id = {series_table}.id{role_clause} AND {table}.{value_col} = ANY($1))",
            );
            Ok(SeaCondition::all().add(Expr::cust_with_values(&sql, [strs])))
        }
        Op::Excludes => {
            let sql = format!(
                "NOT EXISTS (SELECT 1 FROM {table} WHERE {table}.series_id = {series_table}.id{role_clause} AND {table}.{value_col} = ANY($1))",
            );
            Ok(SeaCondition::all().add(Expr::cust_with_values(&sql, [strs])))
        }
        Op::IncludesAll => {
            let mut all = SeaCondition::all();
            for s in strs {
                let sql = format!(
                    "EXISTS (SELECT 1 FROM {table} WHERE {table}.series_id = {series_table}.id{role_clause} AND {table}.{value_col} = $1)",
                );
                all = all.add(Expr::cust_with_values(&sql, [s]));
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
        SortField::Name => Expr::col((series::Entity, series::Column::Name)).into(),
        SortField::Year => Expr::col((series::Entity, series::Column::Year)).into(),
        SortField::CreatedAt => Expr::col((series::Entity, series::Column::CreatedAt)).into(),
        SortField::UpdatedAt => Expr::col((series::Entity, series::Column::UpdatedAt)).into(),
        SortField::LastRead => Expr::col((
            series_progress::subquery_alias(),
            Alias::new("last_read_at"),
        ))
        .into(),
        SortField::ReadProgress => Func::coalesce([
            Expr::col((series_progress::subquery_alias(), Alias::new("percent"))).into(),
            Expr::val(0_i64).into(),
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
    let id_col: SimpleExpr = Expr::col((series::Entity, series::Column::Id)).into();
    let id_op = match order {
        Order::Asc => BinOper::GreaterThan,
        _ => BinOper::SmallerThan,
    };
    if c.sort_value.is_empty() {
        q.and_where(SimpleExpr::Binary(
            Box::new(id_col),
            id_op,
            Box::new(Expr::val(c.id).into()),
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
                    Box::new(Expr::val(c.id).into()),
                )),
        );
    q.cond_where(composite);
}

fn cursor_value_expr(field: SortField, raw: &str) -> SimpleExpr {
    match field {
        SortField::Name => Expr::val(raw.to_owned()).into(),
        SortField::Year => raw
            .parse::<i32>()
            .map(|n| Expr::val(n).into())
            .unwrap_or_else(|_| Expr::val(raw.to_owned()).into()),
        SortField::CreatedAt | SortField::UpdatedAt => {
            Expr::val(raw.to_owned()).cast_as(Alias::new("timestamptz"))
        }
        // sort_value is empty for these — `apply_cursor`'s empty-string
        // branch handles them and never reaches this helper.
        SortField::LastRead | SortField::ReadProgress => Expr::val(raw.to_owned()).into(),
    }
}

// ───── value coercion helpers ─────

fn scalar_value(
    v: &Value,
    kind: FieldKind,
    bad: &dyn Fn(&str) -> CompileError,
) -> Result<SimpleExpr, CompileError> {
    match kind {
        FieldKind::Text | FieldKind::Enum => Ok(Expr::val(as_text(v, bad)?).into()),
        FieldKind::Number => Ok(Expr::val(as_number(v, bad)?).into()),
        FieldKind::Date => Ok(Expr::val(as_text(v, bad)?).into()),
        FieldKind::Uuid => Ok(Expr::val(as_uuid(v, bad)?).into()),
        FieldKind::Multi => Err(bad("multi field requires array op")),
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
}
