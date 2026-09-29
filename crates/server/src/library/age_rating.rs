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
//! Comparisons are case-insensitive and ignore surrounding whitespace so
//! a sidecar that says `mature 17+` still ranks.

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

/// Strings the ComicInfo vocabulary carries but which mean "no rating".
const UNRATED: [&str; 2] = ["unknown", "rating pending"];

/// Rank of a rating on the ladder. `None` = unrated (NULL / empty /
/// `Unknown` / `Rating Pending` / anything the ladder doesn't know).
pub fn rank(rating: &str) -> Option<u8> {
    let needle = rating.trim();
    if needle.is_empty() {
        return None;
    }
    if UNRATED.iter().any(|u| u.eq_ignore_ascii_case(needle)) {
        return None;
    }
    LADDER
        .iter()
        .position(|r| r.eq_ignore_ascii_case(needle))
        .map(|i| i as u8)
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

/// Canonical ladder spellings ranking strictly above `cap`, lowercased
/// for a `lower(btrim(col)) NOT IN (...)` predicate. Empty when the cap
/// is the top rung or isn't on the ladder (→ nothing to hide).
pub fn hidden_ratings(cap: &str) -> Vec<String> {
    match rank(cap) {
        Some(cap_rank) => LADDER
            .iter()
            .skip(cap_rank as usize + 1)
            .map(|r| r.to_ascii_lowercase())
            .collect(),
        None => Vec::new(),
    }
}

/// True iff `cap` names a rung on the ladder (what the admin API accepts
/// for `age_rating_max`).
pub fn is_valid_cap(cap: &str) -> bool {
    rank(cap).is_some()
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
                "x18+"
            ]
        );
        assert!(hidden_ratings("X18+").is_empty());
        assert!(hidden_ratings("bogus").is_empty());
        assert_eq!(hidden_ratings("adults only 18+"), vec!["x18+"]);
    }

    #[test]
    fn hidden_and_passes_agree_on_every_rung() {
        for cap in LADDER {
            let hidden = hidden_ratings(cap);
            for rating in LADDER {
                let in_hidden = hidden.contains(&rating.to_ascii_lowercase());
                assert_eq!(
                    !in_hidden,
                    passes(Some(rating), Some(cap)),
                    "{cap} / {rating}"
                );
            }
        }
    }
}
