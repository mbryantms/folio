//! Ratcliff/Obershelp similarity (matching-accuracy-1.0 M2).
//!
//! Direct port of Python's `difflib.SequenceMatcher`. Three-pass
//! upper-bound chain: [`real_quick_ratio`] (O(1) — pure lengths) →
//! [`quick_ratio`] (O(n+m) — histogram intersection) → [`ratio`]
//! (O(n*m) average via greedy longest-common-substring recursion).
//!
//! Each pass is a valid upper bound on the next, so callers can
//! short-circuit when a candidate falls below their threshold
//! without paying the recursive cost. See [`three_pass_ratio`] for
//! the recommended entry point — it threads the threshold through
//! all three passes automatically.
//!
//! Why port instead of pulling a crate: the existing similarity
//! crates on crates.io (strsim, distance, …) all implement Jaro,
//! Jaro-Winkler, Levenshtein, etc. but **not** Ratcliff/Obershelp.
//! The behavior parity with ComicTagger is the whole point; we want
//! to produce the exact same numbers their `quick_ratio_threshold`
//! short-circuit produces.

use std::collections::HashMap;

/// Three-pass entry point. Returns the same value as [`ratio`] but
/// short-circuits to `0.0` when an upper-bound pass falls below
/// `threshold`. `threshold` is in the `[0.0, 1.0]` range — anything
/// outside collapses to the inclusive bounds.
///
/// Matches ComicTagger's
/// `_thresh_compare_titles` short-circuit pattern: real-quick gates
/// quick gates ratio, with `0.0` returned for any candidate the
/// caller would have discarded anyway.
pub fn three_pass_ratio(a: &str, b: &str, threshold: f32) -> f32 {
    let t = threshold.clamp(0.0, 1.0);
    if real_quick_ratio(a, b) < t {
        return 0.0;
    }
    if quick_ratio(a, b) < t {
        return 0.0;
    }
    ratio(a, b)
}

/// O(1) upper bound. `2 * min(la, lb) / (la + lb)`. Exactly mirrors
/// Python's `SequenceMatcher.real_quick_ratio`.
pub fn real_quick_ratio(a: &str, b: &str) -> f32 {
    let la = a.chars().count();
    let lb = b.chars().count();
    calculate_ratio(la.min(lb), la + lb)
}

/// Multiset-intersection upper bound. Counts each char in `a` that
/// has a matching slot left in `b`'s histogram. O(n + m); always
/// `>= ratio()` but always `<= real_quick_ratio()`. Mirrors Python's
/// `SequenceMatcher.quick_ratio`.
pub fn quick_ratio(a: &str, b: &str) -> f32 {
    let mut fullbcount: HashMap<char, i32> = HashMap::new();
    for c in b.chars() {
        *fullbcount.entry(c).or_insert(0) += 1;
    }
    let mut avail: HashMap<char, i32> = HashMap::new();
    let mut matches: usize = 0;
    for c in a.chars() {
        let numb = if let Some(&n) = avail.get(&c) {
            n
        } else {
            *fullbcount.get(&c).unwrap_or(&0)
        };
        avail.insert(c, numb - 1);
        if numb > 0 {
            matches += 1;
        }
    }
    let la = a.chars().count();
    let lb = b.chars().count();
    calculate_ratio(matches, la + lb)
}

/// Full Ratcliff/Obershelp ratio. `2 * M / (la + lb)` where M is the
/// total length of all matching blocks found by recursive
/// longest-common-substring search. Mirrors Python's
/// `SequenceMatcher.ratio` exactly (modulo the autojunk heuristic
/// which only kicks in past 200-char sequences — never the case for
/// comic titles).
pub fn ratio(a: &str, b: &str) -> f32 {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    if av.is_empty() && bv.is_empty() {
        return 1.0;
    }
    let total = av.len() + bv.len();
    if total == 0 {
        return 1.0;
    }
    let b2j = build_b2j(&bv);
    let matches = sum_matching_blocks(&av, &bv, &b2j, 0, av.len(), 0, bv.len());
    calculate_ratio(matches, total)
}

// ───────── internals ─────────

fn calculate_ratio(matches: usize, total: usize) -> f32 {
    if total == 0 {
        return 1.0;
    }
    (2.0 * matches as f32) / total as f32
}

fn build_b2j(b: &[char]) -> HashMap<char, Vec<usize>> {
    let mut map: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        map.entry(c).or_default().push(j);
    }
    map
}

/// Mirror of `SequenceMatcher.find_longest_match`. Returns
/// `(best_i, best_j, best_size)` — the start indices in `a` + `b`
/// and the length of the longest matching subsequence within the
/// half-open ranges `[alo, ahi)` + `[blo, bhi)`.
fn find_longest_match(
    a: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let mut besti = alo;
    let mut bestj = blo;
    let mut bestsize: usize = 0;
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    // Index-based loop mirrors the Python reference at
    // https://github.com/python/cpython/blob/main/Lib/difflib.py — easier
    // to audit for parity than an `enumerate`-based rewrite.
    #[allow(clippy::needless_range_loop)]
    for i in alo..ahi {
        let mut newj2len: HashMap<usize, usize> = HashMap::new();
        if let Some(positions) = b2j.get(&a[i]) {
            for &j in positions {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                // Extend the match starting at (i-k+1, j-k+1, k+1).
                let k = j2len.get(&j.wrapping_sub(1)).copied().unwrap_or(0) + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = newj2len;
    }
    (besti, bestj, bestsize)
}

/// Recursively walks the longest-common-substring tree, summing the
/// lengths of every matching block. Iterative-with-stack to avoid
/// blowing the call stack on pathological inputs (titles are short
/// in practice but the algorithm is the algorithm).
fn sum_matching_blocks(
    a: &[char],
    _b: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> usize {
    let mut stack = vec![(alo, ahi, blo, bhi)];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = stack.pop() {
        let (besti, bestj, bestsize) = find_longest_match(a, b2j, alo, ahi, blo, bhi);
        if bestsize == 0 {
            continue;
        }
        total += bestsize;
        // Recurse into the left + right halves around the match.
        if alo < besti && blo < bestj {
            stack.push((alo, besti, blo, bestj));
        }
        if besti + bestsize < ahi && bestj + bestsize < bhi {
            stack.push((besti + bestsize, ahi, bestj + bestsize, bhi));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────── parity with Python difflib.SequenceMatcher ─────────
    //
    // Each case below was generated by running the equivalent
    // `difflib.SequenceMatcher(None, a, b).ratio()` in CPython 3.12.

    #[test]
    fn ratio_matches_python_difflib_for_known_pairs() {
        let cases: &[(&str, &str, f32)] = &[
            // Identical
            ("saga", "saga", 1.0),
            ("", "", 1.0),
            // Single-char diff: 1 char swapped → 6 matches in 8 chars = 0.75
            ("saga", "sage", 0.75),
            // Off-by-one chars
            ("abcde", "abcd", 0.888_888_9),
            // No overlap
            ("abc", "xyz", 0.0),
            // Substring + prefix
            ("xmen", "xmen first class", 8.0 / 20.0),
            // Longer real-world series titles after sanitize_title.
            ("walking dead", "walking", 14.0 / 19.0),
            ("watchmen", "watchman", 7.0 / 8.0),
            // Empty / non-empty
            ("abc", "", 0.0),
            ("", "abc", 0.0),
        ];
        for &(a, b, expected) in cases {
            let got = ratio(a, b);
            assert!(
                (got - expected).abs() < 1e-3,
                "ratio({a:?}, {b:?}) = {got}, expected {expected}",
            );
        }
    }

    #[test]
    fn real_quick_ratio_is_upper_bound() {
        for &(a, b) in &[
            ("saga", "sage"),
            ("xmen", "x-men first class"),
            ("watchmen", "watchman"),
            ("abc", "xyz"),
            ("", "abc"),
        ] {
            let rqr = real_quick_ratio(a, b);
            let qr = quick_ratio(a, b);
            let r = ratio(a, b);
            assert!(
                rqr >= qr - 1e-3,
                "real_quick_ratio < quick_ratio for ({a:?}, {b:?}): {rqr} < {qr}",
            );
            assert!(
                qr >= r - 1e-3,
                "quick_ratio < ratio for ({a:?}, {b:?}): {qr} < {r}",
            );
        }
    }

    #[test]
    fn three_pass_short_circuits_below_threshold() {
        // Completely different strings — real_quick_ratio is 0 since
        // they share nothing in common at the histogram level too;
        // ratio() never runs.
        assert_eq!(three_pass_ratio("abc", "xyz", 0.5), 0.0);

        // Borderline match: "watchmen" vs "watchman" = 0.875.
        // Threshold 0.9 → quick_ratio gate trips, returns 0.
        assert_eq!(three_pass_ratio("watchmen", "watchman", 0.95), 0.0);

        // Same pair with a lower threshold → returns the real value.
        let r = three_pass_ratio("watchmen", "watchman", 0.5);
        assert!((r - 0.875).abs() < 1e-3);
    }

    #[test]
    fn unicode_chars_compared_by_codepoint() {
        // After NFKD upstream, accented chars are already decomposed,
        // but make sure we handle raw multi-byte chars without panic.
        assert!((ratio("café", "café") - 1.0).abs() < 1e-3);
        // Different chars but shared prefix.
        let r = ratio("café", "caff");
        assert!((0.0..=1.0).contains(&r));
    }

    #[test]
    fn empty_strings_return_one() {
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(quick_ratio("", ""), 1.0);
        assert_eq!(real_quick_ratio("", ""), 1.0);
    }

    #[test]
    fn longest_match_finds_substring() {
        // "abcdef" inside "xxabcdefyy" → length 6, starts at (0, 2).
        let a: Vec<char> = "abcdef".chars().collect();
        let b: Vec<char> = "xxabcdefyy".chars().collect();
        let b2j = build_b2j(&b);
        let (i, j, k) = find_longest_match(&a, &b2j, 0, a.len(), 0, b.len());
        assert_eq!((i, j, k), (0, 2, 6));
    }
}
