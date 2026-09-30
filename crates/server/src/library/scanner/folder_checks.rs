//! Series-folder consistency checks (WP-3.4, audit R23).
//!
//! Runs once per series folder at series-resolve time, after the folder's
//! archives are ingested, and emits two health kinds through the scan's
//! [`HealthCollector`]:
//!
//! - [`IssueKind::FolderNameMismatch`] — the folder name disagrees with the
//!   ComicInfo `<Series>` its archives carry (spec §7.1).
//! - [`IssueKind::MixedSeriesInFolder`] — non-special archives in one
//!   folder carry more than one `<Series>` (spec §7.2).
//!
//! Both sides of every comparison are reduced by [`series_key`] first. The
//! key is deliberately forgiving so a healthy library produces zero rows:
//! bracket groups (`(2016)`, `[cv-4050-12345]`, `(Digital)`), volume tokens
//! (`v2`, `Vol. 3`), trailing bare numbers (`Batman 2016`, issue-folder
//! layouts like `Batman 001/`) and ComicTagger's article list are dropped,
//! and punctuation / accents / case are folded via
//! [`crate::metadata::title_norm::sanitize_title`] — the same pipeline the
//! matcher uses, so "the scanner and the matcher agree two names are the
//! same series" is one definition.
//!
//! Inputs come from the DB (the folder's active issues, projected to
//! `file_path`, `special_type` and `comic_info_raw->>'series'`) rather than
//! from the ingest loop, so files the size+mtime fast path skipped still
//! count. Rows are intersected with the folder's current on-disk archive
//! list, because the soft-delete reconcile for vanished files runs only
//! after every folder is processed. Folders the folder-level fast path
//! skips are `touch_folder`-ed by the caller, which keeps their rows open.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use entity::{issue, series_provider_range};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::library::health::{
    HealthCollector, IssueKind, MIXED_SERIES_VALUES_LIMIT, SeriesValueCount,
};
use crate::metadata::title_norm::sanitize_title;

/// Reduce a folder name or `<Series>` value to its comparison key. See the
/// module docs for what is folded away. Never returns an empty key for a
/// non-empty alphanumeric input: when stripping would erase everything (a
/// series literally named `1602`), the unstripped sanitized form is kept.
pub fn series_key(raw: &str) -> String {
    let base = strip_bracket_groups(raw);
    let sanitized = sanitize_title(&base);
    let tokens: Vec<&str> = sanitized.split_whitespace().collect();

    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        // `v2`, `v02` — volume token.
        if let Some(rest) = t.strip_prefix('v')
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            i += 1;
            continue;
        }
        // `vol 2`, `volume 2` (sanitize turned `Vol.` into `vol`).
        if (t == "vol" || t == "volume")
            && tokens
                .get(i + 1)
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        {
            i += 2;
            continue;
        }
        kept.push(t);
        i += 1;
    }
    // Trailing bare numbers: years (`Batman 2016`) and issue numbers
    // (`Batman 001` issue-folder layouts). Keep at least one token.
    while kept.len() > 1
        && kept
            .last()
            .is_some_and(|t| t.bytes().all(|b| b.is_ascii_digit()))
    {
        kept.pop();
    }
    if kept.is_empty() {
        return sanitized;
    }
    kept.join(" ")
}

/// Drop `(...)`, `[...]` and `{...}` groups (non-nested is enough for
/// folder names; an unbalanced opener drops the rest of the string).
fn strip_bracket_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// One non-special archive in the folder and its ComicInfo `<Series>`.
#[derive(Debug, Clone)]
pub struct FolderEntry {
    /// Path relative to the series folder.
    pub rel_path: String,
    pub comic_info_series: String,
}

/// Findings for one folder, before they're wrapped into [`IssueKind`]s.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FolderFindings {
    /// `(dominant <Series> spelling, files carrying its key)`.
    pub name_mismatch: Option<(String, u32)>,
    /// `(capped values, distinct count)` when > 1 distinct key.
    pub mixed: Option<(Vec<SeriesValueCount>, u32)>,
}

/// Pure analysis over already-loaded inputs. `folder_leaf` is the folder's
/// own name; `series_json_name` the folder sidecar's `name`, if any;
/// `range_names` the provider names of recorded divergence ranges.
pub fn analyze(
    folder_leaf: &str,
    entries: &[FolderEntry],
    series_json_name: Option<&str>,
    range_names: &[String],
) -> FolderFindings {
    struct Group {
        files: u32,
        spellings: HashMap<String, u32>,
        example: String,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    for e in entries {
        let raw = e.comic_info_series.trim();
        if raw.is_empty() {
            continue;
        }
        let key = series_key(raw);
        if key.is_empty() {
            continue;
        }
        let g = groups.entry(key).or_insert_with(|| Group {
            files: 0,
            spellings: HashMap::new(),
            example: e.rel_path.clone(),
        });
        g.files += 1;
        *g.spellings.entry(raw.to_owned()).or_default() += 1;
        if e.rel_path < g.example {
            g.example = e.rel_path.clone();
        }
    }
    if groups.is_empty() {
        // No ComicInfo `<Series>` anywhere — the series was named from
        // filenames / series.json, so there's nothing to disagree with.
        return FolderFindings::default();
    }

    // Most files first; key ascending as the stable tie-break.
    let mut ranked: Vec<(String, SeriesValueCount)> = groups
        .into_iter()
        .map(|(key, g)| {
            let mut spellings: Vec<(String, u32)> = g.spellings.into_iter().collect();
            spellings.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let series = spellings
                .into_iter()
                .next()
                .map(|s| s.0)
                .unwrap_or_default();
            (
                key,
                SeriesValueCount {
                    series,
                    files: g.files,
                    example: g.example,
                },
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.1.files.cmp(&a.1.files).then_with(|| a.0.cmp(&b.0)));

    let mut findings = FolderFindings::default();

    let (dominant_key, dominant) = &ranked[0];
    let folder_key = series_key(folder_leaf);
    let sidecar_confirms = series_json_name
        .map(series_key)
        .is_some_and(|k| !k.is_empty() && &k == dominant_key);
    if !folder_key.is_empty() && &folder_key != dominant_key && !sidecar_confirms {
        findings.name_mismatch = Some((dominant.series.clone(), dominant.files));
    }

    // A value naming a recorded provider-divergence range (e.g. a Metron
    // split of a legacy-renumbered run, composed into `<Series>` by
    // writeback) is a known, tracked exception — not a misfile.
    let range_keys: Vec<String> = range_names.iter().map(|n| series_key(n)).collect();
    let mixed: Vec<SeriesValueCount> = ranked
        .iter()
        .enumerate()
        .filter(|(i, (k, _))| *i == 0 || !range_keys.contains(k))
        .map(|(_, (_, v))| v.clone())
        .collect();
    if mixed.len() > 1 {
        let distinct = u32::try_from(mixed.len()).unwrap_or(u32::MAX);
        let mut capped = mixed;
        capped.truncate(MIXED_SERIES_VALUES_LIMIT);
        findings.mixed = Some((capped, distinct));
    }

    findings
}

/// Load the folder's inputs and emit any findings into `health`. Errors
/// are returned for the caller to log; the check is best-effort and must
/// never fail a scan.
pub async fn check_series_folder(
    db: &DatabaseConnection,
    series_id: Uuid,
    folder: &Path,
    archives: &[PathBuf],
    series_json_name: Option<&str>,
    health: &mut HealthCollector,
) -> anyhow::Result<()> {
    let on_disk: HashSet<&Path> = archives.iter().map(PathBuf::as_path).collect();
    // `comic_info_raw` is the serialized (MetronInfo-merged) ComicInfo;
    // project just the `series` key so the big jsonb never crosses the
    // wire. Older rows may carry the PascalCase key.
    let rows: Vec<(String, Option<String>, Option<String>)> = issue::Entity::find()
        .select_only()
        .column(issue::Column::FilePath)
        .column(issue::Column::SpecialType)
        .column_as(
            Expr::cust("COALESCE(comic_info_raw->>'series', comic_info_raw->>'Series')"),
            "ci_series",
        )
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .into_tuple()
        .all(db)
        .await?;

    let entries: Vec<FolderEntry> = rows
        .into_iter()
        .filter_map(|(file_path, special_type, ci_series)| {
            // Specials / annuals legitimately carry their own `<Series>`
            // (`Batman Annual`); they're not misfiles.
            if special_type.is_some() || !on_disk.contains(Path::new(&file_path)) {
                return None;
            }
            let rel = Path::new(&file_path).strip_prefix(folder).ok()?;
            Some(FolderEntry {
                rel_path: rel.to_string_lossy().into_owned(),
                comic_info_series: ci_series?,
            })
        })
        .collect();
    if entries.is_empty() {
        return Ok(());
    }

    let range_names: Vec<String> = series_provider_range::Entity::find()
        .select_only()
        .column(series_provider_range::Column::ProviderSeriesName)
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .into_tuple::<Option<String>>()
        .all(db)
        .await?
        .into_iter()
        .flatten()
        .collect();

    let folder_leaf = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let findings = analyze(&folder_leaf, &entries, series_json_name, &range_names);

    if let Some((comic_info_series, files)) = findings.name_mismatch {
        health.emit(IssueKind::FolderNameMismatch {
            folder: folder.to_string_lossy().into_owned(),
            series_id,
            comic_info_series,
            files,
        });
    }
    if let Some((series_values, distinct_values)) = findings.mixed {
        health.emit(IssueKind::MixedSeriesInFolder {
            folder: folder.to_path_buf(),
            series_id,
            series_values,
            distinct_values,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str, series: &str) -> FolderEntry {
        FolderEntry {
            rel_path: rel.to_owned(),
            comic_info_series: series.to_owned(),
        }
    }

    #[test]
    fn key_folds_common_folder_decorations() {
        let k = series_key("Batman");
        for folder in [
            "Batman (2016)",
            "Batman [cv-4050-91273]",
            "Batman v3 (2016) (Digital)",
            "Batman Vol. 3",
            "Batman 2016",
            "Batman 001",
            "The Batman",
            "batman",
        ] {
            assert_eq!(series_key(folder), k, "{folder}");
        }
        assert_eq!(
            series_key("Spider-Man's Tangled Web"),
            series_key("Spider-Mans Tangled Web")
        );
        assert_eq!(series_key("X-Men: Blue"), series_key("X-Men - Blue"));
        assert_eq!(series_key("Batman & Robin"), series_key("Batman and Robin"));
        assert_eq!(
            series_key("Pokémon Adventures"),
            series_key("Pokemon Adventures")
        );
        // Numbers that ARE the name survive.
        assert_eq!(series_key("1602"), "1602");
        assert_eq!(series_key("2000 AD"), "2000 ad");
        assert_ne!(series_key("Batman: Rebirth"), k);
    }

    #[test]
    fn matching_folder_is_quiet() {
        let f = analyze(
            "Saga (2012)",
            &[entry("Saga 001.cbz", "Saga"), entry("Saga 002.cbz", "Saga")],
            None,
            &[],
        );
        assert_eq!(f, FolderFindings::default());
    }

    #[test]
    fn spec_example_is_a_mismatch() {
        let f = analyze(
            "Batman (2016)",
            &[
                entry("Batman 001.cbz", "Batman: Rebirth"),
                entry("Batman 002.cbz", "Batman: Rebirth"),
            ],
            None,
            &[],
        );
        assert_eq!(f.name_mismatch, Some(("Batman: Rebirth".to_owned(), 2)));
        assert!(f.mixed.is_none());
    }

    #[test]
    fn series_json_agreeing_with_comicinfo_suppresses_mismatch() {
        let f = analyze(
            "Batman (2016)",
            &[entry("Batman 001.cbz", "Batman: Rebirth")],
            Some("Batman - Rebirth"),
            &[],
        );
        assert!(f.name_mismatch.is_none());
    }

    #[test]
    fn no_comicinfo_series_is_quiet() {
        let f = analyze("Whatever", &[entry("a.cbz", "  ")], None, &[]);
        assert_eq!(f, FolderFindings::default());
    }

    #[test]
    fn mixed_values_rank_by_file_count() {
        let f = analyze(
            "Saga",
            &[
                entry("Saga 001.cbz", "Saga"),
                entry("Saga 002.cbz", "Saga"),
                entry("Paper Girls 001.cbz", "Paper Girls"),
                entry("Saga 003.cbz", "SAGA"),
            ],
            None,
            &[],
        );
        assert!(f.name_mismatch.is_none());
        let (values, distinct) = f.mixed.expect("mixed");
        assert_eq!(distinct, 2);
        assert_eq!(values[0].series, "Saga");
        assert_eq!(values[0].files, 3);
        assert_eq!(values[1].series, "Paper Girls");
        assert_eq!(values[1].example, "Paper Girls 001.cbz");
    }

    #[test]
    fn spelling_variants_are_not_mixed() {
        let f = analyze(
            "Amazing Spider-Man (2018)",
            &[
                entry("a.cbz", "The Amazing Spider-Man"),
                entry("b.cbz", "Amazing Spider-Man"),
                entry("c.cbz", "Amazing Spider-Man (2018)"),
            ],
            None,
            &[],
        );
        assert_eq!(f, FolderFindings::default());
    }

    #[test]
    fn provider_range_names_are_not_mixed() {
        let f = analyze(
            "Fantastic Four",
            &[
                entry("FF 599.cbz", "Fantastic Four"),
                entry("FF 600.cbz", "Fantastic Four Legacy"),
            ],
            None,
            &["Fantastic Four Legacy (2012)".to_owned()],
        );
        assert!(f.mixed.is_none());
    }

    #[test]
    fn mixed_values_are_capped() {
        let entries: Vec<FolderEntry> = (0..15)
            .map(|i| {
                entry(
                    &format!("{i}.cbz"),
                    &format!("Series {}", (b'a' + i) as char),
                )
            })
            .collect();
        let (values, distinct) = analyze("Dump", &entries, None, &[]).mixed.unwrap();
        assert_eq!(distinct, 15);
        assert_eq!(values.len(), MIXED_SERIES_VALUES_LIMIT);
    }
}
