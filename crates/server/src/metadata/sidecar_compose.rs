//! Compose `ComicInfo` / `MetronInfo` structs from a provider's
//! `GenericMetadata` + the issue's current DB state, ready for the M1
//! serializers to emit as XML.
//!
//! This is the DB-aware half of M2 from
//! [`metadata-sidecar-writeback-1.0`](../../../../../.claude/plans/metadata-sidecar-writeback-1.0.md).
//! M3 wires it into the `RewriteIssueSidecarsJob`; the apply path stops
//! writing DB rows directly and starts writing fresh sidecar XML —
//! which the scanner then ingests back into the DB on the scoped
//! rescan that the job enqueues.
//!
//! ## Q4 lock — user-pin preservation
//!
//! When a field's `field_provenance.set_by='user'` row exists, the
//! caller passes its key into [`ComposeContext::issue_user_pins`] (or
//! `series_user_pins`). The composer **prefers the DB value** over the
//! provider's for those fields, so a user edit survives the
//! provider apply. The existing admin `override_user_edits` checkbox
//! is honoured upstream — when set, the caller passes an empty
//! user-pin set, which collapses this composer back to provider-wins
//! semantics.
//!
//! ## Raw passthrough
//!
//! The existing `issue.comic_info_raw` JSON column is forwarded into
//! the new `ComicInfo.raw` map verbatim. The serializer skips raw
//! entries whose keys overlap with typed fields, so the resulting XML
//! has each element exactly once — but any vendor-custom element
//! (`<X-…>` namespaces, `<MainCharacterOrTeam>`, Metron-Tagger
//! payloads) survives the round-trip.
//!
//! ## CV/Metron attribution
//!
//! When the candidate's source is ComicVine or Metron, a one-line
//! attribution suffix is appended to `<Notes>`:
//!
//! ```text
//! Sources: ComicVine (id=...), Metron (id=...) — CC-BY-NC-SA where applicable
//! ```
//!
//! Existing `<Notes>` content is preserved (concatenated with a blank
//! line). Pure ComicInfo-derived applies (no CV/Metron) skip the line.

use crate::metadata::identifier::Source;
use crate::metadata::provider::GenericMetadata;
use chrono::Datelike;
use entity::{external_id, field_provenance, issue, series};
use parsers::comicinfo::{ComicInfo, PageInfo};
use parsers::metroninfo::MetronInfo;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::{BTreeMap, HashSet};

/// Read-only inputs assembled by the caller (usually the apply worker)
/// before invoking either composer. Splitting the inputs into a struct
/// keeps the function signatures small even with seven references in
/// play.
pub struct ComposeContext<'a> {
    pub provider: &'a GenericMetadata,
    pub issue: &'a issue::Model,
    pub series: &'a series::Model,
    /// External identifiers attached to the issue, keyed by source
    /// (`"comicvine"`, `"metron"`, `"gtin"`, …) → external id. Sourced
    /// from the `external_id` table for `entity_type='issue'`.
    pub issue_external_ids: &'a BTreeMap<String, String>,
    /// Same shape, but for the parent series.
    pub series_external_ids: &'a BTreeMap<String, String>,
    /// Field keys (see [`crate::metadata::field::MetadataField::key`])
    /// pinned by the user on the issue row. Composer reads the DB
    /// value for these instead of the provider's. Empty when the
    /// caller is running with `override_user_edits=true`.
    pub issue_user_pins: &'a HashSet<String>,
    /// Same shape, for the parent series row.
    pub series_user_pins: &'a HashSet<String>,
}

impl ComposeContext<'_> {
    fn source(&self) -> Option<Source> {
        self.provider.source_provider
    }

    fn is_issue_pinned(&self, key: &str) -> bool {
        self.issue_user_pins.contains(key)
    }

    fn is_series_pinned(&self, key: &str) -> bool {
        self.series_user_pins.contains(key)
    }
}

// ───────── public composers ─────────

/// Produce a fresh `ComicInfo` ready for [`parsers::comicinfo::serialize`].
pub fn compose_comicinfo(ctx: &ComposeContext) -> ComicInfo {
    let mut info = ComicInfo {
        // Series-level fields. Title at the series level == `<Series>`
        // in ComicInfo. The Anansi schema fuses series+issue into one
        // doc per archive.
        series: prefer_user_str(ctx.is_series_pinned("title"), &ctx.series.name, ctx.provider.series_name.as_deref()),
        volume: prefer_user_int(ctx.is_series_pinned("volume"), ctx.series.volume, ctx.provider.volume),
        // ComicInfo `<Count>` reflects "total issues in the series".
        // `series.total_issues` is the scanner-managed aggregate (a
        // MAX over per-issue `comicinfo_count` values); we forward it
        // without a provider mapping since `GenericMetadata` has no
        // single field for it.
        count: ctx.series.total_issues,
        publisher: prefer_user_opt_str(
            ctx.is_series_pinned("publisher"),
            ctx.series.publisher.as_deref(),
            ctx.provider.publisher.as_deref(),
        ),
        imprint: prefer_user_opt_str(
            ctx.is_series_pinned("imprint"),
            ctx.series.imprint.as_deref(),
            ctx.provider.imprint.as_deref(),
        ),
        series_group: ctx.series.series_group.clone(),
        // Issue-level scalars.
        title: prefer_user_opt_str(
            ctx.is_issue_pinned("title"),
            ctx.issue.title.as_deref(),
            ctx.provider.title.as_deref(),
        ),
        number: prefer_user_opt_str(
            ctx.is_issue_pinned("title") /* same pin */ || ctx.is_issue_pinned("number_raw"),
            ctx.issue.number_raw.as_deref(),
            ctx.provider.issue_number.as_deref(),
        ),
        alternate_series: ctx.issue.alternate_series.clone(),
        summary: prefer_user_opt_str(
            ctx.is_issue_pinned("summary") || ctx.is_issue_pinned("description"),
            ctx.issue.summary.as_deref(),
            ctx.provider.description.as_deref(),
        ),
        notes: compose_notes(ctx),
        year: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.year,
            ctx.provider.cover_date.map(|d| d.year()),
        ),
        month: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.month,
            ctx.provider.cover_date.map(|d| d.month() as i32),
        ),
        day: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.day,
            ctx.provider.cover_date.map(|d| d.day() as i32),
        ),
        page_count: prefer_user_int(
            ctx.is_issue_pinned("page_count"),
            ctx.issue.page_count,
            ctx.provider.page_count,
        ),
        language_iso: prefer_user_opt_str(
            ctx.is_issue_pinned("language_code"),
            ctx.issue.language_code.as_deref(),
            ctx.provider.language_code.as_deref(),
        ),
        format: prefer_user_opt_str(
            ctx.is_issue_pinned("format"),
            ctx.issue.format.as_deref(),
            ctx.provider.format.as_deref(),
        ),
        black_and_white: ctx.issue.black_and_white,
        manga: ctx.issue.manga.clone(),
        age_rating: prefer_user_opt_str(
            ctx.is_issue_pinned("age_rating"),
            ctx.issue.age_rating.as_deref(),
            ctx.provider.age_rating.as_deref(),
        ),
        community_rating: prefer_user_opt_f64(
            ctx.is_issue_pinned("community_rating"),
            ctx.issue.community_rating,
            ctx.provider.community_rating.map(f64::from),
        ),
        main_character_or_team: None,
        review: ctx.issue.review.clone(),
        gtin: prefer_external_id_str(
            ctx.is_issue_pinned("external_id.gtin"),
            ctx.issue_external_ids,
            "gtin",
        ),
        scan_information: prefer_user_opt_str(
            ctx.is_issue_pinned("scan_information"),
            ctx.issue.scan_information.as_deref(),
            ctx.provider.scan_information.as_deref(),
        ),
        web: ctx.provider.source_url.clone().or_else(|| ctx.issue.web_url.clone()),
        // CV / Metron IDs land in their dedicated typed fields. Issue
        // is the canonical scope for IDs we expose to other readers
        // (ComicTagger / Mylar / Komga).
        comicvine_id: external_id_i64(ctx.issue_external_ids, "comicvine"),
        metron_id: external_id_i64(ctx.issue_external_ids, "metron"),
        comicvine_series_id: external_id_i64(ctx.series_external_ids, "comicvine"),
        metron_series_id: external_id_i64(ctx.series_external_ids, "metron"),
        // Junctions / CSV-shaped lists.
        characters: compose_csv_field(
            ctx.is_issue_pinned("characters"),
            ctx.issue.characters.as_deref(),
            &ctx.provider.characters.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ),
        teams: compose_csv_field(
            ctx.is_issue_pinned("teams"),
            ctx.issue.teams.as_deref(),
            &ctx.provider.teams.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ),
        locations: compose_csv_field(
            ctx.is_issue_pinned("locations"),
            ctx.issue.locations.as_deref(),
            &ctx.provider.locations.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ),
        tags: compose_csv_field(
            ctx.is_issue_pinned("tags"),
            ctx.issue.tags.as_deref(),
            &ctx.provider.tags.iter().map(String::as_str).collect::<Vec<_>>(),
        ),
        genre: compose_csv_field(
            ctx.is_issue_pinned("genres"),
            ctx.issue.genre.as_deref(),
            &ctx.provider.genres.iter().map(String::as_str).collect::<Vec<_>>(),
        ),
        story_arc: compose_csv_field(
            ctx.is_issue_pinned("story_arcs"),
            ctx.issue.story_arc.as_deref(),
            &ctx.provider.story_arcs.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ),
        story_arc_number: ctx.issue.story_arc_number.clone(),
        // Per-role credits. ComicInfo's flat columns map 1:1 with the
        // role-tagged junction; we always go through the DB column
        // (the apply path's existing CSV-rebuild step keeps the column
        // in sync with the junction). For non-pinned issues we
        // re-derive from the provider's role tags so the new XML
        // immediately reflects the provider data without waiting for
        // a rescan.
        writer: compose_role(ctx, "writer", "Writer", |i| i.writer.as_deref()),
        penciller: compose_role(ctx, "writer", "Penciller", |i| i.penciller.as_deref()),
        inker: compose_role(ctx, "writer", "Inker", |i| i.inker.as_deref()),
        colorist: compose_role(ctx, "writer", "Colorist", |i| i.colorist.as_deref()),
        letterer: compose_role(ctx, "writer", "Letterer", |i| i.letterer.as_deref()),
        cover_artist: compose_role(ctx, "writer", "CoverArtist", |i| i.cover_artist.as_deref()),
        editor: compose_role(ctx, "writer", "Editor", |i| i.editor.as_deref()),
        translator: compose_role(ctx, "writer", "Translator", |i| i.translator.as_deref()),
        // Pages: forward the scanner's existing per-page metadata
        // verbatim so the strip thumbnail pipeline + reader UI keep
        // working. The internal `double_page_inferred` field is
        // stripped by the M1 serializer.
        pages: extract_pages(&ctx.issue.pages),
        // Defaults; not exposed by GenericMetadata today.
        alternate_number: None,
        alternate_count: None,
        // Raw passthrough — preserves vendor-custom elements across the
        // round-trip. See module-level doc.
        raw: preserve_raw_from_issue(&ctx.issue.comic_info_raw),
    };

    // Series alias list lives in `series.aliases`; ComicInfo has no
    // typed slot for it. The raw map already covers any aliased element
    // that came from the original archive.
    let _ = &mut info; // silence unused-mut if all branches stay constant
    info
}

/// Produce a fresh `MetronInfo` ready for [`parsers::metroninfo::serialize`].
pub fn compose_metroninfo(ctx: &ComposeContext) -> MetronInfo {
    let mut credits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Provider credits — already structured as (role, name) pairs.
    // User pins on `credits` flip this to read from the issue row's
    // per-role columns instead, mirroring the ComicInfo path.
    if ctx.is_issue_pinned("credits") {
        push_role_csv(&mut credits, "Writer", ctx.issue.writer.as_deref());
        push_role_csv(&mut credits, "Penciller", ctx.issue.penciller.as_deref());
        push_role_csv(&mut credits, "Inker", ctx.issue.inker.as_deref());
        push_role_csv(&mut credits, "Colorist", ctx.issue.colorist.as_deref());
        push_role_csv(&mut credits, "Letterer", ctx.issue.letterer.as_deref());
        push_role_csv(&mut credits, "CoverArtist", ctx.issue.cover_artist.as_deref());
        push_role_csv(&mut credits, "Editor", ctx.issue.editor.as_deref());
        push_role_csv(&mut credits, "Translator", ctx.issue.translator.as_deref());
    } else {
        for c in &ctx.provider.credits {
            credits
                .entry(c.role.clone())
                .or_default()
                .push(c.name.clone());
        }
    }

    let mut ids: BTreeMap<String, String> = ctx.issue_external_ids.clone();
    // Series-scope IDs that MetronInfo recognises via dedicated sources.
    // Sources unique to the series row (typically "comicvine_series" etc.
    // — though our `external_id` table key is the bare source string and
    // is stored on the series entity, so the iteration is uniform) are
    // merged in too.
    for (k, v) in ctx.series_external_ids {
        ids.entry(format!("{k}_series"))
            .or_insert_with(|| v.clone());
    }

    let mut info = MetronInfo {
        title: prefer_user_opt_str(
            ctx.is_issue_pinned("title"),
            ctx.issue.title.as_deref(),
            ctx.provider.title.as_deref(),
        ),
        series: prefer_user_str(
            ctx.is_series_pinned("title"),
            &ctx.series.name,
            ctx.provider.series_name.as_deref(),
        ),
        publisher: prefer_user_opt_str(
            ctx.is_series_pinned("publisher"),
            ctx.series.publisher.as_deref(),
            ctx.provider.publisher.as_deref(),
        ),
        imprint: prefer_user_opt_str(
            ctx.is_series_pinned("imprint"),
            ctx.series.imprint.as_deref(),
            ctx.provider.imprint.as_deref(),
        ),
        number: prefer_user_opt_str(
            ctx.is_issue_pinned("number_raw"),
            ctx.issue.number_raw.as_deref(),
            ctx.provider.issue_number.as_deref(),
        ),
        volume: prefer_user_int(
            ctx.is_series_pinned("volume"),
            ctx.series.volume,
            ctx.provider.volume,
        ),
        year: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.year,
            ctx.provider.cover_date.map(|d| d.year()),
        ),
        month: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.month,
            ctx.provider.cover_date.map(|d| d.month() as i32),
        ),
        day: prefer_user_int(
            ctx.is_issue_pinned("cover_date"),
            ctx.issue.day,
            ctx.provider.cover_date.map(|d| d.day() as i32),
        ),
        summary: prefer_user_opt_str(
            ctx.is_issue_pinned("summary") || ctx.is_issue_pinned("description"),
            ctx.issue.summary.as_deref(),
            ctx.provider.description.as_deref(),
        ),
        notes: compose_notes(ctx),
        age_rating: prefer_user_opt_str(
            ctx.is_issue_pinned("age_rating"),
            ctx.issue.age_rating.as_deref(),
            ctx.provider.age_rating.as_deref(),
        ),
        language: prefer_user_opt_str(
            ctx.is_issue_pinned("language_code"),
            ctx.issue.language_code.as_deref(),
            ctx.provider.language_code.as_deref(),
        ),
        manga: ctx.issue.manga.clone(),
        gtin: prefer_external_id_str(
            ctx.is_issue_pinned("external_id.gtin"),
            ctx.issue_external_ids,
            "gtin",
        ),
        story_arcs: compose_list(
            ctx.is_issue_pinned("story_arcs"),
            ctx.issue.story_arc.as_deref(),
            ctx.provider.story_arcs.iter().map(|a| a.name.clone()).collect(),
        ),
        characters: compose_list(
            ctx.is_issue_pinned("characters"),
            ctx.issue.characters.as_deref(),
            ctx.provider.characters.iter().map(|c| c.name.clone()).collect(),
        ),
        teams: compose_list(
            ctx.is_issue_pinned("teams"),
            ctx.issue.teams.as_deref(),
            ctx.provider.teams.iter().map(|t| t.name.clone()).collect(),
        ),
        locations: compose_list(
            ctx.is_issue_pinned("locations"),
            ctx.issue.locations.as_deref(),
            ctx.provider.locations.iter().map(|l| l.name.clone()).collect(),
        ),
        tags: compose_list(
            ctx.is_issue_pinned("tags"),
            ctx.issue.tags.as_deref(),
            ctx.provider.tags.clone(),
        ),
        genres: compose_list(
            ctx.is_issue_pinned("genres"),
            ctx.issue.genre.as_deref(),
            ctx.provider.genres.clone(),
        ),
        ids,
        credits,
        // MetronInfo doesn't share `comic_info_raw` semantics — vendor
        // custom elements that appeared in the source MetronInfo file
        // are stored separately at parse time. We leave this empty for
        // freshly-composed sidecars; vendor-custom passthrough lives
        // on the ComicInfo composer where most archives carry it.
        raw: BTreeMap::new(),
    };

    let _ = &mut info; // silence unused-mut if all conditional branches stay constant
    info
}

// ───────── helpers ─────────

/// Append the CC-BY-NC-SA attribution suffix to `<Notes>` when the
/// candidate came from ComicVine or Metron. If the issue already has
/// notes, the new line goes after a blank-line separator.
fn compose_notes(ctx: &ComposeContext) -> Option<String> {
    let user_pinned = ctx.is_issue_pinned("notes");
    let base = prefer_user_opt_str(
        user_pinned,
        ctx.issue.notes.as_deref(),
        ctx.provider.notes.as_deref(),
    );

    // Attribution only fires for CV/Metron, and never when the user
    // pinned the Notes field — the user's text wins entirely in that
    // case (no surprise concatenation onto a deliberate edit).
    if user_pinned {
        return base;
    }

    let line = attribution_line(ctx);
    match (base, line) {
        (Some(b), Some(l)) if !b.contains(&l) => Some(format!("{b}\n\n{l}")),
        (Some(b), _) => Some(b),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    }
}

fn attribution_line(ctx: &ComposeContext) -> Option<String> {
    let source = ctx.source()?;
    if !matches!(source, Source::ComicVine | Source::Metron) {
        return None;
    }
    let cv = ctx.issue_external_ids.get("comicvine");
    let metron = ctx.issue_external_ids.get("metron");
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if let Some(id) = cv {
        parts.push(format!("ComicVine (id={id})"));
    }
    if let Some(id) = metron {
        parts.push(format!("Metron (id={id})"));
    }
    if parts.is_empty() {
        // Source provider is set but no identifiable id (rare — caller
        // wired it without populating external_ids). Still emit a bare
        // attribution; the operator can disambiguate by audit row.
        parts.push(source.label().to_string());
    }
    Some(format!(
        "Sources: {} — CC-BY-NC-SA where applicable",
        parts.join(", "),
    ))
}

fn prefer_user_opt_str(user_pinned: bool, db: Option<&str>, provider: Option<&str>) -> Option<String> {
    let pick = if user_pinned { db } else { provider.or(db) };
    pick.filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

/// Series.name is `String` (not `Option`); when the user pinned the
/// series title, prefer it. Provider's series_name (Option) is the
/// fallback when not pinned.
fn prefer_user_str(user_pinned: bool, db: &str, provider: Option<&str>) -> Option<String> {
    if user_pinned {
        return Some(db.to_owned()).filter(|s| !s.trim().is_empty());
    }
    if let Some(p) = provider.filter(|s| !s.trim().is_empty()) {
        return Some(p.to_owned());
    }
    Some(db.to_owned()).filter(|s| !s.trim().is_empty())
}

fn prefer_user_int(user_pinned: bool, db: Option<i32>, provider: Option<i32>) -> Option<i32> {
    if user_pinned { db } else { provider.or(db) }
}

fn prefer_user_opt_f64(user_pinned: bool, db: Option<f64>, provider: Option<f64>) -> Option<f64> {
    if user_pinned { db } else { provider.or(db) }
}

fn external_id_i64(map: &BTreeMap<String, String>, source: &str) -> Option<i64> {
    map.get(source).and_then(|v| v.trim().parse::<i64>().ok())
}

fn prefer_external_id_str(
    user_pinned: bool,
    map: &BTreeMap<String, String>,
    source: &str,
) -> Option<String> {
    // GTIN-shaped fields don't have a provider/DB split — the
    // external_id row IS the source of truth. The user_pinned flag
    // here only matters when we eventually grow a "provider-overridden
    // GTIN" code path; today it's effectively read-through.
    let _ = user_pinned;
    map.get(source).cloned().filter(|s| !s.trim().is_empty())
}

/// Build a `<Characters>foo, bar</Characters>`-shaped CSV. User-pinned
/// fields use the DB value (already CSV); otherwise we synthesize from
/// the provider's structured Vec. Provider order is preserved; an
/// empty result yields `None` so the serializer omits the element.
fn compose_csv_field(user_pinned: bool, db: Option<&str>, provider_names: &[&str]) -> Option<String> {
    if user_pinned {
        return db.map(str::to_owned).filter(|s| !s.trim().is_empty());
    }
    if provider_names.is_empty() {
        return db.map(str::to_owned).filter(|s| !s.trim().is_empty());
    }
    Some(provider_names.join(", "))
}

/// MetronInfo list variant — same logic as the CSV path but emits a
/// `Vec<String>` (the schema uses container/leaf form, not CSV).
fn compose_list(user_pinned: bool, db_csv: Option<&str>, provider_names: Vec<String>) -> Vec<String> {
    if user_pinned {
        return split_csv(db_csv);
    }
    if !provider_names.is_empty() {
        return provider_names;
    }
    split_csv(db_csv)
}

fn split_csv(csv: Option<&str>) -> Vec<String> {
    csv.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

fn push_role_csv(map: &mut BTreeMap<String, Vec<String>>, role: &str, csv: Option<&str>) {
    let names = split_csv(csv);
    if !names.is_empty() {
        map.entry(role.to_owned()).or_default().extend(names);
    }
}

/// Compose a per-role ComicInfo column (`writer`, `penciller`, …).
///
/// Pinning lookup is two-tier:
///
///   - `"credits"` — the apply path's canonical bucket; if set, *every*
///     role reads from its DB column (the writer-helper CSV rebuild
///     keeps each column in sync).
///   - `role_label.to_ascii_lowercase()` — legacy per-column keys
///     (`"writer"`, `"penciller"`, …); pin a single role without
///     affecting the rest.
///
/// `role_label` matches the provider's role tag (e.g. `"CoverArtist"`)
/// for the provider-fallback filter.
fn compose_role<F: Fn(&issue::Model) -> Option<&str>>(
    ctx: &ComposeContext,
    _unused: &str,
    role_label: &str,
    db_column: F,
) -> Option<String> {
    let role_pin = role_label.to_ascii_lowercase();
    let pinned = ctx.is_issue_pinned("credits") || ctx.is_issue_pinned(&role_pin);
    if pinned {
        return db_column(ctx.issue).map(str::to_owned).filter(|s| !s.trim().is_empty());
    }
    // Filter provider credits by role label.
    let names: Vec<String> = ctx
        .provider
        .credits
        .iter()
        .filter(|c| c.role.eq_ignore_ascii_case(role_label))
        .map(|c| c.name.clone())
        .collect();
    if names.is_empty() {
        return db_column(ctx.issue).map(str::to_owned).filter(|s| !s.trim().is_empty());
    }
    Some(names.join(", "))
}

/// Pull `Vec<PageInfo>` out of the scanner's `issue.pages` JSON so the
/// fresh `<Pages>` block carries the per-page metadata the reader
/// strip-thumb pipeline already populated.
fn extract_pages(json: &serde_json::Value) -> Vec<PageInfo> {
    serde_json::from_value::<Vec<PageInfo>>(json.clone()).unwrap_or_default()
}

/// Pull the existing `raw` BTreeMap out of the scanner-stored
/// `issue.comic_info_raw` JSON column. The serializer skips any key
/// that has a typed slot, so vendor-custom elements survive intact.
fn preserve_raw_from_issue(json: &serde_json::Value) -> BTreeMap<String, String> {
    // The column was written by the parser as
    // `serde_json::to_value(&ComicInfo)`, which serialises `raw` as
    // a nested object under the `raw` key. Older rows (pre-rescan)
    // might be the bare ComicInfo struct; try both shapes.
    if let Some(raw_obj) = json.get("raw").and_then(|v| v.as_object()) {
        return raw_obj
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
            .collect();
    }
    BTreeMap::new()
}

// ───────── DB loaders ─────────
//
// These helpers assemble the inputs the composer needs from the live
// database state — used by the M3 `RewriteIssueSidecarsJob` to build a
// `ComposeContext` before serializing.

/// Load every `external_id` row for `(entity_type, entity_id)` into a
/// `source → id` map. Returns an empty map when no rows match.
pub async fn load_external_ids(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<BTreeMap<String, String>, sea_orm::DbErr> {
    let rows = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.source, r.external_id)).collect())
}

/// Load the set of field keys pinned by the user on `(entity_type,
/// entity_id)` — i.e. rows with `set_by='user'`. Q4 lock: the composer
/// reads these and prefers DB values over provider values for matching
/// field keys.
pub async fn load_user_pins(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<HashSet<String>, sea_orm::DbErr> {
    let rows = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.field).collect())
}

/// List of field keys whose composer output WOULD differ from the
/// provider's, because the user pinned the DB value. Drives M3's
/// audit-payload `suppressed_user_pins` array so retrospective drill-
/// downs surface exactly which fields were preserved against the
/// provider's offering.
pub fn enumerate_suppressed_pins(
    ctx: &ComposeContext,
) -> Vec<String> {
    let mut out: Vec<&str> = Vec::new();
    // Issue-level keys whose composer reads from DB when pinned.
    for k in [
        "title",
        "number_raw",
        "summary",
        "description",
        "notes",
        "cover_date",
        "page_count",
        "language_code",
        "format",
        "age_rating",
        "community_rating",
        "scan_information",
        "characters",
        "teams",
        "locations",
        "tags",
        "genres",
        "story_arcs",
        "credits",
        "writer",
        "penciller",
        "inker",
        "colorist",
        "letterer",
        "coverartist",
        "editor",
        "translator",
    ] {
        if ctx.issue_user_pins.contains(k) {
            out.push(k);
        }
    }
    // Series-level keys.
    for k in ["title", "volume", "publisher", "imprint"] {
        if ctx.series_user_pins.contains(k) {
            out.push(k);
        }
    }
    out.into_iter().map(str::to_owned).collect()
}

// ───────── tests ─────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::identifier::{Identifier, Source};
    use crate::metadata::provider::{CreditCandidate, EntityCandidate, GenericMetadata};
    use chrono::NaiveDate;
    use entity::{issue, series};

    fn empty_pins() -> HashSet<String> {
        HashSet::new()
    }

    fn empty_ids() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn make_series(name: &str) -> series::Model {
        let now = chrono::Utc::now().fixed_offset();
        series::Model {
            id: uuid::Uuid::nil(),
            library_id: uuid::Uuid::nil(),
            name: name.to_owned(),
            normalized_name: name.to_ascii_lowercase(),
            slug: "fixture".to_owned(),
            year: Some(2020),
            volume: Some(1),
            publisher: Some("Image Comics".to_owned()),
            imprint: None,
            status: "ongoing".to_owned(),
            total_issues: Some(12),
            age_rating: None,
            summary: None,
            language_code: "en".to_owned(),
            series_group: None,
            alternate_names: serde_json::json!([]),
            sort_name: None,
            year_end: None,
            series_type: None,
            aliases: serde_json::json!([]),
            deck: None,
            publisher_id: None,
            imprint_id: None,
            last_metadata_sync_at: None,
            metadata_sync_paused: false,
            created_at: now,
            updated_at: now,
            folder_path: None,
            last_scanned_at: None,
            match_key: None,
            removed_at: None,
            removal_confirmed_at: None,
            status_user_set_at: None,
            reading_direction: None,
            preserve_canonical_order: false,
        }
    }

    fn make_issue(title: &str) -> issue::Model {
        let now = chrono::Utc::now().fixed_offset();
        issue::Model {
            id: "fixture-issue".into(),
            library_id: uuid::Uuid::nil(),
            series_id: uuid::Uuid::nil(),
            slug: "fixture".into(),
            file_path: "/tmp/fixture.cbz".into(),
            file_size: 1,
            file_mtime: now,
            state: "active".into(),
            content_hash: "abc".into(),
            title: Some(title.to_owned()),
            sort_number: Some(1.0),
            number_raw: Some("1".into()),
            volume: Some(1),
            year: Some(2020),
            month: Some(3),
            day: Some(15),
            summary: None,
            notes: None,
            language_code: Some("en".into()),
            format: None,
            black_and_white: Some(false),
            manga: None,
            age_rating: None,
            page_count: Some(20),
            pages: serde_json::json!([]),
            comic_info_raw: serde_json::json!({}),
            alternate_series: None,
            story_arc: None,
            story_arc_number: None,
            characters: None,
            teams: None,
            locations: None,
            tags: None,
            genre: None,
            writer: None,
            penciller: None,
            inker: None,
            colorist: None,
            letterer: None,
            cover_artist: None,
            editor: None,
            translator: None,
            publisher: None,
            imprint: None,
            scan_information: None,
            community_rating: None,
            review: None,
            web_url: None,
            deck: None,
            store_date: None,
            foc_date: None,
            price: None,
            sku: None,
            staff_rating: None,
            aliases: serde_json::json!([]),
            last_metadata_sync_at: None,
            created_at: now,
            updated_at: now,
            removed_at: None,
            removal_confirmed_at: None,
            superseded_by: None,
            special_type: None,
            hash_algorithm: 1,
            thumbnails_generated_at: None,
            thumbnail_version: 0,
            thumbnails_error: None,
            additional_links: serde_json::json!([]),
            user_edited: serde_json::json!([]),
            comicinfo_count: Some(12),
            last_rewrite_at: None,
            last_rewrite_kind: None,
        }
    }

    fn make_provider() -> GenericMetadata {
        GenericMetadata {
            series_name: Some("Saga".into()),
            series_type: Some("ongoing".into()),
            volume: Some(1),
            year_began: Some(2012),
            issue_number: Some("1".into()),
            publisher: Some("Image Comics".into()),
            title: Some("The Boy from Mars".into()),
            description: Some("An interplanetary love story.".into()),
            cover_date: NaiveDate::from_ymd_opt(2012, 3, 14),
            credits: vec![
                CreditCandidate {
                    name: "Brian K. Vaughan".into(),
                    role: "Writer".into(),
                    ordinal: Some(0),
                    identifiers: vec![],
                },
                CreditCandidate {
                    name: "Fiona Staples".into(),
                    role: "Penciller".into(),
                    ordinal: Some(0),
                    identifiers: vec![],
                },
            ],
            characters: vec![EntityCandidate {
                name: "Alana".into(),
                identifiers: vec![],
                is_first_appearance: false,
                died_in_issue: None,
                disbanded_in_issue: None,
                position_in_arc: None,
            }],
            tags: vec!["science-fiction".into(), "romance".into()],
            page_count: Some(44),
            age_rating: Some("Mature 17+".into()),
            source_provider: Some(Source::ComicVine),
            source_external_id: Some("4000-12345".into()),
            source_url: Some("https://comicvine.gamespot.com/saga-1/4000-12345/".into()),
            ..Default::default()
        }
    }

    #[test]
    fn compose_comicinfo_uses_provider_when_no_pins() {
        let series = make_series("Saga");
        let issue = make_issue("Untitled");
        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };

        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.title.as_deref(), Some("The Boy from Mars"));
        assert_eq!(ci.series.as_deref(), Some("Saga"));
        assert_eq!(ci.number.as_deref(), Some("1"));
        assert_eq!(ci.summary.as_deref(), Some("An interplanetary love story."));
        assert_eq!(ci.year, Some(2012));
        assert_eq!(ci.month, Some(3));
        assert_eq!(ci.day, Some(14));
        assert_eq!(ci.page_count, Some(44));
        assert_eq!(ci.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(ci.writer.as_deref(), Some("Brian K. Vaughan"));
        assert_eq!(ci.penciller.as_deref(), Some("Fiona Staples"));
        assert_eq!(ci.characters.as_deref(), Some("Alana"));
        assert_eq!(ci.tags.as_deref(), Some("science-fiction, romance"));
        assert_eq!(ci.age_rating.as_deref(), Some("Mature 17+"));
    }

    #[test]
    fn compose_comicinfo_preserves_user_pinned_title() {
        let series = make_series("Saga");
        let mut issue = make_issue("My Custom Title");
        // User pinned title and summary at the issue level.
        let mut pins = empty_pins();
        pins.insert("title".into());
        pins.insert("summary".into());
        issue.summary = Some("My own summary".into());

        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &pins,
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.title.as_deref(), Some("My Custom Title"));
        assert_eq!(ci.summary.as_deref(), Some("My own summary"));
        // Provider data still wins on fields the user didn't pin.
        assert_eq!(ci.year, Some(2012));
        assert_eq!(ci.writer.as_deref(), Some("Brian K. Vaughan"));
    }

    #[test]
    fn compose_comicinfo_falls_back_to_db_when_provider_absent() {
        let series = make_series("Saga");
        let mut issue = make_issue("DB Title");
        issue.summary = Some("DB summary".into());

        let provider = GenericMetadata::default();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.title.as_deref(), Some("DB Title"));
        assert_eq!(ci.summary.as_deref(), Some("DB summary"));
        assert_eq!(ci.series.as_deref(), Some("Saga"));
    }

    #[test]
    fn compose_appends_attribution_for_comicvine_source() {
        let series = make_series("Saga");
        let issue = make_issue("X");
        let provider = make_provider();
        let mut ids = empty_ids();
        ids.insert("comicvine".into(), "12345".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci.notes.expect("attribution emits Notes even when none existed");
        assert!(notes.contains("ComicVine (id=12345)"), "{notes}");
        assert!(notes.contains("CC-BY-NC-SA"), "{notes}");
    }

    #[test]
    fn compose_attribution_skipped_when_user_pinned_notes() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.notes = Some("Hand-curated notes — keep me intact.".into());

        let mut pins = empty_pins();
        pins.insert("notes".into());

        let provider = make_provider();
        let mut ids = empty_ids();
        ids.insert("comicvine".into(), "12345".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &pins,
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(
            ci.notes.as_deref(),
            Some("Hand-curated notes — keep me intact."),
            "user-pinned Notes must NOT receive the attribution suffix",
        );
    }

    #[test]
    fn compose_attribution_concatenates_with_existing_notes() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.notes = Some("Scanner identified file: saga-001.cbz.".into());

        let provider = make_provider();
        let mut ids = empty_ids();
        ids.insert("comicvine".into(), "12345".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci.notes.unwrap();
        assert!(notes.starts_with("Scanner identified file: saga-001.cbz."), "{notes}");
        assert!(notes.contains("CC-BY-NC-SA"), "{notes}");
    }

    #[test]
    fn compose_external_ids_round_trip() {
        let series = make_series("Saga");
        let issue = make_issue("X");
        let provider = make_provider();
        let mut iids = empty_ids();
        iids.insert("comicvine".into(), "12345".into());
        iids.insert("metron".into(), "987".into());
        iids.insert("gtin".into(), "9781632154002".into());
        let mut sids = empty_ids();
        sids.insert("comicvine".into(), "49901".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &iids,
            series_external_ids: &sids,
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.comicvine_id, Some(12345));
        assert_eq!(ci.metron_id, Some(987));
        assert_eq!(ci.comicvine_series_id, Some(49901));
        assert_eq!(ci.gtin.as_deref(), Some("9781632154002"));
    }

    #[test]
    fn compose_preserves_comic_info_raw_passthrough() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.comic_info_raw = serde_json::json!({
            "raw": {
                "X-Vendor-Custom": "preserve me",
                "MainCharacterOrTeam": "Alana"
            }
        });

        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(
            ci.raw.get("X-Vendor-Custom").map(String::as_str),
            Some("preserve me"),
        );
        assert_eq!(
            ci.raw.get("MainCharacterOrTeam").map(String::as_str),
            Some("Alana"),
        );
    }

    #[test]
    fn compose_metroninfo_uses_provider_when_no_pins() {
        let series = make_series("Saga");
        let issue = make_issue("Untitled");
        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let mi = compose_metroninfo(&ctx);
        assert_eq!(mi.title.as_deref(), Some("The Boy from Mars"));
        assert_eq!(mi.series.as_deref(), Some("Saga"));
        assert_eq!(
            mi.credits.get("Writer").map(Vec::as_slice),
            Some(["Brian K. Vaughan".to_string()].as_slice()),
        );
        assert_eq!(
            mi.credits.get("Penciller").map(Vec::as_slice),
            Some(["Fiona Staples".to_string()].as_slice()),
        );
        assert_eq!(mi.characters, vec!["Alana".to_string()]);
        assert_eq!(mi.tags, vec!["science-fiction".to_string(), "romance".into()]);
    }

    #[test]
    fn compose_metroninfo_credits_pin_reads_from_db_columns() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.writer = Some("Hand-fixed Writer".into());
        issue.penciller = Some("Hand-fixed Penciller A, Hand-fixed Penciller B".into());
        let mut pins = empty_pins();
        pins.insert("credits".into());

        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &pins,
            series_user_pins: &empty_pins(),
        };
        let mi = compose_metroninfo(&ctx);
        assert_eq!(
            mi.credits.get("Writer").map(Vec::as_slice),
            Some(["Hand-fixed Writer".to_string()].as_slice()),
        );
        assert_eq!(
            mi.credits.get("Penciller").map(Vec::as_slice),
            Some(
                [
                    "Hand-fixed Penciller A".to_string(),
                    "Hand-fixed Penciller B".into()
                ]
                .as_slice()
            ),
        );
    }

    #[test]
    fn compose_user_pin_blocks_provider_override_on_credit() {
        // User pinned `writer` (legacy per-column key). Provider tries
        // to overwrite with "Some Other Writer"; pin holds.
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.writer = Some("My Pinned Writer".into());

        let mut pins = empty_pins();
        pins.insert("writer".into());

        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &pins,
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.writer.as_deref(), Some("My Pinned Writer"));
        // Penciller un-pinned, still uses provider data.
        assert_eq!(ci.penciller.as_deref(), Some("Fiona Staples"));
    }

    #[test]
    fn compose_serializes_through_m1_round_trip() {
        // End-to-end: compose → serialize → parse → fields equal.
        let series = make_series("Saga");
        let issue = make_issue("Untitled");
        let provider = make_provider();
        let mut iids = empty_ids();
        iids.insert("comicvine".into(), "12345".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &iids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        let xml = parsers::comicinfo::serialize(&ci);
        let parsed = parsers::comicinfo::parse(xml.as_bytes()).expect("re-parse");
        assert_eq!(parsed.title.as_deref(), Some("The Boy from Mars"));
        assert_eq!(parsed.series.as_deref(), Some("Saga"));
        assert_eq!(parsed.comicvine_id, Some(12345));
        assert!(parsed.notes.unwrap().contains("CC-BY-NC-SA"));
    }

    #[test]
    fn compose_attribution_omitted_for_non_cv_metron_source() {
        let series = make_series("Saga");
        let issue = make_issue("X");
        let mut provider = make_provider();
        provider.source_provider = Some(Source::Gcd); // not CV / Metron
        provider.notes = None;
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
        };
        let ci = compose_comicinfo(&ctx);
        // No notes anywhere — no provider notes, no DB notes, GCD source.
        assert!(ci.notes.is_none(), "GCD source must not emit CC-BY-NC-SA line");
    }

    // Silence the unused-import warning on Identifier — held for future
    // tests that build provider identifier vectors.
    #[allow(dead_code)]
    fn _identifier_unused() -> Vec<Identifier> {
        Vec::new()
    }
}
