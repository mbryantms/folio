//! Ignore-rule resolution (spec §5).
//!
//! Two layers:
//!   1. **Built-in patterns** (always on): dot files / dot folders, `__MACOSX`,
//!      `Thumbs.db`, `desktop.ini`, `.DS_Store`, `@eaDir`. The spec also
//!      excludes ComicInfo-adjacent metadata files (`.xml`, `.json`, `.txt`,
//!      `.nfo`) inside archives — that filtering happens in the archive
//!      crate, not here.
//!   2. **User globs** from `library.ignore_globs` (spec §5.2). Compiled via
//!      `globset` into a [`GlobSet`]; invalid patterns surface as 400 at the
//!      `PATCH /libraries/{id}` boundary so the DB never holds bad globs.
//!
//! Recognized archive extensions live in [`is_recognized_archive_ext`]. Today
//! that's `.cbz`; Milestone 12 adds `.cbr`, `.cb7`, `.cbt`.

use entity::library;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum IgnoreError {
    #[error("invalid glob pattern '{pattern}': {error}")]
    InvalidGlob { pattern: String, error: String },
    #[error("ignore_globs must be an array of strings")]
    NotStringArray,
}

/// Compiled ignore-rule set, cheap to clone.
#[derive(Debug, Clone)]
pub struct IgnoreRules {
    built_in: GlobSet,
    user: GlobSet,
}

impl IgnoreRules {
    /// Build from a library row. The user globs come from
    /// `library.ignore_globs` (JSONB array of strings).
    pub fn for_library(lib: &library::Model) -> Result<Self, IgnoreError> {
        let user_patterns = parse_ignore_globs(&lib.ignore_globs)?;
        let user = compile(&user_patterns)?;
        Ok(Self {
            built_in: built_in_set(),
            user,
        })
    }

    /// Test path. Matches against:
    ///   - the entry's **own name** against built-in patterns (dotfiles etc.).
    ///     Walking ancestor components would incorrectly skip walks rooted
    ///     under dot-prefixed parents like `/tmp/.tmpXXXX/...`. Recursive
    ///     skipping happens naturally because `WalkDir::filter_entry` returning
    ///     false skips the directory and its children.
    ///   - the full path against user globs.
    pub fn should_skip(&self, path: &Path) -> bool {
        if self.skip_by_builtin_name(path) {
            return true;
        }
        let p = path.to_string_lossy();
        self.user.is_match(p.as_ref())
    }

    fn skip_by_builtin_name(&self, path: &Path) -> bool {
        if let Some(name) = path.file_name() {
            let s = name.to_string_lossy();
            return self.built_in.is_match(s.as_ref());
        }
        false
    }

    /// User-glob-only test. Use this when the caller has already applied
    /// component-level built-in filtering (e.g. enumerate.rs's `name_str`
    /// checks) and only needs the user-configured layer.
    pub fn should_skip_user(&self, path: &Path) -> bool {
        let p = path.to_string_lossy();
        self.user.is_match(p.as_ref())
    }
}

impl Default for IgnoreRules {
    fn default() -> Self {
        Self {
            built_in: built_in_set(),
            user: GlobSet::empty(),
        }
    }
}

/// Validate a list of glob patterns. Returns Ok(()) when every pattern parses,
/// Err(InvalidGlob) for the first bad one. Used by the PATCH /libraries/{id}
/// handler before persisting to the DB.
pub fn validate_globs(patterns: &[String]) -> Result<(), IgnoreError> {
    let _ = compile(patterns)?;
    Ok(())
}

fn compile(patterns: &[String]) -> Result<GlobSet, IgnoreError> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| IgnoreError::InvalidGlob {
            pattern: p.clone(),
            error: e.to_string(),
        })?;
        b.add(g);
    }
    b.build().map_err(|e| IgnoreError::InvalidGlob {
        pattern: "<set>".to_string(),
        error: e.to_string(),
    })
}

fn parse_ignore_globs(json: &serde_json::Value) -> Result<Vec<String>, IgnoreError> {
    let arr = json.as_array().ok_or(IgnoreError::NotStringArray)?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        out.push(v.as_str().ok_or(IgnoreError::NotStringArray)?.to_string());
    }
    Ok(out)
}

fn built_in_set() -> GlobSet {
    // Spec §5.1. These are matched against individual path components, so
    // they are name-only patterns.
    let patterns = [
        ".*", // dot files / dot folders
        "__MACOSX",
        "Thumbs.db",
        "desktop.ini",
        ".DS_Store",
        "@eaDir",
    ];
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        // unwrap-safe: hard-coded patterns
        b.add(Glob::new(p).expect("built-in glob compiles"));
    }
    b.build().expect("built-in glob set builds")
}

/// Recognized comic-archive extensions per Milestone 12. `.cbz` and `.cbt`
/// are fully supported; `.cbr` and `.cb7` are accepted by the walker so the
/// scanner can emit a `UnsupportedArchiveFormat` health issue rather than
/// silently ignore them — full readers land in a follow-up plan.
pub fn is_recognized_archive_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "cbz" | "cbt" | "cbr" | "cb7"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_with(globs: serde_json::Value) -> library::Model {
        library::Model {
            id: uuid::Uuid::nil(),
            name: "x".into(),
            slug: "x".into(),
            root_path: "/x".into(),
            default_language: "eng".into(),
            default_reading_direction: "ltr".into(),
            dedupe_by_content: true,
            scan_schedule_cron: None,
            created_at: chrono::Utc::now().fixed_offset(),
            updated_at: chrono::Utc::now().fixed_offset(),
            last_scan_at: None,
            ignore_globs: globs,
            report_missing_comicinfo: false,
            file_watch_enabled: true,
            soft_delete_days: 30,
            thumbnails_enabled: true,
            thumbnail_format: "webp".into(),
            thumbnail_cover_quality: crate::library::thumbnails::DEFAULT_COVER_QUALITY as i32,
            thumbnail_page_quality: crate::library::thumbnails::DEFAULT_STRIP_QUALITY as i32,
            generate_page_thumbs_on_scan: false,
        }
    }

    #[test]
    fn dotfiles_skip_via_builtin() {
        // `should_skip` checks only the entry's *own* name. Recursive skipping
        // is the responsibility of `WalkDir::filter_entry` once a parent
        // directory matches. So `/lib/.DS_Store` skips (last component = .DS_Store),
        // `/lib/.cache/foo.cbz` does NOT skip on its own (last component = foo.cbz);
        // its parent `/lib/.cache` would have been matched and pruned by the walker.
        let r = IgnoreRules::default();
        assert!(r.should_skip(Path::new("/lib/.DS_Store")));
        assert!(r.should_skip(Path::new("/lib/.cache")));
        assert!(!r.should_skip(Path::new("/lib/.cache/foo.cbz")));
        assert!(!r.should_skip(Path::new("/lib/Series (2020)/01.cbz")));
        // Critically: a path WITHIN a tempdir like `/tmp/.tmpXXX/Series/01.cbz`
        // must not be skipped — only the entry's last component is tested.
        assert!(!r.should_skip(Path::new("/tmp/.tmpABCDE/Series Foo (2024)/01.cbz")));
    }

    #[test]
    fn macosx_thumbs_desktop_eadir_skip() {
        let r = IgnoreRules::default();
        assert!(r.should_skip(Path::new("/lib/__MACOSX")));
        assert!(r.should_skip(Path::new("/lib/Series/Thumbs.db")));
        assert!(r.should_skip(Path::new("/lib/Series/desktop.ini")));
        assert!(r.should_skip(Path::new("/lib/Series/@eaDir")));
    }

    #[test]
    fn user_glob_skips_match() {
        let lib = lib_with(serde_json::json!(["**/Promos/*.cbz"]));
        let r = IgnoreRules::for_library(&lib).unwrap();
        assert!(r.should_skip(Path::new("/lib/Series/Promos/preview.cbz")));
        assert!(!r.should_skip(Path::new("/lib/Series/Issue 01.cbz")));
    }

    #[test]
    fn invalid_glob_errors() {
        let err = validate_globs(&["[unterminated".to_string()]).unwrap_err();
        match err {
            IgnoreError::InvalidGlob { pattern, .. } => {
                assert_eq!(pattern, "[unterminated");
            }
            _ => panic!("wrong error: {err:?}"),
        }
    }

    #[test]
    fn ignore_globs_must_be_string_array() {
        let lib = lib_with(serde_json::json!("not-an-array"));
        let err = IgnoreRules::for_library(&lib).unwrap_err();
        assert!(matches!(err, IgnoreError::NotStringArray));
    }
}
