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

pub fn infer(filename: &str) -> InferredName {
    let stem = strip_extension(filename);
    let (mut base, groups) = pull_groups(stem);
    let mut out = InferredName::default();

    // Issue / volume tokens before bracket groups: "#001", "v3", "Annual 1".
    // We scan from the right because filename usually ends with the issue token
    // before the bracketed groups (which we've already stripped).
    let mut tokens: Vec<&str> = base.split_whitespace().collect();
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
            out.volume = Some(v);
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
            out.volume = Some(n);
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
}
