//! What an archive rewrite keeps, drops, and regenerates — the single
//! policy every writer path follows (WP-2.6 (b), audit DI-11).
//!
//! Three writers rebuild archives: the sidecar rewrite
//! ([`crate::cbz_write::rebuild`], stream-copies pages and swaps the two
//! Folio-managed sidecars), the page editor
//! ([`crate::cbz_write::rebuild_pages`] / [`crate::cbt_write::write_pages`],
//! renames pages and appends `extras`), and the CBR→CBZ converter
//! ([`crate::cbz_write::write_pages`]). Pre-fix each one carried a
//! different subset of the non-page entries: the sidecar rewrite dropped
//! every `.xml` / `.json` / `.txt` (so a `CoMet.xml`, a `notes.txt` or an
//! embedded `.json` vanished on the first provider apply) while the page
//! editor preserved only `ComicInfo.xml` + `MetronInfo.xml`.
//!
//! The policy is now:
//!
//!   - **Junk** ([`is_junk_entry`]) is dropped by every writer: dotfiles,
//!     `Thumbs.db`, `desktop.ini`, anything under `__MACOSX/`. These are
//!     OS droppings, never user data.
//!   - **Folio sidecars** ([`is_folio_sidecar`]) — `ComicInfo.xml` and
//!     `MetronInfo.xml` at any depth — are the two files Folio regenerates.
//!     The sidecar rewrite replaces the root pair and drops stale nested
//!     copies (a `Sub/ComicInfo.xml` beside the root one confuses other
//!     readers); the page editor and converter carry the root pair
//!     through verbatim.
//!   - **Everything else** is carried through byte-for-byte: `CoMet.xml`,
//!     `.txt` / `.json` / `.nfo` notes, `series.json`, fonts, whatever the
//!     archive shipped with. Nested images are *pages* to every reader
//!     (the page list is "any entry with an image extension", directory
//!     ignored), so they follow the page path, not this one.

use crate::comic_archive::ComicArchive;
use crate::{ArchiveError, image_sniff};
use std::collections::HashSet;

/// OS / tooling droppings no writer carries through: dotfiles (`.DS_Store`,
/// `._foo`), `Thumbs.db`, `desktop.ini`, and anything under an `__MACOSX`
/// directory. Case-insensitive on the file name and the `__MACOSX`
/// component.
pub fn is_junk_entry(name: &str) -> bool {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    if leaf.starts_with('.')
        || leaf.eq_ignore_ascii_case("Thumbs.db")
        || leaf.eq_ignore_ascii_case("desktop.ini")
    {
        return true;
    }
    name.split('/').any(|p| p.eq_ignore_ascii_case("__MACOSX"))
}

/// The two sidecars Folio composes and rewrites: `ComicInfo.xml` and
/// `MetronInfo.xml`, matched on the leaf name case-insensitively at any
/// depth (so a stale nested duplicate counts too).
pub fn is_folio_sidecar(name: &str) -> bool {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    leaf.eq_ignore_ascii_case("ComicInfo.xml") || leaf.eq_ignore_ascii_case("MetronInfo.xml")
}

/// Entries the **sidecar rewrite** drops from the source before re-adding
/// the freshly composed root `ComicInfo.xml` / `MetronInfo.xml`: junk plus
/// every Folio sidecar (root and nested). Every other entry — pages and
/// foreign non-page files alike — streams through verbatim.
pub fn is_rewrite_dropped(name: &str) -> bool {
    is_junk_entry(name) || is_folio_sidecar(name)
}

/// Read every non-page, non-junk entry of `src` verbatim as
/// `(name, bytes, deflate_level)` triples — the `extras` list the page
/// editor and the CBR→CBZ converter append after the pages so a rewrite
/// never loses a `CoMet.xml`, a notes file or an embedded `.json`.
///
/// `keep_folio_sidecars` controls the root `ComicInfo.xml` /
/// `MetronInfo.xml` pair: the page editor and the converter pass `true`
/// (carry the existing metadata through untouched); a caller that is
/// about to write fresh sidecars passes `false` and adds its own. Nested
/// duplicates of the pair are always dropped (see the module doc).
///
/// "Page" here is the reader's own [`ComicArchive::pages`] set, so the
/// three formats agree with their page enumeration: an image-named
/// entry whose bytes failed the open-time sniff is neither a page nor
/// an extra (it was dropped from the index at open). Text sidecars get
/// deflate level 6; everything else is stored (level 0) since the
/// payload is usually already compressed.
pub fn preserved_extras(
    src: &mut dyn ComicArchive,
    keep_folio_sidecars: bool,
) -> Result<Vec<(String, Vec<u8>, i64)>, ArchiveError> {
    let page_names: HashSet<String> = src.pages().iter().map(|e| e.name.clone()).collect();
    let candidates: Vec<String> = src
        .entries()
        .iter()
        .map(|e| e.name.clone())
        .filter(|n| !page_names.contains(n))
        .filter(|n| !is_junk_entry(n))
        // An image-named non-page is a sniff casualty (or a reader that
        // lists images it can't decode) — not something to duplicate.
        .filter(|n| !image_sniff::has_image_extension(n))
        .filter(|n| {
            if !is_folio_sidecar(n) {
                return true;
            }
            // Root pair only, and only when the caller wants it.
            keep_folio_sidecars && !n.contains('/')
        })
        .collect();
    let mut extras = Vec::with_capacity(candidates.len());
    for name in candidates {
        let bytes = src.read_entry_bytes(&name)?;
        extras.push((name.clone(), bytes, deflate_level_for(&name)));
    }
    Ok(extras)
}

/// Deflate level for a carried-through extra: text-ish sidecars compress
/// well (6); anything else is stored (0).
pub fn deflate_level_for(name: &str) -> i64 {
    let ext = name
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "xml" | "json" | "txt" | "nfo" | "md" | "csv" | "html" | "htm" | "yaml" | "yml" => 6,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junk_predicate_matches_os_droppings_only() {
        for junk in [
            ".DS_Store",
            "sub/._page.jpg",
            "Thumbs.db",
            "Sub/thumbs.DB",
            "desktop.ini",
            "__MACOSX/p1.jpg",
            "issue/__macosx/x",
        ] {
            assert!(is_junk_entry(junk), "{junk} should be junk");
        }
        for keep in [
            "CoMet.xml",
            "notes.txt",
            "extras/cover-alt.jpg",
            "p1.jpg",
            "meta.json",
        ] {
            assert!(!is_junk_entry(keep), "{keep} should not be junk");
        }
    }

    #[test]
    fn folio_sidecar_predicate_is_case_insensitive_and_depth_agnostic() {
        assert!(is_folio_sidecar("ComicInfo.xml"));
        assert!(is_folio_sidecar("comicinfo.XML"));
        assert!(is_folio_sidecar("Sub Folder/MetronInfo.xml"));
        assert!(!is_folio_sidecar("CoMet.xml"));
        assert!(!is_folio_sidecar("ComicInfo.json"));
    }

    #[test]
    fn rewrite_dropped_is_union_of_junk_and_sidecars() {
        assert!(is_rewrite_dropped("Thumbs.db"));
        assert!(is_rewrite_dropped("MetronInfo.xml"));
        assert!(!is_rewrite_dropped("CoMet.xml"));
        assert!(!is_rewrite_dropped("notes.txt"));
        assert!(!is_rewrite_dropped("p1.jpg"));
    }

    #[test]
    fn deflate_level_text_vs_binary() {
        assert_eq!(deflate_level_for("CoMet.xml"), 6);
        assert_eq!(deflate_level_for("notes.TXT"), 6);
        assert_eq!(deflate_level_for("font.ttf"), 0);
        assert_eq!(deflate_level_for("noext"), 0);
    }
}
