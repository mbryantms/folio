//! Text scoring: normalisation, and character and word error rates with
//! `[?]` wildcards.

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
    let joined = punctuation_spacing(
        &s.replace(['\u{2018}', '\u{2019}'], "'")
            .replace(['\u{201C}', '\u{201D}'], "\"")
            .replace('\u{2014}', "--")
            .replace('\u{2013}', "-")
            .to_uppercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
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

/// The reference's spacing convention, applied to both sides: no space
/// before `. , ! ? ; :`, none around `--`, and one space after `! ?`
/// before a letter or digit and after `. , ; :` before a letter.
fn punctuation_spacing(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == ' ' {
            let next = c.get(i + 1).copied();
            let next2 = c.get(i + 2).copied();
            let before_punct = matches!(next, Some('.' | ',' | '!' | '?' | ';' | ':'));
            let before_dash = next == Some('-') && next2 == Some('-');
            let after_dash =
                out.len() >= 2 && out[out.len() - 1] == '-' && out[out.len() - 2] == '-';
            if before_punct || before_dash || after_dash {
                i += 1;
                continue;
            }
        }
        out.push(ch);
        let next = c.get(i + 1).copied();
        let wants_space = match ch {
            '!' | '?' => next.is_some_and(char::is_alphanumeric),
            ',' | ';' | ':' | '.' => next.is_some_and(char::is_alphabetic),
            _ => false,
        };
        if wants_space {
            out.push(' ');
        }
        i += 1;
    }
    out.into_iter().collect()
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
    fn punctuation_spacing_is_normalised() {
        assert_eq!(normalize("PAPERS ? FOOL!MASS", false), "PAPERS? FOOL! MASS");
        assert_eq!(normalize("WAIT -- NO", false), "WAIT--NO");
        assert_eq!(normalize("AROUND ,RESET", false), "AROUND, RESET");
        assert_eq!(normalize("3.5 \u{2019}EM", false), "3.5 'EM");
        assert_eq!(normalize("OH...WELL", true), "OH... WELL");
        assert_eq!(normalize("500,000", true), "500,000");
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
}
