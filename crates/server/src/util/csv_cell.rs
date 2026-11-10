//! Spreadsheet-safe CSV cells, shared by every CSV export (holdings,
//! reading log).
//!
//! A spreadsheet evaluates a cell that starts with `=`, `+`, `-`, `@`,
//! tab or carriage return as a formula. Text that came from archive
//! metadata or user input could otherwise run as one when the export is
//! opened, so such a cell gets a leading `'`. Plain numbers (`-1`, `+3`,
//! `-0.5`) are left alone so issue `-1` and numeric columns keep their
//! value. Apply this before normal CSV quoting.

use std::borrow::Cow;

/// Neutralise spreadsheet formula injection: a text cell that a
/// spreadsheet would evaluate (leading `=`, `+`, `-`, `@`, tab, CR) gets a
/// leading `'`, unless it is a plain number such as issue `-1`.
pub fn formula_safe(s: &str) -> Cow<'_, str> {
    let risky = s
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
    if risky && !is_plain_number(s) {
        Cow::Owned(format!("'{s}"))
    } else {
        Cow::Borrowed(s)
    }
}

/// `[+-]digits[.digits]` and nothing else. Stricter than `f64::from_str`,
/// which also takes `inf` / `NaN` and would let `-inf` through unguarded.
fn is_plain_number(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    !int.is_empty() && digits(int) && digits(frac) && !(body.contains('.') && frac.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutralises_formula_cells_but_not_numbers() {
        assert_eq!(formula_safe("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(formula_safe("+cmd"), "'+cmd");
        assert_eq!(formula_safe("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(formula_safe("-1"), "-1");
        assert_eq!(formula_safe("-0.5"), "-0.5");
        assert_eq!(formula_safe("-Hellblazer"), "'-Hellblazer");
        assert_eq!(formula_safe("-inf"), "'-inf");
        assert_eq!(formula_safe("+Infinity"), "'+Infinity");
        assert_eq!(formula_safe("-NaN"), "'-NaN");
        assert_eq!(formula_safe("\t1"), "'\t1");
        assert_eq!(formula_safe("\rx"), "'\rx");
        assert_eq!(formula_safe("-1."), "'-1.");
        assert_eq!(formula_safe("+3"), "+3");
        assert_eq!(formula_safe("Batman"), "Batman");
        assert_eq!(formula_safe("a=b"), "a=b");
        assert_eq!(formula_safe(""), "");
    }
}
