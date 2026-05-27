//! Filename inference fallback (§4.7).
//!
//! Pattern (Mylar3-style):
//!   `Series Name (Year) #001 (of 12) (Publisher) (Scanner).cbz`
//!
//! All bracketed groups are optional. `#NNN` may also be `vNNN` for a volume,
//! or `Annual N`, etc. The implementation is deliberately conservative: we
//! prefer to leave a field `None` than to guess wrong.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InferredName {
    pub series: String,
    pub number: Option<String>,
    pub volume: Option<i32>,
    pub year: Option<i32>,
    pub count: Option<i32>,
    pub publisher: Option<String>,
    pub extras: Vec<String>,
}

/// Per-library tuning for [`infer_with_opts`]. Both default OFF
/// (the conservative shape that matches our pre-M7 behavior). Mirror
/// the two ComicTagger toggles that close the most common
/// false-negative inference cases.
///
/// Matching-accuracy-1.0 M7.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InferOpts {
    /// When true, drop any leading numeric token from the filename
    /// before parsing. Closes the `001 - Saga.cbz` case where the
    /// leading number is curation padding, not a series identifier.
    pub ignore_leading_numbers: bool,
    /// When true, infer `1` as the issue number when no number is
    /// detected. Closes the one-shot / first-issue case.
    pub assume_issue_one: bool,
}

/// Strip extension and common scanner-tags directory remnants from the filename.
fn strip_extension(name: &str) -> &str {
    let ext_pos = name.rfind('.').unwrap_or(name.len());
    &name[..ext_pos]
}

/// Extract bracketed groups `(...)` and `[...]`, returning the cleaned base
/// and the list of group contents in order.
fn pull_groups(input: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(input.len());
    let mut groups = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' || c == '[' {
            let close = if c == '(' { ')' } else { ']' };
            let mut depth = 1;
            let mut group = String::new();
            for nc in chars.by_ref() {
                if nc == c {
                    depth += 1;
                    group.push(nc);
                } else if nc == close {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    group.push(nc);
                } else {
                    group.push(nc);
                }
            }
            groups.push(group.trim().to_string());
        } else {
            out.push(c);
        }
    }
    // Collapse whitespace.
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (cleaned, groups)
}

/// Returns true if the string is a 4-digit year between 1900–2100.
fn looks_like_year(s: &str) -> Option<i32> {
    if s.len() != 4 {
        return None;
    }
    let n: i32 = s.parse().ok()?;
    if (1900..=2100).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Extract a plausible `V<N>` volume token from a folder-leaf name.
///
/// Folder names commonly encode the volume separately from the
/// containing filename, e.g. `Deadpool & The Mercs For Money V2 (2016)`.
/// When a `series.json` sidecar is absent and ComicInfo / filename
/// inference can't tell sibling folders apart, this is the last signal
/// available before falling back to NULL.
///
/// Walks whitespace-separated tokens, looking for the first one shaped
/// like `V<digits>` / `v<digits>`. Filters through [`plausible_volume`]
/// so Mylar3's `V<year>` stamps (`V2016`, `V2023`, …) are silently
/// rejected — same rule applied everywhere a V-token is read.
pub fn folder_volume_token(folder_leaf: &str) -> Option<i32> {
    for token in folder_leaf.split_whitespace() {
        if let Some(rest) = token.strip_prefix('v').or_else(|| token.strip_prefix('V'))
            && let Ok(v) = rest.parse::<i32>()
            && plausible_volume(v, None)
        {
            return Some(v);
        }
    }
    None
}

/// Plausibility filter for a parsed `V<N>` volume token.
///
/// Real comic-series volumes are small positive integers — in mainstream
/// publishing, single digits and rarely above ~25. Mylar3 and similar
/// metadata fillers commonly stamp the publication year here as `V2016`,
/// `V2023`, etc., which contaminates the volume field if accepted at face
/// value. The filter:
///
///   - Rejects anything outside `[1, 99]` (year-range values, junk).
///   - Optionally rejects values that match the publication year, as a
///     final tie-breaker for the rare "Volume == Year" coincidence.
///     Pass `year: None` when no year context is available.
///
/// Used by every source that reads a `V<N>` token — filename inference
/// here, folder-name inference in the scanner, and the issue-level
/// majority vote during the post-ingest reconcile.
pub fn plausible_volume(v: i32, year: Option<i32>) -> bool {
    if !(1..=99).contains(&v) {
        return false;
    }
    if let Some(y) = year
        && v == y
    {
        return false;
    }
    true
}

pub fn infer(filename: &str) -> InferredName {
    infer_with_opts(filename, InferOpts::default())
}

/// Like [`infer`] but honors the per-library [`InferOpts`] toggles.
/// Matching-accuracy-1.0 M7.
pub fn infer_with_opts(filename: &str, opts: InferOpts) -> InferredName {
    let stem = strip_extension(filename);
    let (mut base, groups) = pull_groups(stem);
    let mut out = InferredName::default();

    // Issue / volume tokens before bracket groups: "#001", "v3", "Annual 1".
    // We scan from the right because filename usually ends with the issue token
    // before the bracketed groups (which we've already stripped).
    let mut tokens: Vec<&str> = base.split_whitespace().collect();

    // M7 ignore-leading-numbers: drop a bare leading numeric token if
    // present + there's more after it. Matches ComicTagger's
    // `_drop_leading_volume_number` heuristic — covers the common
    // `001 - Saga.cbz` shape where the leading number is curation
    // padding, not a series identifier. Applied BEFORE the right-end
    // pop loop so it can't conflict with the bare-number-as-issue
    // capture below.
    if opts.ignore_leading_numbers
        && tokens.len() > 1
        && tokens[0].chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        tokens.remove(0);
    }

    while let Some(&last) = tokens.last() {
        if let Some(rest) = last.strip_prefix('#')
            && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        {
            out.number = Some(rest.to_string());
            tokens.pop();
            continue;
        }
        if let Some(rest) = last.strip_prefix('v').or_else(|| last.strip_prefix('V'))
            && let Ok(v) = rest.parse::<i32>()
            && out.volume.is_none()
        {
            // Apply the same plausibility filter the resolver uses
            // downstream (see `plausible_volume`). Year context is
            // not known here yet, so we only check the [1, 99]
            // range — that alone rejects the Mylar3 `V<year>`
            // pattern (`V2016`, `V1995`, …) which is the dominant
            // contamination source.
            if plausible_volume(v, None) {
                out.volume = Some(v);
            }
            // Pop the token either way — leaving an implausible
            // `V2016` in the series-name run would be worse than
            // dropping it.
            tokens.pop();
            continue;
        }
        if last.chars().all(|c| c.is_ascii_digit() || c == '.') && tokens.len() > 1 {
            // Bare number at end → assume issue number, but only if there are >=2 tokens.
            out.number = Some(last.to_string());
            tokens.pop();
            continue;
        }
        break;
    }
    base = tokens.join(" ");

    for group in groups {
        let trimmed = group.trim();
        if let Some(year) = looks_like_year(trimmed)
            && out.year.is_none()
        {
            out.year = Some(year);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("of ")
            && let Ok(n) = rest.parse::<i32>()
        {
            out.count = Some(n);
            continue;
        }
        if (trimmed.starts_with("v") || trimmed.starts_with("V"))
            && let Ok(n) = trimmed[1..].parse::<i32>()
            && out.volume.is_none()
        {
            // Same plausibility gate as the inline-token branch above.
            if plausible_volume(n, None) {
                out.volume = Some(n);
            }
            continue;
        }
        // Common publisher hints (deliberately small; can grow):
        let lower = trimmed.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "marvel"
                | "dc"
                | "image"
                | "image comics"
                | "vertigo"
                | "dark horse"
                | "boom!"
                | "boom"
                | "idw"
                | "valiant"
                | "oni press"
                | "aftershock"
                | "fantagraphics"
        ) && out.publisher.is_none()
        {
            out.publisher = Some(trimmed.to_string());
            continue;
        }
        out.extras.push(trimmed.to_string());
    }

    out.series = base.trim().to_string();

    // M7 assume-issue-one: when no number was detected anywhere in
    // the filename, fall back to "1". Closes the one-shot /
    // first-issue case where the operator's curation strips the
    // `#1`. Only fires when the operator opts in per-library.
    if opts.assume_issue_one && out.number.is_none() {
        out.number = Some("1".to_string());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn classic_pattern() {
        let i = infer("Saga (2012) #001 (of 54) (Image) (digital-Empire).cbz");
        assert_eq!(i.series, "Saga");
        assert_eq!(i.number.as_deref(), Some("001"));
        assert_eq!(i.year, Some(2012));
        assert_eq!(i.count, Some(54));
        assert_eq!(i.publisher.as_deref(), Some("Image"));
        assert_eq!(i.extras, vec!["digital-Empire"]);
    }

    #[test]
    fn just_series_and_number() {
        let i = infer("Adventures #1.cbz");
        assert_eq!(i.series, "Adventures");
        assert_eq!(i.number.as_deref(), Some("1"));
    }

    #[test]
    fn series_with_year_only() {
        let i = infer("Daredevil (2019).cbz");
        assert_eq!(i.series, "Daredevil");
        assert_eq!(i.year, Some(2019));
        assert_eq!(i.number, None);
    }

    #[test]
    fn volume_token() {
        let i = infer("Berserk v01.cbz");
        assert_eq!(i.series, "Berserk");
        assert_eq!(i.volume, Some(1));
    }

    #[test]
    fn plausible_volume_filter() {
        // Real volumes pass.
        assert!(plausible_volume(1, None));
        assert!(plausible_volume(2, None));
        assert!(plausible_volume(25, None));
        assert!(plausible_volume(99, None));
        // Year-range Mylar3 stamps are rejected.
        assert!(!plausible_volume(1900, None));
        assert!(!plausible_volume(2016, None));
        assert!(!plausible_volume(2100, None));
        // Out-of-range junk.
        assert!(!plausible_volume(0, None));
        assert!(!plausible_volume(-5, None));
        assert!(!plausible_volume(100, None));
        // Year-equality tie-breaker — the rare Vol=Year coincidence
        // is treated as ambiguous and dropped.
        assert!(!plausible_volume(2, Some(2)));
        assert!(plausible_volume(2, Some(2016)));
    }

    #[test]
    fn vyear_token_rejected_as_volume() {
        // Mylar3 stamps `V<year>` in the filename to satisfy schemas
        // that want a V-token. It must NOT be promoted to the volume
        // field — that single bug poisoned 99.9% of one user's series
        // rows and caused sibling-folder collisions on same-year
        // multi-volume releases (e.g. Deadpool & The Mercs For Money
        // 2016 vs V2 (2016)).
        let i = infer("Deadpool & The Mercs For Money V2016 001 (April 2016).cbz");
        assert_eq!(i.series, "Deadpool & The Mercs For Money");
        assert_eq!(i.number.as_deref(), Some("001"));
        assert_eq!(i.volume, None, "V2016 must not be parsed as volume");
        // The `(April 2016)` bracket group doesn't match `looks_like_year`
        // (requires bare 4 digits), so year stays None — that's the
        // existing inference behavior, unchanged by the volume fix.
    }

    #[test]
    fn folder_volume_token_extracts_plausible_v() {
        assert_eq!(
            folder_volume_token("Deadpool & The Mercs For Money V2 (2016)"),
            Some(2),
        );
        assert_eq!(folder_volume_token("Howard the Duck V4 (2015)"), Some(4));
        assert_eq!(folder_volume_token("Berserk v3"), Some(3));
    }

    #[test]
    fn folder_volume_token_rejects_year_stamp() {
        assert_eq!(
            folder_volume_token("Green Arrow V2010 (2010)"),
            None,
            "V<year> must be rejected",
        );
        assert_eq!(folder_volume_token("Silk V2015 (2015)"), None);
    }

    #[test]
    fn folder_volume_token_returns_none_when_absent() {
        assert_eq!(
            folder_volume_token("Deadpool & The Mercs For Money (2016)"),
            None,
        );
        assert_eq!(folder_volume_token("Saga"), None);
        assert_eq!(folder_volume_token(""), None);
    }

    #[test]
    fn vyear_in_bracket_group_rejected_as_volume() {
        // Same Mylar3 stamp, but inside a bracket group: `(V2016)`.
        // Must also be rejected by the plausibility filter.
        let i = infer("Some Series (2014) (V2016).cbz");
        assert_eq!(i.year, Some(2014));
        assert_eq!(i.volume, None);
    }

    #[test]
    fn small_volume_in_bracket_group_still_works() {
        // `(v2)` in brackets still passes the plausibility filter.
        let i = infer("Wolverine #1 (v2) (2014).cbz");
        assert_eq!(i.volume, Some(2));
        assert_eq!(i.year, Some(2014));
    }

    #[test]
    fn unknown_publisher_kept_as_extra() {
        let i = infer("Series (2020) #5 (Garage).cbz");
        assert_eq!(i.publisher, None);
        assert_eq!(i.extras, vec!["Garage"]);
    }

    #[test]
    fn no_extension() {
        let i = infer("Series 100");
        assert_eq!(i.series, "Series");
        assert_eq!(i.number.as_deref(), Some("100"));
    }

    #[test]
    fn empty_input_does_not_panic() {
        let i = infer("");
        assert_eq!(i.series, "");
    }

    proptest! {
        #[test]
        fn never_panics(s in ".{0,200}") {
            let _ = infer(&s);
        }
    }

    // ────────────────────────────────────────────────────────────
    // M7 — InferOpts per-library toggles
    // ────────────────────────────────────────────────────────────

    #[test]
    fn ignore_leading_numbers_strips_curation_padding() {
        // Default (off) — leading number stays as series prefix.
        let off = infer("001 - Saga.cbz");
        assert!(off.series.contains("001"), "got series = {:?}", off.series);

        // Toggle on — leading number dropped, series clean.
        let on = infer_with_opts(
            "001 - Saga.cbz",
            InferOpts {
                ignore_leading_numbers: true,
                ..InferOpts::default()
            },
        );
        assert_eq!(on.series, "- Saga");
    }

    #[test]
    fn ignore_leading_numbers_does_not_strip_lone_number() {
        // `001.cbz` with one token only — guard says don't drop, else
        // we'd end up with an empty series for issue-only filenames.
        let i = infer_with_opts(
            "001.cbz",
            InferOpts {
                ignore_leading_numbers: true,
                ..InferOpts::default()
            },
        );
        assert!(!i.series.is_empty() || i.number.is_some());
    }

    #[test]
    fn assume_issue_one_fires_when_no_number_detected() {
        // Filename with no `#` token: pre-M7 → no inferred number.
        let off = infer("Saga - Origin.cbz");
        assert_eq!(off.number, None);

        // Toggle on → assume `1`.
        let on = infer_with_opts(
            "Saga - Origin.cbz",
            InferOpts {
                assume_issue_one: true,
                ..InferOpts::default()
            },
        );
        assert_eq!(on.number.as_deref(), Some("1"));
    }

    #[test]
    fn assume_issue_one_does_not_clobber_detected_number() {
        // Filename has #5 — toggle shouldn't overwrite to 1.
        let i = infer_with_opts(
            "Saga #5.cbz",
            InferOpts {
                assume_issue_one: true,
                ..InferOpts::default()
            },
        );
        assert_eq!(i.number.as_deref(), Some("5"));
    }

    #[test]
    fn both_toggles_compose_cleanly() {
        // `001 Saga.cbz` with leading-numbers off → series = "001 Saga"
        // (or something containing 001), no detected number.
        // With BOTH toggles → leading 001 dropped, no number detected
        // since "001" was popped as a leading token (not a trailing
        // issue token), so assume_issue_one fires → number = "1".
        let i = infer_with_opts(
            "001 Saga.cbz",
            InferOpts {
                ignore_leading_numbers: true,
                assume_issue_one: true,
            },
        );
        assert_eq!(i.series, "Saga");
        assert_eq!(i.number.as_deref(), Some("1"));
    }
}
