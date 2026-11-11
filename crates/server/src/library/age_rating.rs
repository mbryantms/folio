//! ComicInfo `AgeRating` ladder (WP-2.7).
//!
//! The vocabulary is the ComicInfo v2.1 `AgeRating` enumeration, ordered
//! youngest-audience-first. `library_user_access.age_rating_max` names
//! one rung; a capped user sees every row whose rating ranks at or
//! below that rung.
//!
//! Decision D6 (2026-09-29): **unrated content is shown** to capped
//! users. `NULL`, empty, `Unknown`, `Rating Pending` and any string the
//! ladder doesn't know all count as unrated and pass. Only rows whose
//! rating is a known rung ranking *above* the cap are hidden. The SQL
//! form ([`hidden_ratings`]) therefore builds a `NOT IN (<above-cap>)`
//! list rather than an `IN (<allowed>)` list so the two agree exactly.
//!
//! **Aliases.** MetronInfo and the Metron API rate with their own words
//! (`Everyone`, `Teen`, `Teen Plus`, `Mature`, `Explicit`, `Adult`).
//! The first two are already rungs; the rest rank by [`ALIASES`]:
//!
//! | word        | ranks as          |
//! |-------------|-------------------|
//! | `Teen Plus` | `MA15+`           |
//! | `Mature`    | `Mature 17+`      |
//! | `Adult`     | `Adults Only 18+` |
//! | `Explicit`  | `X18+`            |
//!
//! Aliases are recognised at comparison time only (here and in every SQL
//! list built from [`spellings`]); stored text is never rewritten. GCD
//! ratings are mapped onto ladder words by the provider; ComicVine has
//! no rating. A rating declared in `series.json` is free text and is
//! read through [`declared_rung`] (bare ages such as `17+`, Mylar's
//! `All`, and the publisher wording [`from_publisher_text`] knows) before
//! it is stored as the series' floor; issue ratings are not.
//!
//! Comparisons are case-insensitive and ignore surrounding ASCII
//! whitespace ([`RATING_TRIM`], the same set the SQL [`sql_norm`] strips)
//! so a sidecar that says `mature 17+` still ranks.

use sea_orm::sea_query::{Alias, Expr, ExprTrait, Func, SimpleExpr};

/// The ladder, ordered youngest-audience-first. Index = rank.
pub const LADDER: [&str; 13] = [
    "Early Childhood",
    "Everyone",
    "G",
    "Everyone 10+",
    "PG",
    "Kids to Adults",
    "Teen",
    "MA15+",
    "Mature 17+",
    "M",
    "R18+",
    "Adults Only 18+",
    "X18+",
];

/// Off-ladder words that rank as a rung (Metron's vocabulary), as
/// `(word, rank)`. See the module docs for the mapping.
pub const ALIASES: [(&str, u8); 4] = [
    ("Teen Plus", 7),
    ("Mature", 8),
    ("Adult", 11),
    ("Explicit", 12),
];

/// Strings the ComicInfo vocabulary carries but which mean "no rating".
const UNRATED: [&str; 2] = ["unknown", "rating pending"];

/// Characters stripped from both ends of a rating before it is compared.
/// The SQL twin is the character list in [`sql_norm`]; keep them equal.
pub const RATING_TRIM: [char; 6] = [' ', '\t', '\n', '\r', '\x0c', '\x0b'];

/// [`RATING_TRIM`] as a SQL string literal, the second argument of
/// every rating `btrim`.
pub const SQL_TRIM_CHARS: &str = "E' \\t\\n\\r\\f\\013'";

/// `lower(btrim(<expr>, <RATING_TRIM>))`: the SQL normalisation every
/// rating comparison uses, stripping exactly [`RATING_TRIM`].
pub fn sql_norm(expr: &str) -> String {
    format!("lower(btrim({expr}, {SQL_TRIM_CHARS}))")
}

/// Every recognised spelling, lowercased, with its rank: the ladder
/// rungs, then [`ALIASES`]. The SQL ladder and the hidden / shown lists
/// are built from this so Rust and SQL rank the same words.
pub fn spellings() -> impl Iterator<Item = (String, u8)> {
    LADDER
        .iter()
        .enumerate()
        .map(|(i, r)| (r.to_lowercase(), i as u8))
        .chain(ALIASES.iter().map(|(w, r)| (w.to_lowercase(), *r)))
}

/// Rank of a rating on the ladder. `None` = unrated (NULL / empty /
/// `Unknown` / `Rating Pending` / anything neither the ladder nor
/// [`ALIASES`] knows).
pub fn rank(rating: &str) -> Option<u8> {
    let needle = rating.trim_matches(RATING_TRIM).to_lowercase();
    if needle.is_empty() || UNRATED.contains(&needle.as_str()) {
        return None;
    }
    spellings().find(|(w, _)| *w == needle).map(|(_, r)| r)
}

/// True iff `rating` is a value an edit may store: empty (clears it), a
/// ladder rung, an alias, or one of the ComicInfo "no rating" words.
pub fn is_recognised(rating: &str) -> bool {
    let needle = rating.trim_matches(RATING_TRIM).to_lowercase();
    needle.is_empty() || UNRATED.contains(&needle.as_str()) || rank(&needle).is_some()
}

/// The field message for an `age_rating` edit that is not
/// [`is_recognised`].
pub const UNRECOGNISED_MESSAGE: &str = "not a recognised age rating";

/// The field message for a series PATCH that sets `Unknown` or
/// `Rating Pending`: pinning either would leave the series unrated for
/// good and override its derived rating.
pub const SERIES_UNRATED_MESSAGE: &str =
    "clear the rating (null) instead of setting Unknown or Rating Pending";

/// garde rule for the series PATCH `age_rating`: [`garde_recognised`],
/// and not one of the "no rating" words (clear the rating instead, which
/// returns the series to its derived rating). Issues keep accepting them.
pub fn garde_series_rating(value: &Option<Option<String>>, ctx: &()) -> garde::Result {
    garde_recognised(value, ctx)?;
    match value {
        Some(Some(v)) if UNRATED.contains(&v.trim_matches(RATING_TRIM).to_lowercase().as_str()) => {
            Err(garde::Error::new(SERIES_UNRATED_MESSAGE))
        }
        _ => Ok(()),
    }
}

/// garde rule for a PATCH-style `age_rating` field (`None` = untouched,
/// `Some(None)` = clear): a set value must be [`is_recognised`]. Stored
/// values are never re-checked; only new edits are.
pub fn garde_recognised(value: &Option<Option<String>>, _: &()) -> garde::Result {
    match value {
        Some(Some(v)) if !is_recognised(v) => Err(garde::Error::new(UNRECOGNISED_MESSAGE)),
        _ => Ok(()),
    }
}

/// True iff a row with `rating` is visible to a user capped at `cap`.
///
/// - no cap → visible
/// - unrated row → visible (D6)
/// - cap not on the ladder → treated as no cap (the admin API refuses to
///   write one, so this only guards hand-edited rows)
/// - else `rank(rating) <= rank(cap)`
pub fn passes(rating: Option<&str>, cap: Option<&str>) -> bool {
    let Some(cap_rank) = cap.and_then(rank) else {
        return true;
    };
    match rating.and_then(rank) {
        None => true,
        Some(r) => r <= cap_rank,
    }
}

/// Every recognised spelling (rungs and aliases) ranking strictly above
/// `cap`, lowercased for a `sql_norm(col) NOT IN (...)` predicate. Empty
/// when the cap is the top rung or isn't on the ladder (→ nothing to hide).
pub fn hidden_ratings(cap: &str) -> Vec<String> {
    match rank(cap) {
        Some(cap_rank) => spellings()
            .filter(|(_, r)| *r > cap_rank)
            .map(|(w, _)| w)
            .collect(),
        None => Vec::new(),
    }
}

/// [`passes`] for a grant that may also hide unrated content
/// (`library_user_access.hide_unrated`). With `hide_unrated` on, a row
/// whose rating has no rank (NULL, empty, `Unknown`, off-ladder text) is
/// hidden as if it ranked above the cap; with no cap, that is all it
/// hides.
pub fn passes_grant(rating: Option<&str>, cap: Option<&str>, hide_unrated: bool) -> bool {
    if hide_unrated && rating.and_then(rank).is_none() {
        return false;
    }
    passes(rating, cap)
}

/// Every recognised spelling at or below `cap`, lowercased, for a
/// `sql_norm(col) = ANY(...)` predicate: what a grant with `hide_unrated`
/// on shows. Every spelling when there is no cap (or the cap is not on
/// the ladder, which [`passes`] treats as no cap).
pub fn shown_ratings(cap: Option<&str>) -> Vec<String> {
    let top = cap.and_then(rank).unwrap_or(u8::MAX);
    spellings()
        .filter(|(_, r)| *r <= top)
        .map(|(w, _)| w)
        .collect()
}

/// The highest-ranked rating among `ratings`, in its canonical ladder
/// spelling: how a series' rating is derived from its issues. Ratings
/// with no rank (NULL, `Unknown`, off-ladder text) are ignored, the same
/// way the cap treats them as unrated; `None` when nothing ranks. The
/// SQL twin is [`crate::library::series_rating`].
pub fn highest<'a>(ratings: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'static str> {
    ratings
        .into_iter()
        .filter_map(|r| r.and_then(rank))
        .max()
        .map(|r| LADDER[r as usize])
}

/// Filter values split the way filters compare a stored rating (the list
/// `?age_rating=` parameters and saved-view `age_rating` conditions).
///
/// A value that ranks (a rung or an alias, any case, surrounding
/// [`RATING_TRIM`]) stands for its whole rung: `spellings` holds every
/// recognised spelling of every rung named, lowercased, for a
/// `sql_norm(col) IN (...)` test. A value that does not rank (`Unknown`,
/// `Rating Pending`, off-ladder text) is kept in `verbatim` and matches
/// stored text exactly, as every rating filter did before ratings were
/// folded onto the ladder.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilterValues {
    pub spellings: Vec<String>,
    pub verbatim: Vec<String>,
}

/// Split filter `values` into [`FilterValues`].
pub fn filter_values<S: AsRef<str>>(values: &[S]) -> FilterValues {
    let mut ranks = Vec::new();
    let mut verbatim = Vec::new();
    for v in values {
        match rank(v.as_ref()) {
            Some(r) => ranks.push(r),
            None => {
                let v = v.as_ref().to_owned();
                if !verbatim.contains(&v) {
                    verbatim.push(v);
                }
            }
        }
    }
    FilterValues {
        spellings: spellings()
            .filter(|(_, r)| ranks.contains(r))
            .map(|(w, _)| w)
            .collect(),
        verbatim,
    }
}

/// `sql_norm` over a sea-query expression.
fn norm_expr(rating: SimpleExpr) -> SimpleExpr {
    Func::lower(
        Func::cust(Alias::new("btrim"))
            .arg(rating)
            .arg(Expr::cust(SQL_TRIM_CHARS)),
    )
    .into()
}

/// The filter predicate "`rating` is one of `values`" ([`filter_values`]):
/// `sql_norm(rating) IN (<spellings of the named rungs>) OR rating IN
/// (<unranked values as written>)`. A NULL rating matches nothing, and
/// its negation (`NOT (...)`) is NULL too, so `is_not` / `not_in` keep
/// excluding unrated rows exactly as the plain column comparison did.
/// `None` when `values` is empty.
pub fn filter_match<S: AsRef<str>>(rating: SimpleExpr, values: &[S]) -> Option<SimpleExpr> {
    let f = filter_values(values);
    let mut parts: Vec<SimpleExpr> = Vec::new();
    if !f.spellings.is_empty() {
        parts.push(norm_expr(rating.clone()).is_in(f.spellings));
    }
    if !f.verbatim.is_empty() {
        parts.push(rating.is_in(f.verbatim));
    }
    parts.into_iter().reduce(|a, b| a.or(b))
}

/// Publisher free-text rating → the ComicInfo `AgeRating` vocabulary
/// (GCD's `rating` field; also [`declared_rung`]'s fallback).
/// Publisher ratings (`"Rated T+"`, `"Parental Advisory"`, `"Mature
/// Readers"`, `"All Ages"`, `"Ages 12+"`, `"PSR"`) normalise; Comics-Code
/// approval text, `"[none]"` and anything unrecognised → `None` (better
/// empty than junk in the sidecar — the CCA seal isn't an age rating).
pub fn from_publisher_text(raw: &str) -> Option<String> {
    let r = raw.to_lowercase();
    let words: Vec<&str> = r
        .split(|c: char| !(c.is_alphanumeric() || c == '+'))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let phrase = |p: &str| r.contains(p);
    if phrase("rating pending") {
        return Some("Rating Pending".to_owned());
    }
    if phrase("comics code") || phrase("code authority") || words.is_empty() {
        return None;
    }
    // A publisher letter code counts only as the whole value ("T+") or
    // right after "rated" ("Rated M") — a bare "a"/"e" in prose is not
    // a rating.
    let code: Option<&str> = if words.len() == 1 {
        Some(words[0])
    } else {
        words.windows(2).find(|w| w[0] == "rated").map(|w| w[1])
    };
    // An explicit minimum age wins ("Ages 12+", "16+", "18+").
    let min_age = words.iter().find_map(|w| {
        w.strip_suffix('+')
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| (3..=21).contains(n))
    });
    let label = if has("explicit")
        || has("adult")
        || has("adults")
        || code.is_some_and(|c| matches!(c, "x" | "x18+" | "ao" | "max" | "r18+"))
    {
        "Adults Only 18+"
    } else if let Some(age) = min_age {
        match age {
            18.. => "Adults Only 18+",
            17 => "Mature 17+",
            13..=16 => "Teen",
            10..=12 => "Everyone 10+",
            _ => "Everyone",
        }
    } else if has("mature") || code.is_some_and(|c| matches!(c, "m" | "ma" | "17+")) {
        "Mature 17+"
    } else if has("teen")
        || has("psr")
        || has("psr+")
        || phrase("parental advisory")
        || phrase("parental supervision")
        || code.is_some_and(|c| matches!(c, "t" | "t+" | "pg" | "psr" | "psr+" | "pa"))
    {
        "Teen"
    } else if phrase("early childhood") {
        "Early Childhood"
    } else if phrase("all ages")
        || has("everyone")
        || has("kids")
        || code.is_some_and(|c| matches!(c, "e" | "a" | "g"))
    {
        "Everyone"
    } else {
        return None;
    };
    Some(label.to_owned())
}

/// The rung a rating declared in a `series.json` ranks as, in its
/// canonical ladder spelling; `None` when it names no rating.
///
/// Mylar writes `metadata.age_rating` as free text: only an operator sets
/// it, from a menu of `All Ages`, `9+`, `12+`, `15+`, `17+` and `Adult`
/// (the menu's unquoted HTML values submit `All Ages` as `All`). In order:
///
/// 1. a ladder rung or alias ranks as itself ([`rank`]);
/// 2. `Unknown` / `Rating Pending` / blank name nothing;
/// 3. `All` is `Everyone`;
/// 4. a bare minimum age (`17+`, `17`, `Ages 12+`, `12+ and up`) maps to
///    the nearest rung that is not milder than it ([`minimum_age_rung`]);
/// 5. anything else goes through [`from_publisher_text`] (GCD's publisher
///    wording: `All Ages`, `Rated T+`, `Mature Readers`, `PSR`, ...) and
///    counts only when that names a rung.
///
/// Only the declared series rating is read this way; issue ratings from
/// ComicInfo / MetronInfo still rank only as ladder words or aliases.
pub fn declared_rung(raw: &str) -> Option<&'static str> {
    if let Some(r) = rank(raw) {
        return Some(LADDER[r as usize]);
    }
    let needle = raw.trim_matches(RATING_TRIM).to_lowercase();
    if needle.is_empty() || UNRATED.contains(&needle.as_str()) {
        return None;
    }
    if needle == "all" {
        return Some("Everyone");
    }
    if let Some(age) = bare_minimum_age(&needle) {
        return Some(minimum_age_rung(age));
    }
    from_publisher_text(&needle)
        .and_then(|w| rank(&w))
        .map(|r| LADDER[r as usize])
}

/// `17+`, `17`, `ages 17+`, `age 17`, `17+ and up` (lowercased) → 17.
/// Ages above 21 are not read as ages.
fn bare_minimum_age(needle: &str) -> Option<u32> {
    let mut s = needle.trim();
    for prefix in ["ages", "age"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim_start_matches([' ', ':']);
            break;
        }
    }
    for suffix in ["and up", "and over", "and older"] {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest.trim_end();
            break;
        }
    }
    let digits = s.strip_suffix('+').unwrap_or(s).trim_end();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 2 {
        return None;
    }
    digits.parse::<u32>().ok().filter(|n| *n <= 21)
}

/// The rung for "ages `age` and up": the youngest-ranked rung whose own
/// minimum age is at least `age`, never a milder one. `Everyone` stands
/// for every age up to 6 (ESRB's E), `Everyone 10+` for 10, `Teen` for
/// 13, `MA15+` for 15, `Mature 17+` for 17 and `Adults Only 18+` for 18
/// (the stricter of the two 18+ rungs, as GCD maps `18+`).
pub fn minimum_age_rung(age: u32) -> &'static str {
    match age {
        0..=6 => "Everyone",
        7..=10 => "Everyone 10+",
        11..=13 => "Teen",
        14..=15 => "MA15+",
        16..=17 => "Mature 17+",
        _ => "Adults Only 18+",
    }
}

/// True iff `cap` names a rung on the ladder (what the admin API accepts
/// for `age_rating_max`). Aliases are not caps: a cap is a ladder word.
pub fn is_valid_cap(cap: &str) -> bool {
    let needle = cap.trim_matches(RATING_TRIM).to_lowercase();
    LADDER.iter().any(|r| r.to_lowercase() == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_strictly_ordered() {
        for w in LADDER.windows(2) {
            assert!(rank(w[0]).unwrap() < rank(w[1]).unwrap(), "{:?}", w);
        }
        assert_eq!(rank("Early Childhood"), Some(0));
        assert_eq!(rank("Teen"), Some(6));
        assert_eq!(rank("Mature 17+"), Some(8));
        assert_eq!(rank("X18+"), Some(12));
    }

    #[test]
    fn rank_is_case_and_whitespace_insensitive() {
        assert_eq!(rank("  mature 17+ "), Some(8));
        assert_eq!(rank("TEEN"), Some(6));
        assert_eq!(rank("everyone 10+"), Some(3));
    }

    #[test]
    fn unrated_strings_have_no_rank() {
        assert_eq!(rank(""), None);
        assert_eq!(rank("   "), None);
        assert_eq!(rank("Unknown"), None);
        assert_eq!(rank("rating pending"), None);
        assert_eq!(rank("Sixteen and up"), None);
    }

    #[test]
    fn passes_no_cap_shows_everything() {
        assert!(passes(Some("X18+"), None));
        assert!(passes(None, None));
    }

    #[test]
    fn passes_unrated_rows_shown_to_capped_users() {
        assert!(passes(None, Some("Teen")));
        assert!(passes(Some(""), Some("Teen")));
        assert!(passes(Some("Unknown"), Some("Teen")));
        assert!(passes(Some("Rating Pending"), Some("Everyone")));
        assert!(passes(Some("not-a-rating"), Some("Everyone")));
    }

    #[test]
    fn passes_compares_ranks() {
        assert!(passes(Some("Teen"), Some("Teen")));
        assert!(passes(Some("Everyone"), Some("Teen")));
        assert!(!passes(Some("Mature 17+"), Some("Teen")));
        assert!(!passes(Some("MA15+"), Some("Teen")));
        assert!(passes(Some("mature 17+"), Some("Adults Only 18+")));
        assert!(!passes(Some("X18+"), Some("Adults Only 18+")));
    }

    #[test]
    fn unknown_cap_is_treated_as_no_cap() {
        assert!(passes(Some("X18+"), Some("bogus")));
        assert!(passes(Some("X18+"), Some("Unknown")));
    }

    #[test]
    fn hidden_ratings_are_the_rungs_above_the_cap() {
        assert_eq!(
            hidden_ratings("Teen"),
            vec![
                "ma15+",
                "mature 17+",
                "m",
                "r18+",
                "adults only 18+",
                "x18+",
                "teen plus",
                "mature",
                "adult",
                "explicit"
            ]
        );
        assert!(hidden_ratings("X18+").is_empty());
        assert!(hidden_ratings("bogus").is_empty());
        assert_eq!(hidden_ratings("adults only 18+"), vec!["x18+", "explicit"]);
    }

    #[test]
    fn hide_unrated_hides_only_unranked_rows_without_a_cap() {
        assert!(!passes_grant(None, None, true));
        assert!(!passes_grant(Some("Unknown"), None, true));
        assert!(!passes_grant(Some("not-a-rating"), Some("Teen"), true));
        assert!(passes_grant(Some("X18+"), None, true));
        assert!(passes_grant(Some("teen"), Some("Teen"), true));
        assert!(!passes_grant(Some("Mature 17+"), Some("Teen"), true));
        assert!(passes_grant(None, Some("Teen"), false));
    }

    #[test]
    fn shown_and_hidden_partition_every_spelling() {
        let every: Vec<String> = spellings().map(|(w, _)| w).collect();
        for cap in LADDER {
            let mut all = shown_ratings(Some(cap));
            all.extend(hidden_ratings(cap));
            all.sort();
            let mut want = every.clone();
            want.sort();
            assert_eq!(all, want, "{cap}");
        }
        assert_eq!(shown_ratings(None).len(), every.len());
        assert_eq!(shown_ratings(Some("bogus")).len(), every.len());
    }

    #[test]
    fn metron_words_rank_as_their_rungs() {
        assert_eq!(rank("Teen Plus"), rank("MA15+"));
        assert_eq!(rank(" mature "), rank("Mature 17+"));
        assert_eq!(rank("ADULT"), rank("Adults Only 18+"));
        assert_eq!(rank("Explicit"), rank("X18+"));
        assert_eq!(rank("Everyone"), Some(1));
        assert!(!passes(Some("Teen Plus"), Some("Teen")));
        assert!(passes(Some("Mature"), Some("Mature 17+")));
        assert_eq!(highest([Some("Teen"), Some("Mature")]), Some("Mature 17+"));
        for (word, r) in ALIASES {
            assert!(
                LADDER.iter().all(|l| !l.eq_ignore_ascii_case(word)),
                "{word}"
            );
            assert!((r as usize) < LADDER.len());
        }
    }

    #[test]
    fn trim_set_is_ascii_whitespace_only() {
        assert_eq!(rank("\t\x0bTeen\r\n\x0c "), Some(6));
        // A non-breaking space is not trimmed (SQL btrim keeps it too).
        assert_eq!(rank("\u{a0}Teen"), None);
        assert_eq!(sql_norm("x"), "lower(btrim(x, E' \\t\\n\\r\\f\\013'))");
    }

    #[test]
    fn recognised_values_are_the_ladder_aliases_and_blank() {
        assert!(is_recognised("Teen"));
        assert!(is_recognised("teen plus"));
        assert!(is_recognised("  "));
        assert!(is_recognised("Unknown"));
        assert!(!is_recognised("Sixteen and up"));
        assert!(is_valid_cap("teen"));
        assert!(!is_valid_cap("Mature"));
        let set = |v: &str| Some(Some(v.to_owned()));
        assert!(garde_series_rating(&set("Teen Plus"), &()).is_ok());
        assert!(garde_series_rating(&Some(None), &()).is_ok());
        assert!(garde_series_rating(&set(" unknown "), &()).is_err());
        assert!(garde_series_rating(&set("Rating Pending"), &()).is_err());
        assert!(garde_recognised(&set("Rating Pending"), &()).is_ok());
    }

    #[test]
    fn highest_ignores_unranked_ratings() {
        assert_eq!(
            highest([Some("teen"), Some(" mature 17+ "), None]),
            Some("Mature 17+")
        );
        assert_eq!(highest([Some("Unknown"), Some("Sixteen"), None]), None);
        assert_eq!(highest(std::iter::empty()), None);
        assert_eq!(
            highest([Some("Everyone"), Some("Unknown")]),
            Some("Everyone")
        );
    }

    #[test]
    fn filter_values_fold_ranked_values_and_keep_the_rest_verbatim() {
        let f = filter_values(&[" mature ", "Unknown", "Teen", "Unknown", "Sixteen"]);
        assert_eq!(f.spellings, vec!["teen", "mature 17+", "mature"]);
        assert_eq!(f.verbatim, vec!["Unknown", "Sixteen"]);
        assert_eq!(filter_values(&["X18+"]).spellings, vec!["x18+", "explicit"]);
        assert_eq!(filter_values::<&str>(&[]), FilterValues::default());
    }

    #[test]
    fn filter_match_renders_one_norm_test_and_one_verbatim_test() {
        use sea_orm::sea_query::{PostgresQueryBuilder, Query};
        let render = |values: &[&str]| {
            let e = filter_match(Expr::col(Alias::new("age_rating")), values).unwrap();
            Query::select()
                .expr(Expr::val(1))
                .and_where(e)
                .to_string(PostgresQueryBuilder)
        };
        let sql = render(&["Mature", "Unknown"]);
        assert!(
            sql.contains(
                "LOWER(btrim(\"age_rating\", E' \\t\\n\\r\\f\\013')) IN ('mature 17+', 'mature')"
            ),
            "{sql}"
        );
        assert!(sql.contains("OR \"age_rating\" IN ('Unknown')"), "{sql}");
        assert!(!render(&["Teen"]).contains(" OR "));
        assert!(filter_match::<&str>(Expr::val(1), &[]).is_none());
    }

    #[test]
    fn declared_ratings_read_mylar_bare_ages_and_publisher_wording() {
        let cases: &[(&str, Option<&str>)] = &[
            // Ladder words and aliases rank as themselves.
            ("Mature 17+", Some("Mature 17+")),
            (" teen ", Some("Teen")),
            ("Adult", Some("Adults Only 18+")),
            ("Teen Plus", Some("MA15+")),
            ("M", Some("M")),
            ("PG", Some("PG")),
            ("Kids to Adults", Some("Kids to Adults")),
            // Mylar's menu (`All Ages` submits as `All`).
            ("All", Some("Everyone")),
            ("All Ages", Some("Everyone")),
            ("9+", Some("Everyone 10+")),
            ("12+", Some("Teen")),
            ("15+", Some("MA15+")),
            ("17+", Some("Mature 17+")),
            // Other bare ages: never a milder rung.
            ("3+", Some("Everyone")),
            ("6+", Some("Everyone")),
            ("7+", Some("Everyone 10+")),
            ("10+", Some("Everyone 10+")),
            ("13+", Some("Teen")),
            ("14+", Some("MA15+")),
            ("16+", Some("Mature 17+")),
            ("17", Some("Mature 17+")),
            ("18+", Some("Adults Only 18+")),
            ("21+", Some("Adults Only 18+")),
            ("Ages 12+", Some("Teen")),
            ("ages 9 and up", Some("Everyone 10+")),
            // Publisher wording through the GCD normaliser.
            ("Rated T+", Some("Teen")),
            ("T+", Some("Teen")),
            ("Parental Advisory", Some("Teen")),
            ("PSR", Some("Teen")),
            ("Suggested for Mature Readers", Some("Mature 17+")),
            ("Rated M", Some("Mature 17+")),
            ("Explicit Content", Some("Adults Only 18+")),
            ("MAX", Some("Adults Only 18+")),
            ("Rated E", Some("Everyone")),
            // Nothing.
            ("Unknown", None),
            ("Rating Pending", None),
            ("", None),
            ("Sixteen and up", None),
            ("99+", None),
            ("Approved by the Comics Code Authority", None),
        ];
        for (raw, want) in cases {
            assert_eq!(declared_rung(raw), *want, "{raw:?}");
        }
    }

    #[test]
    fn minimum_age_rungs_are_never_milder_and_climb_the_ladder() {
        let mut last = 0;
        for age in 0..=21 {
            let r = rank(minimum_age_rung(age)).unwrap();
            assert!(r >= last, "{age}");
            last = r;
        }
    }

    #[test]
    fn hidden_and_passes_agree_on_every_rung() {
        for cap in LADDER {
            let hidden = hidden_ratings(cap);
            for (rating, _) in spellings() {
                let in_hidden = hidden.contains(&rating);
                assert_eq!(
                    !in_hidden,
                    passes(Some(&rating), Some(cap)),
                    "{cap} / {rating}"
                );
            }
        }
    }
}
