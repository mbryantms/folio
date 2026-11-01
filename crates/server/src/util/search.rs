//! Small helpers for case-insensitive, multi-term user-facing search.
//!
//! Every "search this list / these users / these notes" box should feel the
//! same: typing is case-insensitive, and multiple words narrow the result
//! set (each word must match *something*) rather than being treated as one
//! literal phrase. These two primitives keep that behaviour consistent
//! across the handlers without each one re-deriving the ILIKE plumbing.
//!
//! Usage — chain one `.filter()` per token so the tokens AND together, each
//! token OR-ing across the searchable columns:
//!
//! ```ignore
//! for token in needle.split_whitespace() {
//!     let pat = ilike_pattern(token);
//!     sel = sel.filter(
//!         Condition::any()
//!             .add(col_ilike(user::Column::Email, &pat))
//!             .add(col_ilike(user::Column::DisplayName, &pat)),
//!     );
//! }
//! ```

use sea_orm::ColumnTrait;
use sea_orm::sea_query::{Expr, LikeExpr, SimpleExpr, extension::postgres::PgExpr};

/// The escape character every Folio `LIKE` / `ILIKE` pattern uses. It is
/// Postgres' fixed default too; `LIKE` sites still name it (via
/// [`literal_like`] / [`SQL_LIKE_ESCAPE`]) so the query states which
/// character [`escape_like`] escaped for.
pub const LIKE_ESCAPE: char = '\\';

/// SQL suffix naming [`LIKE_ESCAPE`] for hand-written `… LIKE $n` clauses
/// (`lower(name) LIKE $3 ESCAPE E'\\'`) — the same text sea-query emits
/// for [`literal_like`]. The `E''` literal reads the same under any
/// `standard_conforming_strings`.
pub const SQL_LIKE_ESCAPE: &str = r" ESCAPE E'\\'";

/// Escape the LIKE metacharacters in `term` (`\`, `%`, `_`) so it matches
/// literally under the [`LIKE_ESCAPE`] backslash: `100%` stays one percent
/// sign, `X_Men` never matches `X-Men`, and a trailing `\` can't escape the
/// wildcard after it. Backslash goes first so the escapes added for `%` /
/// `_` aren't doubled.
pub fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A sea-query LIKE pattern with an explicit `ESCAPE '\'`, for the
/// `.like()` / `.not_like()` builders only (see [`col_ilike`] for why not
/// `.ilike()`). `pattern` is the
/// [`escape_like`]-escaped value plus whatever wildcards the caller adds; it
/// is bound as a parameter, never spliced into the SQL text.
pub fn literal_like(pattern: String) -> LikeExpr {
    LikeExpr::new(pattern).escape(LIKE_ESCAPE)
}

/// `%term%` with `term` escaped: a literal substring match.
pub fn contains_pattern(term: &str) -> String {
    format!("%{}%", escape_like(term))
}

/// `term%` with `term` escaped: a literal prefix match.
pub fn prefix_pattern(term: &str) -> String {
    format!("{}%", escape_like(term))
}

/// `%term` with `term` escaped: a literal suffix match.
pub fn suffix_pattern(term: &str) -> String {
    format!("%{}", escape_like(term))
}

/// Build a `%term%` ILIKE pattern with the LIKE metacharacters escaped, so a
/// query like `10_things` or `50%` is matched literally instead of being
/// read as a wildcard. Same as [`contains_pattern`]; the name pairs it with
/// [`col_ilike`].
pub fn ilike_pattern(term: &str) -> String {
    contains_pattern(term)
}

/// Table-qualified, case-insensitive `col ILIKE pattern`.
///
/// Mirrors how `ColumnTrait::like` qualifies the column (`(entity_name,
/// col)`) so the predicate stays unambiguous even when the query carries a
/// join (e.g. searching a joined issue's title). `pattern` comes from
/// [`ilike_pattern`] and relies on Postgres' default backslash escape: no
/// [`literal_like`] here, because sea-query 1.0 only knows `LIKE` /
/// `NOT LIKE` take an `ESCAPE` operand and renders `ILIKE ($1 ESCAPE …)`,
/// a syntax error.
pub fn col_ilike<C: ColumnTrait>(col: C, pattern: &str) -> SimpleExpr {
    Expr::col((col.entity_name(), col)).ilike(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_escapes_every_metacharacter_once() {
        assert_eq!(escape_like("plain"), "plain");
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like("X_Men"), r"X\_Men");
        assert_eq!(escape_like(r"a\b"), r"a\\b");
        // Backslash first: the escape added for `%` is not itself doubled.
        assert_eq!(escape_like(r"\%_"), r"\\\%\_");
        assert_eq!(ilike_pattern("50%"), r"%50\%%");
        assert_eq!(contains_pattern("a_b"), r"%a\_b%");
        assert_eq!(prefix_pattern("100%"), r"100\%%");
        assert_eq!(suffix_pattern(r"\"), r"%\\");
    }

    /// `col_ilike` must render a bare `ILIKE $n`: sea-query 1.0 wraps an
    /// `ESCAPE` operand of `ILIKE` in parentheses (invalid SQL), which is
    /// why it leans on Postgres' default backslash escape instead.
    #[test]
    fn col_ilike_renders_plain_ilike() {
        use sea_orm::sea_query::{PostgresQueryBuilder, Query};
        let (sql, _) = Query::select()
            .column(entity::user::Column::Email)
            .from(entity::user::Entity)
            .and_where(col_ilike(
                entity::user::Column::Email,
                &ilike_pattern("a_b"),
            ))
            .build(PostgresQueryBuilder);
        assert!(sql.ends_with(r#""users"."email" ILIKE $1"#), "SQL: {sql}");
    }
}
