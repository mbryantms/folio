//! URL-slug generation and conflict-resolving allocation (M1 of the
//! human-readable-URLs plan, `~/.claude/plans/let-s-create-a-new-merry-finch.md`).
//!
//! Slugs are stored on each entity (libraries, series, issues, …) and used
//! as the user-facing URL identifier instead of raw UUIDs. Allocation runs
//! at insert/scan time and is *persisted*, so URLs stay stable across
//! renames; admin override paths re-allocate on demand.

use entity::{issue, library, series, user_page};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect,
};
use std::collections::HashSet;
use std::error::Error;
use uuid::Uuid;

/// Maximum length of a base slug before any disambiguating suffix is
/// appended. Generous enough to keep most series titles intact while still
/// fitting comfortably under URL/path length limits even after a multi-token
/// suffix like `-2026-vol-2`.
pub const MAX_BASE_LEN: usize = 80;

/// Fallback when an input slugifies to the empty string (e.g., a CJK-only
/// title with no transliteration available, or a name made entirely of
/// punctuation). The allocator's numeric-suffix path makes successive empty
/// inputs safe (`untitled`, `untitled-2`, `untitled-3`, …).
pub const EMPTY_FALLBACK: &str = "untitled";

/// Slugify a single name segment. ASCII-folds, lowercases, replaces runs of
/// non-alphanumeric characters with hyphens, trims leading/trailing
/// hyphens, and caps at [`MAX_BASE_LEN`]. The empty result is normalized to
/// [`EMPTY_FALLBACK`] so callers always have something to feed to the
/// allocator.
pub fn slugify_segment(input: &str) -> String {
    let mut s = ::slug::slugify(input);
    if s.len() > MAX_BASE_LEN {
        // Cut at a char boundary; `slug::slugify` only emits ASCII so any
        // index up to `len()` is valid, but be defensive against future
        // changes.
        let mut end = MAX_BASE_LEN;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        // Re-trim in case the cut landed mid-token and left a trailing dash.
        while s.ends_with('-') {
            s.pop();
        }
    }
    if s.is_empty() {
        EMPTY_FALLBACK.to_string()
    } else {
        s
    }
}

/// Looks up whether a candidate slug is already taken within the entity's
/// uniqueness scope (e.g., across all libraries; or `(series_id, slug)` for
/// issues). The implementation lives next to the call site so each entity
/// can express its own scope.
#[async_trait::async_trait]
pub trait SlugAllocator {
    type Error: Error + Send + Sync + 'static;

    /// Returns `Ok(true)` when the slug is taken (cannot be used) and
    /// `Ok(false)` when it's free.
    async fn is_taken(&self, candidate: &str) -> Result<bool, Self::Error>;
}

/// Allocate a unique slug for a new entity.
///
/// Strategy:
///   1. `slugify_segment(base)` → candidate.
///   2. If free, return it.
///   3. Otherwise, try each `disambiguator` in order, appending it as a
///      kebab-case suffix (`<base>-<disambiguator>`). The disambiguators
///      are deterministic, content-derived hints — typically year, then
///      `vol-N`, then publisher-slug for series; birth-year for creators.
///      The caller passes them already in the desired precedence order.
///   4. Falling back to numeric suffixes `-2`, `-3`, …. The numeric path is
///      always available so allocation cannot fail under normal conditions.
///
/// Each disambiguator is run through `slugify_segment` itself so callers
/// can pass raw publisher names like `"Image Comics"` and get
/// `"image-comics"` glued onto the base.
pub async fn allocate_slug<A>(
    base: &str,
    disambiguators: &[&str],
    allocator: &A,
) -> Result<String, A::Error>
where
    A: SlugAllocator + ?Sized,
{
    let base = slugify_segment(base);
    if !allocator.is_taken(&base).await? {
        return Ok(base);
    }

    for disambiguator in disambiguators {
        let suffix = slugify_segment(disambiguator);
        // `slugify_segment` always returns a non-empty result; an "empty"
        // disambiguator (caller passed `""`) becomes `untitled`, which
        // would produce a meaningless `<base>-untitled`. Skip those.
        if suffix == EMPTY_FALLBACK {
            continue;
        }
        let candidate = format!("{base}-{suffix}");
        if !allocator.is_taken(&candidate).await? {
            return Ok(candidate);
        }
    }

    // Numeric suffix fallback. Starts at 2 so the first collision becomes
    // `<base>-2` (matching common slug-suffix UX in CMSes / WordPress / etc.).
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !allocator.is_taken(&candidate).await? {
            return Ok(candidate);
        }
        n += 1;
    }
}

// ───── entity-specific allocators ─────
//
// Each entity has its own uniqueness scope (global for libraries/series,
// per-series for issues) and so needs its own `SlugAllocator` adapter
// around the shared `crate::slug::allocate_slug` machinery. The `excluding`
// field lets the admin override path re-allocate while ignoring the row's
// own current slug — otherwise renaming `spider-man` to itself would
// always collide.

/// SlugAllocator for the `libraries` table. Scope: all libraries.
pub struct LibrarySlugAllocator<'a, C: ConnectionTrait> {
    pub db: &'a C,
    /// Library id whose existing slug should NOT count as taken (set on
    /// admin rename; `None` for fresh inserts).
    pub excluding: Option<Uuid>,
}

#[async_trait::async_trait]
impl<C: ConnectionTrait> SlugAllocator for LibrarySlugAllocator<'_, C> {
    type Error = DbErr;
    async fn is_taken(&self, candidate: &str) -> Result<bool, DbErr> {
        let mut q = library::Entity::find().filter(library::Column::Slug.eq(candidate));
        if let Some(id) = self.excluding {
            q = q.filter(library::Column::Id.ne(id));
        }
        Ok(q.count(self.db).await? > 0)
    }
}

/// SlugAllocator for the `series` table. Scope: all series.
pub struct SeriesSlugAllocator<'a, C: ConnectionTrait> {
    pub db: &'a C,
    pub excluding: Option<Uuid>,
}

#[async_trait::async_trait]
impl<C: ConnectionTrait> SlugAllocator for SeriesSlugAllocator<'_, C> {
    type Error = DbErr;
    async fn is_taken(&self, candidate: &str) -> Result<bool, DbErr> {
        let mut q = series::Entity::find().filter(series::Column::Slug.eq(candidate));
        if let Some(id) = self.excluding {
            q = q.filter(series::Column::Id.ne(id));
        }
        Ok(q.count(self.db).await? > 0)
    }
}

/// SlugAllocator for the `issues` table. Scope: a single parent series.
pub struct IssueSlugAllocator<'a, C: ConnectionTrait> {
    pub db: &'a C,
    pub series_id: Uuid,
    /// Issue id whose existing slug should NOT count as taken. `String`
    /// rather than UUID because issue ids are BLAKE3 hex.
    pub excluding: Option<String>,
}

#[async_trait::async_trait]
impl<C: ConnectionTrait> SlugAllocator for IssueSlugAllocator<'_, C> {
    type Error = DbErr;
    async fn is_taken(&self, candidate: &str) -> Result<bool, DbErr> {
        let mut q = issue::Entity::find()
            .filter(issue::Column::SeriesId.eq(self.series_id))
            .filter(issue::Column::Slug.eq(candidate));
        if let Some(id) = &self.excluding {
            q = q.filter(issue::Column::Id.ne(id.as_str()));
        }
        Ok(q.count(self.db).await? > 0)
    }
}

/// SlugAllocator for the `user_page` table. Scope: a single user.
/// `excluding` lets the rename path skip the page's own current slug so
/// renaming "Marvel" to itself doesn't always collide.
pub struct UserPageSlugAllocator<'a, C: ConnectionTrait> {
    pub db: &'a C,
    pub user_id: Uuid,
    pub excluding: Option<Uuid>,
}

#[async_trait::async_trait]
impl<C: ConnectionTrait> SlugAllocator for UserPageSlugAllocator<'_, C> {
    type Error = DbErr;
    async fn is_taken(&self, candidate: &str) -> Result<bool, DbErr> {
        let mut q = user_page::Entity::find()
            .filter(user_page::Column::UserId.eq(self.user_id))
            .filter(user_page::Column::Slug.eq(candidate));
        if let Some(id) = self.excluding {
            q = q.filter(user_page::Column::Id.ne(id));
        }
        Ok(q.count(self.db).await? > 0)
    }
}

// ───── convenience helpers ─────

/// Allocate a globally-unique slug for a new library. Falls back to numeric
/// suffixes — libraries don't have a natural disambiguator (the user
/// supplies the name).
pub async fn allocate_library_slug<C: ConnectionTrait>(
    db: &C,
    name: &str,
) -> Result<String, DbErr> {
    allocate_slug(
        name,
        &[],
        &LibrarySlugAllocator {
            db,
            excluding: None,
        },
    )
    .await
}

/// Allocate a globally-unique slug for a new series. Disambiguators in
/// precedence order: year → `vol-{volume}` → publisher.
pub async fn allocate_series_slug<C: ConnectionTrait>(
    db: &C,
    name: &str,
    year: Option<i32>,
    volume: Option<i32>,
    publisher: Option<&str>,
) -> Result<String, DbErr> {
    let year_str = year.map(|y| y.to_string());
    let volume_str = volume.map(|v| format!("vol-{v}"));
    let mut disambiguators: Vec<&str> = Vec::with_capacity(3);
    if let Some(s) = year_str.as_deref() {
        disambiguators.push(s);
    }
    if let Some(s) = volume_str.as_deref() {
        disambiguators.push(s);
    }
    if let Some(p) = publisher
        && !p.trim().is_empty()
    {
        disambiguators.push(p);
    }
    allocate_slug(
        name,
        &disambiguators,
        &SeriesSlugAllocator {
            db,
            excluding: None,
        },
    )
    .await
}

/// Allocate a unique slug for a user-owned page. Scope is per-user. Pass
/// `excluding = Some(page_id)` on rename so the page's existing slug
/// doesn't count as a conflict against itself.
pub async fn allocate_user_page_slug<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    name: &str,
    excluding: Option<Uuid>,
) -> Result<String, DbErr> {
    allocate_slug(
        name,
        &[],
        &UserPageSlugAllocator {
            db,
            user_id,
            excluding,
        },
    )
    .await
}

/// Allocate an issue slug unique within `series_id`. Source precedence is
/// `number_raw` → `title` → first 8 chars of the BLAKE3 id (always
/// non-empty since the id is the row's primary key).
pub async fn allocate_issue_slug<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    number_raw: Option<&str>,
    title: Option<&str>,
    id_hash: &str,
) -> Result<String, DbErr> {
    let base = number_raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| title.map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_owned)
        .unwrap_or_else(|| id_hash.chars().take(8).collect());
    allocate_slug(
        &base,
        &[],
        &IssueSlugAllocator {
            db,
            series_id,
            excluding: None,
        },
    )
    .await
}

/// Fetch every existing issue slug for `series_id` in a single query, ready
/// to be passed to [`allocate_issue_slug_in_set`] across a per-folder scan.
/// Replaces the per-archive `SELECT COUNT(*)` round-trip the DB-backed
/// allocator does (see `docs/dev/scanner-perf.md` finding F-2).
pub async fn fetch_issue_slugs_for_series<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<HashSet<String>, DbErr> {
    let rows: Vec<String> = issue::Entity::find()
        .select_only()
        .column(issue::Column::Slug)
        .filter(issue::Column::SeriesId.eq(series_id))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Sync, in-memory variant of [`allocate_issue_slug`]. Inserts the chosen
/// slug into `taken` so successive allocations within the same scan batch
/// see prior choices. Pre-load `taken` via [`fetch_issue_slugs_for_series`]
/// before the per-archive loop.
///
/// Inputs match `allocate_issue_slug` so call sites can swap with no
/// behavioural change. `Infallible` — sync HashSet operations don't fail.
pub fn allocate_issue_slug_in_set(
    taken: &mut HashSet<String>,
    number_raw: Option<&str>,
    title: Option<&str>,
    id_hash: &str,
) -> String {
    let base_input = number_raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| title.map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_owned)
        .unwrap_or_else(|| id_hash.chars().take(8).collect());
    let base = slugify_segment(&base_input);
    if taken.insert(base.clone()) {
        return base;
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::convert::Infallible;
    use std::sync::Mutex;

    /// In-memory allocator for unit tests. Treats anything in `taken` as
    /// already in use; everything else is free.
    struct MockAllocator {
        taken: Mutex<HashSet<String>>,
    }

    impl MockAllocator {
        fn new<I: IntoIterator<Item = &'static str>>(taken: I) -> Self {
            Self {
                taken: Mutex::new(taken.into_iter().map(str::to_owned).collect()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SlugAllocator for MockAllocator {
        type Error = Infallible;
        async fn is_taken(&self, candidate: &str) -> Result<bool, Infallible> {
            Ok(self.taken.lock().unwrap().contains(candidate))
        }
    }

    #[test]
    fn slugify_basic_kebab_case() {
        assert_eq!(slugify_segment("Spider-Man (2018)"), "spider-man-2018");
        assert_eq!(slugify_segment("X-Men: Blue"), "x-men-blue");
        assert_eq!(slugify_segment("  Saga  "), "saga");
    }

    #[test]
    fn slugify_unicode_transliterates() {
        // The `slug` crate transliterates Latin-extended via deunicode.
        assert_eq!(slugify_segment("Pokémon Adventures"), "pokemon-adventures");
        assert_eq!(slugify_segment("Æon Flux"), "aeon-flux");
    }

    #[test]
    fn slugify_empty_falls_back() {
        assert_eq!(slugify_segment(""), EMPTY_FALLBACK);
        assert_eq!(slugify_segment("   "), EMPTY_FALLBACK);
        assert_eq!(slugify_segment("!!!---!!!"), EMPTY_FALLBACK);
    }

    #[test]
    fn slugify_caps_long_input_at_max() {
        let long = "a".repeat(200);
        let s = slugify_segment(&long);
        assert!(s.len() <= MAX_BASE_LEN);
        assert!(s.starts_with("aaa"));
        // No trailing dash from a mid-token cut.
        assert!(!s.ends_with('-'));
    }

    #[tokio::test]
    async fn allocate_returns_base_when_free() {
        let alloc = MockAllocator::new([]);
        let s = allocate_slug("Saga", &[], &alloc).await.unwrap();
        assert_eq!(s, "saga");
    }

    #[tokio::test]
    async fn allocate_uses_disambiguator_on_collision() {
        let alloc = MockAllocator::new(["spider-man"]);
        let s = allocate_slug("Spider-Man", &["2018"], &alloc)
            .await
            .unwrap();
        assert_eq!(s, "spider-man-2018");
    }

    #[tokio::test]
    async fn allocate_skips_taken_disambiguator_then_numeric() {
        let alloc = MockAllocator::new(["spider-man", "spider-man-2018"]);
        let s = allocate_slug("Spider-Man", &["2018"], &alloc)
            .await
            .unwrap();
        assert_eq!(s, "spider-man-2");
    }

    #[tokio::test]
    async fn allocate_falls_back_to_numeric_when_no_disambiguators() {
        let alloc = MockAllocator::new(["main", "main-2", "main-3"]);
        let s = allocate_slug("Main", &[], &alloc).await.unwrap();
        assert_eq!(s, "main-4");
    }

    #[tokio::test]
    async fn allocate_skips_empty_disambiguator() {
        let alloc = MockAllocator::new(["spider-man"]);
        // Empty disambiguator (e.g., publisher missing) must not produce
        // `spider-man-untitled`.
        let s = allocate_slug("Spider-Man", &["", "2018"], &alloc)
            .await
            .unwrap();
        assert_eq!(s, "spider-man-2018");
    }

    #[tokio::test]
    async fn allocate_normalizes_disambiguator_through_slugify() {
        let alloc = MockAllocator::new(["spider-man"]);
        let s = allocate_slug("Spider-Man", &["Image Comics"], &alloc)
            .await
            .unwrap();
        assert_eq!(s, "spider-man-image-comics");
    }

    // ───── allocate_issue_slug_in_set tests (F-2) ─────

    #[test]
    fn in_set_allocates_base_when_free() {
        let mut taken = HashSet::new();
        let s = allocate_issue_slug_in_set(&mut taken, Some("1"), None, "deadbeef");
        assert_eq!(s, "1");
        assert!(taken.contains("1"));
    }

    #[test]
    fn in_set_appends_numeric_suffix_on_collision() {
        let mut taken: HashSet<String> = ["1".into()].into_iter().collect();
        let s = allocate_issue_slug_in_set(&mut taken, Some("1"), None, "deadbeef");
        assert_eq!(s, "1-2");
        assert!(taken.contains("1-2"));
        let s2 = allocate_issue_slug_in_set(&mut taken, Some("1"), None, "cafebabe");
        assert_eq!(s2, "1-3");
    }

    #[test]
    fn in_set_falls_through_to_title_then_id_hash() {
        let mut taken = HashSet::new();
        let s = allocate_issue_slug_in_set(&mut taken, None, Some("Annual"), "deadbeefcafebabe");
        assert_eq!(s, "annual");
        let s2 = allocate_issue_slug_in_set(&mut taken, None, None, "f00ba12345678abc");
        // First 8 chars of the id, slugified.
        assert_eq!(s2, "f00ba123");
    }

    #[test]
    fn in_set_treats_pre_loaded_slugs_as_taken() {
        // Mirror the scanner flow: pre-fetch existing slugs, then allocate
        // for incoming issues. Pre-loaded ones must collide.
        let mut taken: HashSet<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let s = allocate_issue_slug_in_set(&mut taken, Some("1"), None, "x");
        assert_eq!(s, "1-2");
        let s = allocate_issue_slug_in_set(&mut taken, Some("4"), None, "x");
        assert_eq!(s, "4");
    }
}
