//! Shared library-visibility helpers.
//!
//! `VisibleLibraries` collapses an admin / per-user-ACL distinction into
//! a single value: `unrestricted = true` (admin) or an explicit set of
//! library IDs (non-admin). API handlers use it to scope list endpoints
//! and the saved-views compiler uses it as the first WHERE predicate.
//!
//! WP-2.7 adds the **age-rating cap**: each grant may carry
//! `age_rating_max`, and every surface that consults the ACL also hides
//! rows whose rating ranks above the cap (see
//! [`crate::library::age_rating`] for the ladder and the unrated rule).
//! Three shapes are offered so callers don't re-derive the predicate:
//!
//! - in-memory: [`VisibleLibraries::series_ok`] / [`VisibleLibraries::issue_ok`]
//! - sea-orm / sea-query: [`VisibleLibraries::series_filter`] /
//!   [`VisibleLibraries::issue_filter`] / [`VisibleLibraries::cap_condition`]
//! - raw SQL: [`VisibleLibraries::raw_cap_clause`]
//!
//! Issues inherit their series' rating when their own is NULL
//! (`COALESCE(issue.age_rating, series.age_rating)`), so an issue-level
//! check needs the parent rating; [`filter_issues`] and
//! [`issue_visible`] fetch it — and only when the caller is actually
//! capped, so uncapped users pay no extra round-trip.

use crate::auth::extractor::CurrentUser;
use crate::library::age_rating;
use crate::state::AppState;
use entity::{issue, library_user_access, series};
use sea_orm::sea_query::{Condition, Expr, ExprTrait, Func, SimpleExpr};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct VisibleLibraries {
    /// Admin / unrestricted access.
    pub unrestricted: bool,
    /// Library IDs the user has explicit access to (only used when not
    /// unrestricted).
    pub allowed: HashSet<Uuid>,
    /// Per-library age-rating cap (`library_user_access.age_rating_max`).
    /// Only libraries whose grant carries a cap appear here; a library
    /// in `allowed` but absent from `caps` is uncapped. Always empty for
    /// unrestricted (admin) callers.
    pub caps: HashMap<Uuid, String>,
}

impl VisibleLibraries {
    pub fn unrestricted() -> Self {
        Self {
            unrestricted: true,
            allowed: HashSet::new(),
            caps: HashMap::new(),
        }
    }

    /// True iff the user can see series in this library (membership
    /// only — the cap is a per-row question, see [`Self::series_ok`]).
    pub fn contains(&self, library_id: Uuid) -> bool {
        self.unrestricted || self.allowed.contains(&library_id)
    }

    /// The cap for a library, if the grant carries one.
    pub fn cap_for(&self, library_id: Uuid) -> Option<&str> {
        self.caps.get(&library_id).map(String::as_str)
    }

    /// True iff at least one visible library carries a cap — the cheap
    /// gate every helper uses to skip the rating work entirely.
    pub fn has_caps(&self) -> bool {
        !self.caps.is_empty()
    }

    /// Membership + cap check for a series row.
    pub fn series_ok(&self, library_id: Uuid, rating: Option<&str>) -> bool {
        self.contains(library_id) && age_rating::passes(rating, self.cap_for(library_id))
    }

    /// Membership + cap check for an issue row. The issue's own rating
    /// wins; the series rating is the fallback when it's NULL.
    pub fn issue_ok(
        &self,
        library_id: Uuid,
        issue_rating: Option<&str>,
        series_rating: Option<&str>,
    ) -> bool {
        self.contains(library_id)
            && age_rating::passes(issue_rating.or(series_rating), self.cap_for(library_id))
    }

    /// Capped libraries paired with the lowercased ladder rungs each one
    /// hides. Libraries whose cap hides nothing (top rung / unknown) are
    /// dropped so the predicate stays minimal.
    fn hidden_by_library(&self) -> Vec<(Uuid, Vec<String>)> {
        let mut out: Vec<(Uuid, Vec<String>)> = self
            .caps
            .iter()
            .filter_map(|(lib, cap)| {
                let hidden = age_rating::hidden_ratings(cap);
                (!hidden.is_empty()).then_some((*lib, hidden))
            })
            .collect();
        // Deterministic SQL text (HashMap order is random) keeps query
        // plans / logs stable.
        out.sort_by_key(|(lib, _)| *lib);
        out
    }

    /// Cap predicate as a sea-query condition, for any statement that
    /// exposes a library column and a rating expression:
    ///
    /// ```sql
    /// (library <> $lib OR rating IS NULL OR lower(btrim(rating)) NOT IN (...))
    /// AND ... -- one conjunct per capped library
    /// ```
    ///
    /// `None` when nothing is capped (no predicate needed). Membership
    /// (`library_id IN allowed`) is *not* included — pair it with the
    /// existing ACL filter, or use [`Self::series_filter`] /
    /// [`Self::issue_filter`] which bundle both.
    pub fn cap_condition(&self, library_col: SimpleExpr, rating: SimpleExpr) -> Option<Condition> {
        let hidden = self.hidden_by_library();
        if hidden.is_empty() {
            return None;
        }
        let mut all = Condition::all();
        for (lib, rungs) in hidden {
            all = all.add(
                Condition::any()
                    .add(library_col.clone().ne(lib))
                    .add(rating.clone().is_null())
                    .add(normalized_rating(rating.clone()).is_not_in(rungs)),
            );
        }
        Some(all)
    }

    /// Complete visibility predicate for a `series` query: membership +
    /// cap on `series.age_rating`. `None` for unrestricted callers (no
    /// filter needed). An empty allow-set yields a `1 = 2` predicate via
    /// sea-query's empty-`IN` rendering, so callers that already
    /// short-circuit on `allowed.is_empty()` keep working.
    pub fn series_filter(&self) -> Option<Condition> {
        if self.unrestricted {
            return None;
        }
        let mut cond = Condition::all()
            .add(series::Column::LibraryId.is_in(self.allowed.iter().copied().collect::<Vec<_>>()));
        if let Some(cap) = self.cap_condition(
            Expr::col((series::Entity, series::Column::LibraryId)),
            Expr::col((series::Entity, series::Column::AgeRating)),
        ) {
            cond = cond.add(cap);
        }
        Some(cond)
    }

    /// Complete visibility predicate for an `issues` query: membership +
    /// cap on `COALESCE(issues.age_rating, series.age_rating)`. The
    /// series rating is reached through a correlated sub-select so the
    /// caller doesn't have to join `series`. `None` for unrestricted
    /// callers.
    pub fn issue_filter(&self) -> Option<Condition> {
        if self.unrestricted {
            return None;
        }
        let mut cond = Condition::all()
            .add(issue::Column::LibraryId.is_in(self.allowed.iter().copied().collect::<Vec<_>>()));
        if let Some(cap) = self.issue_cap_condition() {
            cond = cond.add(cap);
        }
        Some(cond)
    }

    /// Cap-only predicate for an `issues` query (membership handled by
    /// the caller). `None` when nothing is capped.
    pub fn issue_cap_condition(&self) -> Option<Condition> {
        self.cap_condition(
            Expr::col((issue::Entity, issue::Column::LibraryId)),
            issue_effective_rating_expr(),
        )
    }

    /// Cap-only predicate for a `series` query (membership handled by
    /// the caller). `None` when nothing is capped.
    pub fn series_cap_condition(&self) -> Option<Condition> {
        self.cap_condition(
            Expr::col((series::Entity, series::Column::LibraryId)),
            Expr::col((series::Entity, series::Column::AgeRating)),
        )
    }

    /// Cap predicate for hand-written SQL. `library_expr` / `rating_expr`
    /// are SQL fragments (e.g. `i.library_id`,
    /// `COALESCE(i.age_rating, s.age_rating)`); bound values are pushed
    /// onto `params` and referenced as `$n` from the returned fragment,
    /// which starts with ` AND ` and is empty when nothing is capped.
    pub fn raw_cap_clause(
        &self,
        library_expr: &str,
        rating_expr: &str,
        params: &mut Vec<Value>,
    ) -> String {
        let hidden = self.hidden_by_library();
        if hidden.is_empty() {
            return String::new();
        }
        let mut sql = String::new();
        for (lib, rungs) in hidden {
            params.push(Value::from(lib));
            let lib_param = params.len();
            params.push(Value::from(rungs));
            let rungs_param = params.len();
            sql.push_str(&format!(
                " AND ({library_expr} <> ${lib_param} OR {rating_expr} IS NULL \
                 OR lower(btrim({rating_expr})) <> ALL(${rungs_param}))"
            ));
        }
        sql
    }
}

/// `lower(btrim(rating))` — the normalisation the ladder applies in
/// Rust, mirrored in SQL so `mature 17+` and `Mature 17+` rank alike.
fn normalized_rating(rating: SimpleExpr) -> SimpleExpr {
    Func::lower(Func::cust(BtrimFn).arg(rating)).into()
}

struct BtrimFn;

impl sea_orm::sea_query::Iden for BtrimFn {
    fn unquoted(&self) -> &str {
        "btrim"
    }
}

/// `COALESCE(issues.age_rating, (SELECT series.age_rating ...))` — the
/// effective rating of an issue row, for statements selecting from
/// `issues` without a `series` join.
pub fn issue_effective_rating_expr() -> SimpleExpr {
    Expr::cust(
        r#"COALESCE("issues"."age_rating", (SELECT "series"."age_rating" FROM "series" WHERE "series"."id" = "issues"."series_id"))"#,
    )
}

pub async fn for_user(app: &AppState, user: &CurrentUser) -> VisibleLibraries {
    if user.role == "admin" {
        return VisibleLibraries::unrestricted();
    }
    let granted = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let mut allowed = HashSet::with_capacity(granted.len());
    let mut caps = HashMap::new();
    for g in granted {
        allowed.insert(g.library_id);
        if let Some(cap) = g.age_rating_max.filter(|c| age_rating::is_valid_cap(c)) {
            caps.insert(g.library_id, cap);
        }
    }
    VisibleLibraries {
        unrestricted: false,
        allowed,
        caps,
    }
}

/// Single-row grant lookup: membership + cap for one library. Admins
/// are unrestricted. Same query count as the old per-module
/// `visible(app, user, lib_id)` helpers this replaces.
pub async fn for_library(app: &AppState, user: &CurrentUser, library_id: Uuid) -> VisibleLibraries {
    if user.role == "admin" {
        return VisibleLibraries::unrestricted();
    }
    for_library_by_id(app, user.id, library_id).await
}

/// [`for_library`] for callers that only hold a user id (no role
/// bypass — the caller handles admins itself).
pub async fn for_library_by_id(
    app: &AppState,
    user_id: Uuid,
    library_id: Uuid,
) -> VisibleLibraries {
    let grant = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user_id))
        .filter(library_user_access::Column::LibraryId.eq(library_id))
        .one(&app.db)
        .await
        .ok()
        .flatten();
    let mut out = VisibleLibraries::default();
    if let Some(g) = grant {
        out.allowed.insert(g.library_id);
        if let Some(cap) = g.age_rating_max.filter(|c| age_rating::is_valid_cap(c)) {
            out.caps.insert(g.library_id, cap);
        }
    }
    out
}

/// `series.age_rating` for a set of series ids (one query; empty input
/// → no query).
pub async fn series_ratings(app: &AppState, series_ids: &[Uuid]) -> HashMap<Uuid, Option<String>> {
    if series_ids.is_empty() {
        return HashMap::new();
    }
    series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::AgeRating)
        .filter(series::Column::Id.is_in(series_ids.to_vec()))
        .into_tuple::<(Uuid, Option<String>)>()
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Full visibility check for one issue row: membership + cap, with the
/// series-rating fallback fetched only when it's actually needed (the
/// caller is capped in that library and the issue carries no rating).
pub async fn issue_visible(app: &AppState, user: &CurrentUser, row: &issue::Model) -> bool {
    let acl = for_library(app, user, row.library_id).await;
    issue_allowed(app, &acl, row).await
}

/// [`issue_visible`] for a caller that already holds the ACL.
pub async fn issue_allowed(app: &AppState, acl: &VisibleLibraries, row: &issue::Model) -> bool {
    if !acl.contains(row.library_id) {
        return false;
    }
    let Some(cap) = acl.cap_for(row.library_id) else {
        return true;
    };
    if row.age_rating.is_some() {
        return age_rating::passes(row.age_rating.as_deref(), Some(cap));
    }
    let parent = series_ratings(app, &[row.series_id]).await;
    let series_rating = parent.get(&row.series_id).and_then(|r| r.as_deref());
    age_rating::passes(series_rating, Some(cap))
}

/// Full visibility check for one series row (membership + cap).
pub async fn series_visible(app: &AppState, user: &CurrentUser, row: &series::Model) -> bool {
    let acl = for_library(app, user, row.library_id).await;
    acl.series_ok(row.library_id, row.age_rating.as_deref())
}

/// Drop issues the caller can't see (membership + cap), preserving
/// order. One extra query for the parent ratings, and only when the
/// caller is capped somewhere and some candidate lacks its own rating.
pub async fn filter_issues(
    app: &AppState,
    acl: &VisibleLibraries,
    rows: Vec<issue::Model>,
) -> Vec<issue::Model> {
    if !acl.has_caps() {
        return rows
            .into_iter()
            .filter(|i| acl.contains(i.library_id))
            .collect();
    }
    let need_parent: Vec<Uuid> = rows
        .iter()
        .filter(|i| i.age_rating.is_none() && acl.cap_for(i.library_id).is_some())
        .map(|i| i.series_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let parents = series_ratings(app, &need_parent).await;
    rows.into_iter()
        .filter(|i| {
            let series_rating = parents.get(&i.series_id).and_then(|r| r.as_deref());
            acl.issue_ok(i.library_id, i.age_rating.as_deref(), series_rating)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped(lib: Uuid, cap: &str) -> VisibleLibraries {
        let mut v = VisibleLibraries::default();
        v.allowed.insert(lib);
        v.caps.insert(lib, cap.to_owned());
        v
    }

    #[test]
    fn in_memory_checks_combine_membership_and_cap() {
        let lib = Uuid::now_v7();
        let other = Uuid::now_v7();
        let v = capped(lib, "Teen");
        assert!(v.series_ok(lib, None));
        assert!(v.series_ok(lib, Some("Teen")));
        assert!(!v.series_ok(lib, Some("Mature 17+")));
        assert!(!v.series_ok(other, None));
        // issue rating wins over the series rating
        assert!(!v.issue_ok(lib, Some("Mature 17+"), Some("Everyone")));
        assert!(v.issue_ok(lib, Some("Everyone"), Some("Mature 17+")));
        // NULL issue rating falls back to the series rating
        assert!(!v.issue_ok(lib, None, Some("Mature 17+")));
        assert!(v.issue_ok(lib, None, None));
    }

    #[test]
    fn unrestricted_has_no_predicates() {
        let v = VisibleLibraries::unrestricted();
        assert!(v.series_filter().is_none());
        assert!(v.issue_filter().is_none());
        let mut params = Vec::new();
        assert_eq!(
            v.raw_cap_clause("i.library_id", "i.age_rating", &mut params),
            ""
        );
        assert!(params.is_empty());
    }

    #[test]
    fn uncapped_member_has_no_cap_condition() {
        let lib = Uuid::now_v7();
        let mut v = VisibleLibraries::default();
        v.allowed.insert(lib);
        assert!(v.series_cap_condition().is_none());
        assert!(v.issue_cap_condition().is_none());
        assert!(v.series_filter().is_some());
    }

    #[test]
    fn raw_clause_binds_library_and_hidden_rungs() {
        let lib = Uuid::now_v7();
        let v = capped(lib, "Teen");
        let mut params: Vec<Value> = vec![Value::from(1i64)];
        let sql = v.raw_cap_clause(
            "i.library_id",
            "COALESCE(i.age_rating, s.age_rating)",
            &mut params,
        );
        assert_eq!(
            sql,
            " AND (i.library_id <> $2 OR COALESCE(i.age_rating, s.age_rating) IS NULL \
             OR lower(btrim(COALESCE(i.age_rating, s.age_rating))) <> ALL($3))"
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn top_rung_cap_hides_nothing() {
        let lib = Uuid::now_v7();
        let v = capped(lib, "X18+");
        assert!(v.series_cap_condition().is_none());
        let mut params = Vec::new();
        assert_eq!(v.raw_cap_clause("l", "r", &mut params), "");
    }

    #[test]
    fn sea_query_condition_renders_expected_sql() {
        use sea_orm::sea_query::{PostgresQueryBuilder, Query};
        let lib = Uuid::now_v7();
        let v = capped(lib, "Teen");
        let mut q = Query::select();
        q.column(series::Column::Id).from(series::Entity);
        q.cond_where(v.series_filter().unwrap());
        let sql = q.to_string(PostgresQueryBuilder);
        assert!(sql.contains(r#""series"."library_id" IN"#), "{sql}");
        assert!(sql.contains(r#""series"."age_rating" IS NULL"#), "{sql}");
        assert!(sql.contains(r#"LOWER(btrim("series"."age_rating")) NOT IN ('ma15+', 'mature 17+', 'm', 'r18+', 'adults only 18+', 'x18+')"#), "{sql}");

        let mut q = Query::select();
        q.column(issue::Column::Id).from(issue::Entity);
        q.cond_where(v.issue_filter().unwrap());
        let sql = q.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#"COALESCE("issues"."age_rating", (SELECT "series"."age_rating""#),
            "{sql}"
        );
    }
}
