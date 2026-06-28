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
use sea_orm::sea_query::{Expr, SimpleExpr, extension::postgres::PgExpr};

/// Build a `%term%` ILIKE pattern with the LIKE metacharacters escaped, so a
/// query like `10_things` or `50%` is matched literally instead of being
/// read as a wildcard. Backslash is escaped first to avoid double-escaping.
pub fn ilike_pattern(term: &str) -> String {
    format!(
        "%{}%",
        term.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// Table-qualified, case-insensitive `col ILIKE pattern`.
///
/// Mirrors how `ColumnTrait::like` qualifies the column (`(entity_name,
/// col)`) so the predicate stays unambiguous even when the query carries a
/// join (e.g. searching a joined issue's title).
pub fn col_ilike<C: ColumnTrait>(col: C, pattern: &str) -> SimpleExpr {
    Expr::col((col.entity_name(), col)).ilike(pattern)
}
