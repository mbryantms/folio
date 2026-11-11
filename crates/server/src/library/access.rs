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
//! A grant may also set `hide_unrated`, which hides a series with no
//! rating on the ladder as if it ranked above the cap. Three shapes are
//! offered so callers don't re-derive the predicate:
//!
//! - in-memory: [`VisibleLibraries::series_ok`] / [`VisibleLibraries::issue_ok`]
//! - sea-orm / sea-query: [`VisibleLibraries::series_filter`] /
//!   [`VisibleLibraries::issue_filter`] / [`VisibleLibraries::issue_cap_condition`]
//! - raw SQL: [`VisibleLibraries::raw_cap_clause`] (series rows),
//!   [`VisibleLibraries::issue_cap_sql_joined`] / [`VisibleLibraries::issue_cap_sql`]
//!   (issue rows)
//!
//! **An issue is visible only when its series is**: its own rating must
//! pass the cap AND its series must pass the series-level rule. An issue
//! with no rating of its own follows its series. The issue-level check
//! therefore needs the parent rating; [`filter_issues`] and
//! [`issue_visible`] fetch it, and only when the caller's grant hides
//! something, so uncapped users pay no extra round-trip and send the same
//! SQL as before (every predicate builder returns nothing for them).

use crate::auth::extractor::CurrentUser;
use crate::library::age_rating;
use crate::state::AppState;
use entity::user::UserRole;
use entity::{issue, library_user_access, series};
use sea_orm::sea_query::{Condition, Expr, ExprTrait, Func, SimpleExpr};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, QueryTrait, Value};
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
    /// Libraries whose grant hides unrated content
    /// (`library_user_access.hide_unrated`): a series with no rating on
    /// the ladder is hidden there as if it ranked above the cap, and its
    /// issues with it. Independent of `caps` (a grant may hide unrated
    /// content without a cap). Always empty for unrestricted callers.
    pub hide_unrated: HashSet<Uuid>,
}

/// What one library's grant hides: the ladder rungs above its cap
/// (lowercased) and whether unrated rows go too. Only libraries where
/// at least one of the two hides something get a rule.
struct LibraryRule {
    library_id: Uuid,
    hidden: Vec<String>,
    hide_unrated: bool,
    cap: Option<String>,
}

impl VisibleLibraries {
    pub fn unrestricted() -> Self {
        Self {
            unrestricted: true,
            allowed: HashSet::new(),
            caps: HashMap::new(),
            hide_unrated: HashSet::new(),
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

    /// True iff the grant on this library hides unrated content.
    pub fn hides_unrated_in(&self, library_id: Uuid) -> bool {
        self.hide_unrated.contains(&library_id)
    }

    /// True iff at least one visible library carries a cap or hides
    /// unrated content — the cheap gate for "this caller may not see
    /// everything in the libraries it is granted".
    pub fn has_caps(&self) -> bool {
        !self.caps.is_empty() || !self.hide_unrated.is_empty()
    }

    /// True iff some grant actually hides rows: a cap that hides a rung
    /// (a cap on the top rung hides nothing) or `hide_unrated`. The
    /// switch between the two shapes of an entity query: the `series_*`
    /// rollup junctions are the union of every issue in the series, so a
    /// caller this returns `true` for reads the `issue_*` twin over the
    /// issues it can see instead; every other caller keeps the rollup
    /// (and its query plan).
    pub fn hides_any(&self) -> bool {
        !self.hide_unrated.is_empty()
            || self
                .caps
                .values()
                .any(|cap| !age_rating::hidden_ratings(cap).is_empty())
    }

    /// [`Self::hides_any`] for one library.
    pub fn hides_in(&self, library_id: Uuid) -> bool {
        self.hides_unrated_in(library_id)
            || self
                .cap_for(library_id)
                .is_some_and(|cap| !age_rating::hidden_ratings(cap).is_empty())
    }

    /// Membership + cap check for a series row (and `hide_unrated`).
    pub fn series_ok(&self, library_id: Uuid, rating: Option<&str>) -> bool {
        self.contains(library_id)
            && age_rating::passes_grant(
                rating,
                self.cap_for(library_id),
                self.hides_unrated_in(library_id),
            )
    }

    /// Membership + cap check for an issue row. An issue is visible only
    /// when its series is ([`Self::series_ok`] on `series_rating`) AND its
    /// own rating passes the cap. An issue with no rating of its own (or
    /// one not on the ladder) follows its series, so `hide_unrated` is a
    /// question about the series alone.
    pub fn issue_ok(
        &self,
        library_id: Uuid,
        issue_rating: Option<&str>,
        series_rating: Option<&str>,
    ) -> bool {
        self.series_ok(library_id, series_rating)
            && age_rating::passes(issue_rating, self.cap_for(library_id))
    }

    /// One rule per library whose grant hides something, sorted by
    /// library id so the SQL text (HashMap order is random) keeps query
    /// plans / logs stable. Empty when nothing is hidden, which is what
    /// keeps uncapped callers' SQL unchanged.
    fn rules(&self) -> Vec<LibraryRule> {
        let mut libs: Vec<Uuid> = self
            .caps
            .keys()
            .chain(self.hide_unrated.iter())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        libs.sort();
        libs.into_iter()
            .filter_map(|library_id| {
                let cap = self.cap_for(library_id).map(str::to_owned);
                let hidden = cap
                    .as_deref()
                    .map(age_rating::hidden_ratings)
                    .unwrap_or_default();
                let hide_unrated = self.hides_unrated_in(library_id);
                (!hidden.is_empty() || hide_unrated).then_some(LibraryRule {
                    library_id,
                    hidden,
                    hide_unrated,
                    cap,
                })
            })
            .collect()
    }

    /// Series-level pass for one rule, as a sea-query condition on
    /// `rating` (no library test). Without `hide_unrated`: unrated passes,
    /// a rung above the cap does not. With it: only a rung at or below
    /// the cap passes.
    fn series_pass_condition(rule: &LibraryRule, rating: SimpleExpr) -> Condition {
        if rule.hide_unrated {
            Condition::all().add(
                normalized_rating(rating).is_in(age_rating::shown_ratings(rule.cap.as_deref())),
            )
        } else {
            Condition::any()
                .add(rating.clone().is_null())
                .add(normalized_rating(rating).is_not_in(rule.hidden.clone()))
        }
    }

    /// Series-level cap predicate as a sea-query condition, for any
    /// statement that exposes a library column and a **series** rating
    /// expression:
    ///
    /// ```sql
    /// (library <> $lib OR rating IS NULL OR lower(btrim(rating)) NOT IN (...))
    /// AND ... -- one conjunct per library whose grant hides something
    /// ```
    ///
    /// (`lower(btrim(rating)) IN (<shown rungs>)` for a grant with
    /// `hide_unrated`.) `None` when nothing is hidden (no predicate
    /// needed). Membership (`library_id IN allowed`) is *not* included —
    /// pair it with the existing ACL filter, or use [`Self::series_filter`]
    /// / [`Self::issue_filter`] which bundle both. For an issue row use
    /// [`Self::issue_cap_condition`], never this on an effective rating.
    pub fn cap_condition(&self, library_col: SimpleExpr, rating: SimpleExpr) -> Option<Condition> {
        let rules = self.rules();
        if rules.is_empty() {
            return None;
        }
        let mut all = Condition::all();
        for rule in rules {
            all = all.add(
                Condition::any()
                    .add(library_col.clone().ne(rule.library_id))
                    .add(Self::series_pass_condition(&rule, rating.clone())),
            );
        }
        Some(all)
    }

    /// Issue-level cap predicate as a sea-query condition: per library
    /// whose grant hides something, the issue's own rating passes the cap
    /// (NULL / unranked passes) AND its series' rating passes the
    /// series-level rule. `None` when nothing is hidden.
    fn issue_condition(
        &self,
        library_col: SimpleExpr,
        own: SimpleExpr,
        series_rating: SimpleExpr,
    ) -> Option<Condition> {
        let rules = self.rules();
        if rules.is_empty() {
            return None;
        }
        let mut all = Condition::all();
        for rule in rules {
            let mut pass = Condition::all();
            if !rule.hidden.is_empty() {
                pass = pass.add(
                    Condition::any()
                        .add(own.clone().is_null())
                        .add(normalized_rating(own.clone()).is_not_in(rule.hidden.clone())),
                );
            }
            pass = pass.add(Self::series_pass_condition(&rule, series_rating.clone()));
            all = all.add(
                Condition::any()
                    .add(library_col.clone().ne(rule.library_id))
                    .add(pass),
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
        if let Some(cap) = self.series_cap_condition() {
            cond = cond.add(cap);
        }
        Some(cond)
    }

    /// Complete visibility predicate for an `issues` query: membership +
    /// [`Self::issue_cap_condition`]. The series rating is reached through
    /// a correlated sub-select so the caller doesn't have to join
    /// `series`. `None` for unrestricted callers.
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
    /// the caller): the issue's own rating under the cap AND its series
    /// visible under the series-level rule. `None` when nothing is
    /// hidden.
    pub fn issue_cap_condition(&self) -> Option<Condition> {
        self.issue_condition(
            Expr::col((issue::Entity, issue::Column::LibraryId)),
            Expr::col((issue::Entity, issue::Column::AgeRating)),
            issue_series_rating_expr(),
        )
    }

    /// Raw-SQL twin of [`Self::issue_cap_condition`] for an `issues` row
    /// aliased `alias`, with the series rating read through a correlated
    /// sub-select. Same ` AND ` / `params` convention as
    /// [`Self::raw_cap_clause`]; empty when nothing is hidden. For
    /// statements scoped to one already-visible series (membership is
    /// settled), and the cap half of [`visible_issues_sql`].
    pub fn issue_cap_sql(&self, alias: &str, params: &mut Vec<Value>) -> String {
        self.raw_issue_clause(
            &format!("{alias}.library_id"),
            &format!("{alias}.age_rating"),
            &format!("(SELECT s.age_rating FROM series s WHERE s.id = {alias}.series_id)"),
            params,
        )
    }

    /// [`Self::issue_cap_sql`] for an `issues` row aliased `issue` that
    /// the statement already joins to its series aliased `series`: the
    /// issue's own rating under the cap AND the series visible. The one
    /// issue predicate for every hand-written statement that has both
    /// rows (issue lists, rails, the reading log, entity surfaces). Same
    /// ` AND ` / `params` convention as [`Self::raw_cap_clause`]; empty
    /// when nothing is hidden, so uncapped SQL is unchanged.
    pub fn issue_cap_sql_joined(
        &self,
        issue: &str,
        series: &str,
        params: &mut Vec<Value>,
    ) -> String {
        self.raw_issue_clause(
            &format!("{series}.library_id"),
            &format!("{issue}.age_rating"),
            &format!("{series}.age_rating"),
            params,
        )
    }

    /// Cap-only predicate for a `series` query (membership handled by
    /// the caller). `None` when nothing is hidden.
    pub fn series_cap_condition(&self) -> Option<Condition> {
        self.cap_condition(
            Expr::col((series::Entity, series::Column::LibraryId)),
            Expr::col((series::Entity, series::Column::AgeRating)),
        )
    }

    /// Series-level cap predicate for hand-written SQL. `library_expr` /
    /// `rating_expr` are SQL fragments for a **series** row (e.g.
    /// `s.library_id`, `s.age_rating`); bound values are pushed onto
    /// `params` and referenced as `$n` from the returned fragment, which
    /// starts with ` AND ` and is empty when nothing is hidden. An issue
    /// row goes through [`Self::issue_cap_sql_joined`] /
    /// [`Self::issue_cap_sql`] instead, never this on
    /// `COALESCE(i.age_rating, s.age_rating)`.
    pub fn raw_cap_clause(
        &self,
        library_expr: &str,
        rating_expr: &str,
        params: &mut Vec<Value>,
    ) -> String {
        let mut sql = String::new();
        for rule in self.rules() {
            params.push(Value::from(rule.library_id));
            let lib_param = params.len();
            let pass = raw_series_pass(&rule, rating_expr, params);
            sql.push_str(&format!(" AND ({library_expr} <> ${lib_param} OR {pass})"));
        }
        sql
    }

    /// Record one `library_user_access` row: membership, its cap (when
    /// on the ladder) and `hide_unrated`.
    fn add_grant(&mut self, g: library_user_access::Model) {
        self.allowed.insert(g.library_id);
        if let Some(cap) = g.age_rating_max.filter(|c| age_rating::is_valid_cap(c)) {
            self.caps.insert(g.library_id, cap);
        }
        if g.hide_unrated {
            self.hide_unrated.insert(g.library_id);
        }
    }

    /// Issue-level raw clause: per rule, the own rating under the cap AND
    /// the series rating passing the series-level rule.
    fn raw_issue_clause(
        &self,
        library_expr: &str,
        own_expr: &str,
        series_expr: &str,
        params: &mut Vec<Value>,
    ) -> String {
        let mut sql = String::new();
        for rule in self.rules() {
            params.push(Value::from(rule.library_id));
            let lib_param = params.len();
            let own = if rule.hidden.is_empty() {
                String::new()
            } else {
                params.push(Value::from(rule.hidden.clone()));
                let rungs_param = params.len();
                format!(
                    "({own_expr} IS NULL OR lower(btrim({own_expr})) <> ALL(${rungs_param})) AND "
                )
            };
            let series_pass = raw_series_pass(&rule, series_expr, params);
            sql.push_str(&format!(
                " AND ({library_expr} <> ${lib_param} OR ({own}({series_pass})))"
            ));
        }
        sql
    }
}

/// The series-level pass of one rule over `rating_expr`, binding its rung
/// list onto `params`.
fn raw_series_pass(rule: &LibraryRule, rating_expr: &str, params: &mut Vec<Value>) -> String {
    if rule.hide_unrated {
        params.push(Value::from(age_rating::shown_ratings(rule.cap.as_deref())));
        format!("lower(btrim({rating_expr})) = ANY(${})", params.len())
    } else {
        params.push(Value::from(rule.hidden.clone()));
        format!(
            "{rating_expr} IS NULL OR lower(btrim({rating_expr})) <> ALL(${})",
            params.len()
        )
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

/// `(SELECT series.age_rating ...)` — the rating of an issue row's
/// series, for statements selecting from `issues` without a `series`
/// join.
fn issue_series_rating_expr() -> SimpleExpr {
    Expr::cust(
        r#"(SELECT "series"."age_rating" FROM "series" WHERE "series"."id" = "issues"."series_id")"#,
    )
}

/// `series.removed_at IS NULL` — the row is still on disk as far as the
/// last scan knows. Every surface that *lists* series (library grid,
/// search, saved-view rails, OPDS catalogs) applies this on top of the ACL
/// predicate so a soft-deleted series never renders as an empty card; the
/// admin Removed tab (`api::reconcile`) is the one place removed rows are
/// meant to appear. Soft-deleted issues are filtered the same way at every
/// issue listing (`issue.removed_at IS NULL`), this is the series-level twin.
pub fn active_series() -> SimpleExpr {
    Expr::col((series::Entity, series::Column::RemovedAt)).is_null()
}

/// `series::Entity::find()` with [`active_series`] already applied — the
/// starting point for any series listing.
pub fn active_series_select() -> sea_orm::Select<series::Entity> {
    series::Entity::find().filter(active_series())
}

/// Series the caller can currently see: [`active_series`] plus the
/// caller's library grant and age cap ([`VisibleLibraries::series_filter`]).
/// The starting point for any series lookup a personal list hydrates
/// (collection entries, …).
pub fn visible_series_select(acl: &VisibleLibraries) -> sea_orm::Select<series::Entity> {
    let mut select = active_series_select();
    if let Some(cond) = acl.series_filter() {
        select = select.filter(cond);
    }
    select
}

/// Issues the caller can currently open: `issue.removed_at IS NULL`, a
/// live parent series (a removed series must not surface through its
/// issues either), and the caller's grant + cap
/// ([`VisibleLibraries::issue_filter`]: own rating under the cap, series
/// visible). One predicate for every personal-list surface that keys on
/// an issue (collection entries, reading sessions, markers, reading
/// stats); [`visible_issues_sql`] is its twin for hand-written SQL. Scan
/// `state` is left to the caller: list surfaces add `state = 'active'`,
/// history surfaces do not.
pub fn visible_issues_select(acl: &VisibleLibraries) -> sea_orm::Select<issue::Entity> {
    let live_parent = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .filter(active_series())
        .filter(
            Expr::col((series::Entity, series::Column::Id))
                .equals((issue::Entity, issue::Column::SeriesId)),
        )
        .into_query();
    let mut select = issue::Entity::find()
        .filter(issue::Column::RemovedAt.is_null())
        .filter(Expr::exists(live_parent));
    if let Some(cond) = acl.issue_filter() {
        select = select.filter(cond);
    }
    select
}

/// [`visible_issues_select`] for hand-written SQL over an `issues` row
/// aliased `alias` (e.g. `i` in `markers m JOIN issues i ON ...`): not
/// soft-removed, a live parent series, and the caller's grant + cap.
/// Bound values are pushed onto `params` and referenced as `$n`; the
/// fragment starts with ` AND `. Keep the two in step.
pub fn visible_issues_sql(acl: &VisibleLibraries, alias: &str, params: &mut Vec<Value>) -> String {
    let mut sql = format!(
        " AND {alias}.removed_at IS NULL AND EXISTS (SELECT 1 FROM series vis_s \
         WHERE vis_s.id = {alias}.series_id AND vis_s.removed_at IS NULL)"
    );
    if !acl.unrestricted {
        params.push(Value::from(
            acl.allowed.iter().copied().collect::<Vec<Uuid>>(),
        ));
        sql.push_str(&format!(" AND {alias}.library_id = ANY(${})", params.len()));
        sql.push_str(&acl.issue_cap_sql(alias, params));
    }
    sql
}

/// [`visible_series_select`] for hand-written SQL over a `series` row
/// aliased `alias`: not soft-removed, inside the caller's grant, under the
/// cap. Same `params` / ` AND ` convention as [`visible_issues_sql`].
pub fn visible_series_sql(acl: &VisibleLibraries, alias: &str, params: &mut Vec<Value>) -> String {
    let mut sql = format!(" AND {alias}.removed_at IS NULL");
    if !acl.unrestricted {
        params.push(Value::from(
            acl.allowed.iter().copied().collect::<Vec<Uuid>>(),
        ));
        sql.push_str(&format!(" AND {alias}.library_id = ANY(${})", params.len()));
        sql.push_str(&acl.raw_cap_clause(
            &format!("{alias}.library_id"),
            &format!("{alias}.age_rating"),
            params,
        ));
    }
    sql
}

pub async fn for_user(app: &AppState, user: &CurrentUser) -> VisibleLibraries {
    if user.role == UserRole::Admin {
        return VisibleLibraries::unrestricted();
    }
    let granted = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let mut out = VisibleLibraries::default();
    for g in granted {
        out.add_grant(g);
    }
    out
}

/// True iff `acl` hides nothing in the library: an admin, or a non-admin
/// with no age cap on any grant AND a grant on every library that exists.
/// For surfaces that hold stored global facts (CBL refresh-log counts,
/// dry-run rematch counts) which cannot be recomputed per caller: only a
/// caller this returns `true` for may see them unchanged. One query for a
/// non-admin without caps; a DB error reads as `false` (fail closed).
pub async fn sees_whole_library(db: &sea_orm::DatabaseConnection, acl: &VisibleLibraries) -> bool {
    if acl.unrestricted {
        return true;
    }
    if acl.has_caps() {
        return false;
    }
    let mut ungranted = entity::library::Entity::find()
        .select_only()
        .column(entity::library::Column::Id);
    if !acl.allowed.is_empty() {
        ungranted = ungranted.filter(
            entity::library::Column::Id.is_not_in(acl.allowed.iter().copied().collect::<Vec<_>>()),
        );
    }
    matches!(ungranted.into_tuple::<Uuid>().one(db).await, Ok(None))
}

/// Single-row grant lookup: membership + cap for one library. Admins
/// are unrestricted. Same query count as the old per-module
/// `visible(app, user, lib_id)` helpers this replaces.
pub async fn for_library(app: &AppState, user: &CurrentUser, library_id: Uuid) -> VisibleLibraries {
    if user.role == UserRole::Admin {
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
        out.add_grant(g);
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

/// Full visibility check for one issue row: membership, the issue's own
/// rating under the cap, and its series visible
/// ([`VisibleLibraries::issue_ok`]). The series rating is fetched only
/// when the caller's grant on that library hides something.
/// The one gate for every issue-scoped route: a `false` must answer
/// exactly like an unknown id. Takes the full model or a slim projection
/// (`IssueCardRow`).
pub async fn issue_visible<T: IssueAclFields>(app: &AppState, user: &CurrentUser, row: &T) -> bool {
    let acl = for_library(app, user, row.acl_library_id()).await;
    issue_allowed(app, &acl, row).await
}

/// [`issue_visible`] for a caller that already holds the ACL.
pub async fn issue_allowed<T: IssueAclFields>(
    app: &AppState,
    acl: &VisibleLibraries,
    row: &T,
) -> bool {
    let library_id = row.acl_library_id();
    if !acl.contains(library_id) {
        return false;
    }
    if !acl.hides_in(library_id) {
        return true;
    }
    // Own rating first: a hidden one settles it without the lookup.
    if !age_rating::passes(row.acl_age_rating(), acl.cap_for(library_id)) {
        return false;
    }
    let series_id = row.acl_series_id();
    let parent = series_ratings(app, &[series_id]).await;
    let series_rating = parent.get(&series_id).and_then(|r| r.as_deref());
    acl.issue_ok(library_id, row.acl_age_rating(), series_rating)
}

/// Full visibility check for one series row (membership + cap).
pub async fn series_visible(app: &AppState, user: &CurrentUser, row: &series::Model) -> bool {
    let acl = for_library(app, user, row.library_id).await;
    acl.series_ok(row.library_id, row.age_rating.as_deref())
}

/// Drop issues the caller can't see ([`VisibleLibraries::issue_ok`]),
/// preserving order. One extra query for the parent ratings, and only
/// when some grant hides something.
pub async fn filter_issues<T: IssueAclFields>(
    app: &AppState,
    acl: &VisibleLibraries,
    rows: Vec<T>,
) -> Vec<T> {
    if !acl.hides_any() {
        return rows
            .into_iter()
            .filter(|i| acl.contains(i.acl_library_id()))
            .collect();
    }
    let need_parent: Vec<Uuid> = rows
        .iter()
        .filter(|i| acl.hides_in(i.acl_library_id()))
        .map(|i| i.acl_series_id())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let parents = series_ratings(app, &need_parent).await;
    rows.into_iter()
        .filter(|i| {
            let series_rating = parents.get(&i.acl_series_id()).and_then(|r| r.as_deref());
            acl.issue_ok(i.acl_library_id(), i.acl_age_rating(), series_rating)
        })
        .collect()
}

/// The three issue columns [`filter_issues`] reads. Implemented by the
/// full `issue::Model` and by slim list projections
/// (`api::issue_card::IssueCardRow`) so list paths can ACL-filter without
/// hydrating the wide row.
pub trait IssueAclFields {
    fn acl_library_id(&self) -> Uuid;
    fn acl_series_id(&self) -> Uuid;
    fn acl_age_rating(&self) -> Option<&str>;
}

impl IssueAclFields for issue::Model {
    fn acl_library_id(&self) -> Uuid {
        self.library_id
    }
    fn acl_series_id(&self) -> Uuid {
        self.series_id
    }
    fn acl_age_rating(&self) -> Option<&str> {
        self.age_rating.as_deref()
    }
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
        // the issue's own rating and its series' rating must both pass
        assert!(!v.issue_ok(lib, Some("Mature 17+"), Some("Everyone")));
        assert!(!v.issue_ok(lib, Some("Everyone"), Some("Mature 17+")));
        assert!(v.issue_ok(lib, Some("Everyone"), Some("Teen")));
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
        let sql = v.raw_cap_clause("s.library_id", "s.age_rating", &mut params);
        assert_eq!(
            sql,
            " AND (s.library_id <> $2 OR s.age_rating IS NULL \
             OR lower(btrim(s.age_rating)) <> ALL($3))"
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn issue_cap_sql_checks_own_rating_and_series() {
        let lib = Uuid::now_v7();
        let mut params = Vec::new();
        assert_eq!(
            VisibleLibraries::unrestricted().issue_cap_sql("i", &mut params),
            ""
        );
        let sql = capped(lib, "Teen").issue_cap_sql("i", &mut params);
        assert_eq!(
            sql,
            " AND (i.library_id <> $1 OR ((i.age_rating IS NULL OR lower(btrim(i.age_rating)) <> ALL($2)) \
             AND ((SELECT s.age_rating FROM series s WHERE s.id = i.series_id) IS NULL \
             OR lower(btrim((SELECT s.age_rating FROM series s WHERE s.id = i.series_id))) <> ALL($3))))"
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn joined_issue_cap_checks_issue_and_series() {
        let lib = Uuid::now_v7();
        let mut params = Vec::new();
        assert_eq!(
            VisibleLibraries::unrestricted().issue_cap_sql_joined("i", "s", &mut params),
            ""
        );
        let sql = capped(lib, "Teen").issue_cap_sql_joined("i", "s", &mut params);
        assert_eq!(
            sql,
            " AND (s.library_id <> $1 OR ((i.age_rating IS NULL OR lower(btrim(i.age_rating)) <> ALL($2)) \
             AND (s.age_rating IS NULL OR lower(btrim(s.age_rating)) <> ALL($3))))"
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn issue_ok_requires_the_series_to_pass() {
        let lib = Uuid::now_v7();
        let v = capped(lib, "Teen");
        // An issue within the cap inside a series above it is hidden.
        assert!(!v.issue_ok(lib, Some("Teen"), Some("Mature 17+")));
        assert!(v.issue_ok(lib, Some("Teen"), None));
        assert!(v.issue_ok(lib, Some("Unknown"), Some("Teen")));
        assert!(!v.issue_ok(lib, Some("Unknown"), Some("Mature 17+")));
    }

    fn hiding_unrated(lib: Uuid, cap: Option<&str>) -> VisibleLibraries {
        let mut v = VisibleLibraries::default();
        v.allowed.insert(lib);
        if let Some(cap) = cap {
            v.caps.insert(lib, cap.to_owned());
        }
        v.hide_unrated.insert(lib);
        v
    }

    #[test]
    fn hide_unrated_hides_unrated_series_and_their_issues() {
        let lib = Uuid::now_v7();
        for v in [hiding_unrated(lib, None), hiding_unrated(lib, Some("Teen"))] {
            assert!(v.has_caps() && v.hides_any() && v.hides_in(lib));
            assert!(!v.series_ok(lib, None));
            assert!(!v.series_ok(lib, Some("Unknown")));
            assert!(v.series_ok(lib, Some("Teen")));
            // No own rating in a rated series: follows the series.
            assert!(v.issue_ok(lib, None, Some("Teen")));
            assert!(v.issue_ok(lib, Some("Unknown"), Some("Teen")));
            // Unrated series: its issues go with it, rated or not.
            assert!(!v.issue_ok(lib, Some("Teen"), None));
        }
        // Without a cap only unrated content is hidden.
        let v = hiding_unrated(lib, None);
        assert!(v.series_ok(lib, Some("X18+")));
        assert!(v.issue_ok(lib, Some("X18+"), Some("Teen")));
        let v = hiding_unrated(lib, Some("Teen"));
        assert!(!v.series_ok(lib, Some("Mature 17+")));
    }

    #[test]
    fn hide_unrated_sql_requires_a_shown_rung() {
        let lib = Uuid::now_v7();
        let mut params = Vec::new();
        let sql =
            hiding_unrated(lib, None).raw_cap_clause("s.library_id", "s.age_rating", &mut params);
        assert_eq!(
            sql,
            " AND (s.library_id <> $1 OR lower(btrim(s.age_rating)) = ANY($2))"
        );
        assert_eq!(params.len(), 2);
        let mut params = Vec::new();
        let sql = hiding_unrated(lib, None).issue_cap_sql_joined("i", "s", &mut params);
        assert_eq!(
            sql,
            " AND (s.library_id <> $1 OR ((lower(btrim(s.age_rating)) = ANY($2))))"
        );
        let mut params = Vec::new();
        let sql = hiding_unrated(lib, Some("Teen")).issue_cap_sql_joined("i", "s", &mut params);
        assert!(
            sql.contains("<> ALL($2)) AND (lower(btrim(s.age_rating)) = ANY($3))"),
            "{sql}"
        );
        assert_eq!(params.len(), 3);
        // A top-rung cap hides nothing on its own, but hide_unrated still does.
        assert!(hiding_unrated(lib, Some("X18+")).hides_any());
    }

    #[test]
    fn top_rung_cap_hides_nothing() {
        let lib = Uuid::now_v7();
        let v = capped(lib, "X18+");
        assert!(v.has_caps());
        assert!(!v.hides_any());
        assert!(capped(lib, "Teen").hides_any());
        assert!(capped(lib, "Teen").hides_in(lib));
        assert!(!capped(lib, "Teen").hides_in(Uuid::now_v7()));
        assert!(!v.hides_in(lib));
        assert!(!VisibleLibraries::unrestricted().hides_any());
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
        assert!(sql.contains(r#""issues"."age_rating" IS NULL"#), "{sql}");
        assert!(
            sql.contains(r#"LOWER(btrim((SELECT "series"."age_rating" FROM "series""#),
            "{sql}"
        );
        let mut q = Query::select();
        q.column(series::Column::Id).from(series::Entity);
        q.cond_where(hiding_unrated(lib, None).series_filter().unwrap());
        let sql = q.to_string(PostgresQueryBuilder);
        assert!(
            sql.contains(r#"LOWER(btrim("series"."age_rating")) IN ('early childhood'"#),
            "{sql}"
        );
        assert!(!sql.contains("IS NULL"), "{sql}");
    }
}
