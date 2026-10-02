//! Issue-range citations in collected-edition text ("Collects Saga #1-6",
//! "Reprints Uncanny X-Men #129–137 and Annual #4", "issues 1-12").
//!
//! Pure string parsing, no regex dependency: scan for `#N-M` (or
//! `issues N-M`), then walk back to the nearest delimiter for the cited
//! series name. The name is normalised with
//! [`entity::series::normalize_name`] so it compares against
//! `series.normalized_name` directly. An empty name means "the same title
//! as the collected edition" — the caller substitutes its own base name.

use entity::series::normalize_name;

/// One `name #lo-hi` reference.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    /// Normalised cited series name; empty when the text named none.
    pub name_norm: String,
    pub lo: f64,
    pub hi: f64,
}

/// Widest range we accept. A "#1-9999" is a typo or a catalogue number,
/// not a collected run.
const MAX_SPAN: f64 = 1000.0;

/// Words that introduce a citation and are not part of the series name.
const LEAD_WORDS: &[&str] = &[
    "collects",
    "collecting",
    "collected",
    "reprints",
    "reprinting",
    "reprinted",
    "contains",
    "containing",
    "includes",
    "including",
    "material from",
    "from",
    "and",
    "plus",
    "with",
    "also",
    "originally published in",
    "published in",
];

/// Every range citation in `text`.
pub fn parse(text: &str) -> Vec<Citation> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // The name of a citation never reaches back into the previous one
    // ("X #1-5 and Y #2-3").
    let mut last_end = 0;
    while i < chars.len() {
        // `#N-M` form.
        if chars[i] == '#'
            && let Some((lo, hi, end)) = parse_range(&chars, i + 1)
        {
            let name = cited_name(&chars[last_end..i]);
            push(&mut out, name, lo, hi);
            i = end;
            last_end = end;
            continue;
        }
        // `issues N-M` form (no `#`).
        if word_at(&chars, i, "issues")
            && let Some((lo, hi, end)) = parse_range(&chars, i + "issues".len())
        {
            let name = cited_name(&chars[last_end..i]);
            push(&mut out, name, lo, hi);
            i = end;
            last_end = end;
            continue;
        }
        i += 1;
    }
    out
}

fn push(out: &mut Vec<Citation>, name: String, lo: f64, hi: f64) {
    if lo > hi || hi - lo > MAX_SPAN || lo < 0.0 {
        return;
    }
    let c = Citation {
        name_norm: name,
        lo,
        hi,
    };
    if !out.contains(&c) {
        out.push(c);
    }
}

/// Case-insensitive `word` at `i`, on a word boundary.
fn word_at(chars: &[char], i: usize, word: &str) -> bool {
    if i > 0 && chars[i - 1].is_alphanumeric() {
        return false;
    }
    let w: Vec<char> = word.chars().collect();
    if i + w.len() > chars.len() {
        return false;
    }
    let matches = chars[i..i + w.len()]
        .iter()
        .zip(&w)
        .all(|(a, b)| a.to_ascii_lowercase() == *b);
    matches && chars.get(i + w.len()).is_none_or(|c| !c.is_alphanumeric())
}

/// Parse `N <sep> [#]M` starting at `i` (leading spaces allowed). Separators:
/// `-`, en/em dash, `to`, `through`, `thru`. Returns `(lo, hi, end_index)`.
fn parse_range(chars: &[char], mut i: usize) -> Option<(f64, f64, usize)> {
    skip_ws(chars, &mut i);
    let lo = number(chars, &mut i)?;
    skip_ws(chars, &mut i);
    let sep_ok = match chars.get(i) {
        Some('-' | '\u{2013}' | '\u{2014}') => {
            i += 1;
            true
        }
        _ => {
            let mut ok = false;
            for w in ["through", "thru", "to"] {
                if word_at(chars, i, w) {
                    i += w.len();
                    ok = true;
                    break;
                }
            }
            ok
        }
    };
    if !sep_ok {
        return None;
    }
    skip_ws(chars, &mut i);
    if chars.get(i) == Some(&'#') {
        i += 1;
    }
    let hi = number(chars, &mut i)?;
    Some((lo, hi, i))
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while chars.get(*i).is_some_and(|c| c.is_whitespace()) {
        *i += 1;
    }
}

fn number(chars: &[char], i: &mut usize) -> Option<f64> {
    let start = *i;
    while chars.get(*i).is_some_and(char::is_ascii_digit) {
        *i += 1;
    }
    if *i == start || *i - start > 5 {
        return None;
    }
    // A decimal issue number ("#1.5") keeps its fraction.
    if chars.get(*i) == Some(&'.') && chars.get(*i + 1).is_some_and(char::is_ascii_digit) {
        *i += 1;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
    }
    chars[start..*i].iter().collect::<String>().parse().ok()
}

/// The series name right before a citation: back to the nearest clause
/// delimiter, minus lead-in words ("Collects", "and", …) and a trailing
/// "issues"/"vol N".
fn cited_name(before: &[char]) -> String {
    let mut start = before.len();
    while start > 0 {
        let c = before[start - 1];
        if matches!(
            c,
            ',' | ';' | '(' | ')' | '[' | ']' | ':' | '\n' | '#' | '"'
        ) {
            break;
        }
        // A sentence-ending period ("...story. Saga #1-6"), but not an
        // abbreviation dot inside a name ("Vol. 2", "Dr. Strange").
        if c == '.' && start >= 2 && before[start - 2].is_lowercase() {
            let word_start = before[..start - 1]
                .iter()
                .rposition(|c| !c.is_alphanumeric())
                .map_or(0, |p| p + 1);
            let word: String = before[word_start..start - 1].iter().collect();
            if !matches!(
                word.to_ascii_lowercase().as_str(),
                "vol" | "dr" | "mr" | "ms" | "st" | "no"
            ) {
                break;
            }
        }
        start -= 1;
    }
    let raw: String = before[start..].iter().collect();
    let mut name = normalize_name(&raw);
    // Strip lead-in words repeatedly ("collecting material from …").
    loop {
        let before_len = name.len();
        for lead in LEAD_WORDS {
            if let Some(rest) = name.strip_prefix(&format!("{lead} ")) {
                name = rest.to_owned();
            } else if name == *lead {
                name.clear();
            }
        }
        if name.len() == before_len {
            break;
        }
    }
    for tail in [" issues", " issue", " nos", " no"] {
        if let Some(rest) = name.strip_suffix(tail) {
            name = rest.to_owned();
        }
    }
    if name == "issues" || name == "issue" {
        name.clear();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, lo: f64, hi: f64) -> Citation {
        Citation {
            name_norm: name.into(),
            lo,
            hi,
        }
    }

    #[test]
    fn collects_with_name() {
        assert_eq!(parse("Collects Saga #1-6."), vec![c("saga", 1.0, 6.0)]);
        assert_eq!(
            parse("Collecting Uncanny X-Men #129\u{2013}137 and X-Men Annual #4-5"),
            vec![
                c("uncanny x men", 129.0, 137.0),
                c("x men annual", 4.0, 5.0)
            ]
        );
    }

    #[test]
    fn separators_and_hash_on_both_ends() {
        assert_eq!(parse("Saga #1 to #6"), vec![c("saga", 1.0, 6.0)]);
        assert_eq!(parse("Saga #7 through 12"), vec![c("saga", 7.0, 12.0)]);
        assert_eq!(parse("Saga #13 - #18"), vec![c("saga", 13.0, 18.0)]);
    }

    #[test]
    fn nameless_and_issues_form() {
        assert_eq!(parse("Collects #1-6"), vec![c("", 1.0, 6.0)]);
        assert_eq!(parse("Reprints issues 1-12"), vec![c("", 1.0, 12.0)]);
    }

    #[test]
    fn single_numbers_and_junk_are_ignored() {
        assert!(parse("Saga #1").is_empty());
        assert!(parse("Scraped metadata from ComicVine [CVDB684931].").is_empty());
        assert!(parse("Saga #6-1").is_empty(), "reversed range");
        assert!(parse("Saga #1-5000").is_empty(), "span too wide");
    }

    #[test]
    fn clause_delimiters_bound_the_name() {
        assert_eq!(
            parse("A great story. Collects Daredevil #1-5, Elektra #1-3"),
            vec![c("daredevil", 1.0, 5.0), c("elektra", 1.0, 3.0)]
        );
        assert_eq!(parse("Vol. 2: Saga #7-12"), vec![c("saga", 7.0, 12.0)]);
    }
}
