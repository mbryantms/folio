//! Security-audit L-4 / WP-6.3 regression guard: raw-SQL `LIMIT` / `OFFSET`
//! values are bound parameters (`LIMIT $n`), never `format!`-interpolated.
//!
//! The interpolated values were always clamped integers — not injectable —
//! but every `LIMIT {limit}` was a flag future auditors had to re-verify. The
//! sweep converted them all to `params.push(..)` + `LIMIT ${n}`; this test
//! keeps new ones from creeping back in. A compile-time *constant* (an
//! `UPPER_SNAKE` name such as `BROWSE_PUBLISHER_FACET_LIMIT`) is allowed —
//! it can't carry request input.

use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `true` when the text right after `LIMIT ` / `OFFSET ` opens a format
/// placeholder that isn't a SCREAMING_SNAKE constant.
fn interpolated_after(rest: &str) -> bool {
    let Some(inner) = rest.strip_prefix('{') else {
        return false;
    };
    let name: String = inner.chars().take_while(|c| *c != '}').collect();
    let is_const = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    !is_const
}

#[test]
fn raw_sql_limit_and_offset_are_bound_parameters() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (lineno, line) in text.lines().enumerate() {
            for kw in ["LIMIT ", "OFFSET "] {
                let mut from = 0;
                while let Some(pos) = line[from..].find(kw) {
                    let after = &line[from + pos + kw.len()..];
                    if interpolated_after(after) {
                        offenders.push(format!(
                            "{}:{}: {}",
                            file.strip_prefix(&src).unwrap().display(),
                            lineno + 1,
                            line.trim()
                        ));
                    }
                    from += pos + kw.len();
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "interpolated LIMIT/OFFSET — push the value onto the statement's \
         params and use `LIMIT ${{n}}` instead:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn detector_flags_interpolation_but_not_constants_or_placeholders() {
    assert!(interpolated_after("{limit}\""));
    assert!(interpolated_after("{}\", limit"));
    assert!(!interpolated_after("${n}"));
    assert!(!interpolated_after("$3"));
    assert!(!interpolated_after("{BROWSE_PUBLISHER_FACET_LIMIT}"));
}
