//! Text scoring: normalisation, character and word error rates with
//! `[?]` wildcards, and a one-to-one assignment (Hungarian method).

/// Stands in for a reference `[?]` (an illegible word) after
/// normalisation. Never produced by a recogniser.
const WILD: char = '\u{FFFC}';
/// A space next to a wildcard: matches one space or nothing, free, so an
/// illegible word the recogniser dropped entirely costs nothing.
const OPT_SPACE: char = '\u{FFFD}';

/// Upper-case, whitespace collapsed to single spaces, trimmed. A
/// reference's `[?]` becomes the wildcard marker; a hypothesis never
/// carries one.
pub(crate) fn normalize(s: &str, is_reference: bool) -> String {
    let s = if is_reference {
        s.replace("[?]", &format!(" {WILD} "))
    } else {
        s.replace([WILD, OPT_SPACE], "")
    };
    let joined = s
        .to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !is_reference {
        return joined;
    }
    let chars: Vec<char> = joined.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let next_to_wild =
                (i > 0 && chars[i - 1] == WILD) || chars.get(i + 1).is_some_and(|n| *n == WILD);
            if c == ' ' && next_to_wild {
                OPT_SPACE
            } else {
                c
            }
        })
        .collect()
}

/// A normalised reference with its wildcard spaces turned back into
/// plain spaces (for display and word splitting).
pub(crate) fn display(reference: &str) -> String {
    reference.replace(OPT_SPACE, " ").replace(WILD, "[?]")
}

/// Character error rate of `hyp` against `reference` (both already
/// normalised): edit distance over characters divided by the number of
/// known reference characters. A wildcard matches any run of non-space
/// characters (including none) at no cost and does not count in the
/// denominator.
pub(crate) fn cer(reference: &str, hyp: &str) -> f64 {
    let r: Vec<char> = reference.chars().collect();
    let h: Vec<char> = hyp.chars().collect();
    let known = r.iter().filter(|c| **c != WILD && **c != OPT_SPACE).count();
    let d = char_distance(&r, &h);
    if known == 0 {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    d as f64 / known as f64
}

fn char_distance(r: &[char], h: &[char]) -> usize {
    let m = h.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for &rc in r {
        if rc == OPT_SPACE {
            cur[0] = prev[0];
            for j in 1..=m {
                let take = prev[j - 1] + usize::from(h[j - 1] != ' ');
                cur[j] = prev[j].min(take).min(cur[j - 1] + 1);
            }
        } else if rc == WILD {
            cur[0] = prev[0];
            for j in 1..=m {
                let absorb = if h[j - 1] == ' ' { 1 } else { 0 };
                cur[j] = prev[j].min(cur[j - 1] + absorb);
            }
        } else {
            cur[0] = prev[0] + 1;
            for j in 1..=m {
                let sub = prev[j - 1] + usize::from(h[j - 1] != rc);
                cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Word error rate: edit distance over whitespace-separated words,
/// divided by the number of known reference words. A reference word
/// containing a wildcard matches any one hypothesis word.
pub(crate) fn wer(reference: &str, hyp: &str) -> f64 {
    let reference = reference.replace(OPT_SPACE, " ");
    let r: Vec<&str> = reference.split_whitespace().collect();
    let h: Vec<&str> = hyp.split_whitespace().collect();
    let known = r.iter().filter(|w| !w.contains(WILD)).count();
    let m = h.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for rw in &r {
        let wild = rw.contains(WILD);
        cur[0] = prev[0] + usize::from(!wild);
        for j in 1..=m {
            let same = wild || *rw == h[j - 1];
            let sub = prev[j - 1] + usize::from(!same);
            let del = prev[j] + usize::from(!wild);
            cur[j] = sub.min(del).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    if known == 0 {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    prev[m] as f64 / known as f64
}

/// Cost that marks a pair as not allowed.
pub(crate) const FORBIDDEN: f64 = 1.0e6;

/// Minimum-cost one-to-one assignment of rows to columns. Returns, for
/// each row, the column it got (`None` when there are more rows than
/// columns, or when its only option was a [`FORBIDDEN`] pair).
pub(crate) fn assign(cost: &[Vec<f64>]) -> Vec<Option<usize>> {
    let n = cost.len();
    if n == 0 {
        return Vec::new();
    }
    let m = cost[0].len();
    if m == 0 {
        return vec![None; n];
    }
    // The algorithm below needs rows <= columns; transpose if not.
    if n > m {
        let t: Vec<Vec<f64>> = (0..m)
            .map(|j| (0..n).map(|i| cost[i][j]).collect())
            .collect();
        let col_for = hungarian(&t);
        let mut out = vec![None; n];
        for (j, i) in col_for.into_iter().enumerate() {
            if cost[i][j] < FORBIDDEN {
                out[i] = Some(j);
            }
        }
        return out;
    }
    hungarian(cost)
        .into_iter()
        .enumerate()
        .map(|(i, j)| (cost[i][j] < FORBIDDEN).then_some(j))
        .collect()
}

/// Classic O(n^2 m) Hungarian method for n <= m; returns the column of
/// each row.
fn hungarian(a: &[Vec<f64>]) -> Vec<usize> {
    let n = a.len();
    let m = a[0].len();
    let inf = f64::INFINITY;
    let mut u = vec![0.0; n + 1];
    let mut v = vec![0.0; m + 1];
    let mut p = vec![0usize; m + 1];
    let mut way = vec![0usize; m + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0usize;
            for j in 1..=m {
                if !used[j] {
                    let cur = a[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut row_to_col = vec![0usize; n];
    for j in 1..=m {
        if p[j] != 0 {
            row_to_col[p[j] - 1] = j - 1;
        }
    }
    row_to_col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        normalize(s, true)
    }

    #[test]
    fn normalises_case_and_space() {
        assert_eq!(normalize("  Hello\n  world ", false), "HELLO WORLD");
    }

    #[test]
    fn cer_basic() {
        assert_eq!(cer(&n("ABCD"), "ABCD"), 0.0);
        assert!((cer(&n("ABCD"), "ABXD") - 0.25).abs() < 1e-9);
        assert!((cer(&n("ABCD"), "") - 1.0).abs() < 1e-9);
        assert!((cer(&n("AB"), "ABCD") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn wildcard_matches_any_word() {
        let r = n("THE [?] DOG");
        assert_eq!(cer(&r, "THE BROWN DOG"), 0.0);
        assert_eq!(cer(&r, "THE DOG"), 0.0);
        assert_eq!(wer(&r, "THE DOG"), 0.0);
        assert_eq!(cer(&n("HELLO [?]"), "HELLO"), 0.0);
        assert_eq!(display(&r), "THE [?] DOG");
        assert_eq!(wer(&r, "THE BROWN DOG"), 0.0);
        // The wildcard does not swallow a whole extra word with its space.
        assert!(cer(&r, "THE BIG BROWN DOG") > 0.0);
    }

    #[test]
    fn wer_basic() {
        assert_eq!(wer("A B C", "A B C"), 0.0);
        assert!((wer("A B C", "A X C") - 1.0 / 3.0).abs() < 1e-9);
        assert!((wer("A B C", "A B") - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn assignment_is_one_to_one_and_optimal() {
        // Greedy would give row 0 column 0 (0.1) and row 1 column 1 (0.9);
        // the optimum is 0->1, 1->0 (0.2 + 0.2).
        let cost = vec![vec![0.1, 0.2], vec![0.2, 0.9]];
        assert_eq!(assign(&cost), vec![Some(1), Some(0)]);
    }

    #[test]
    fn assignment_handles_rectangles_and_forbidden() {
        let cost = vec![vec![0.5], vec![0.1], vec![FORBIDDEN]];
        assert_eq!(assign(&cost), vec![None, Some(0), None]);
        let cost = vec![vec![FORBIDDEN, 0.3, 0.1]];
        assert_eq!(assign(&cost), vec![Some(2)]);
        let cost = vec![vec![FORBIDDEN]];
        assert_eq!(assign(&cost), vec![None]);
    }
}
