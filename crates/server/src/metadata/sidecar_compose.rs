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

use crate::library::scanner::metadata_rollup::split_csv;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::provider::{GenericMetadata, canonical_credit_role, canonicalize_role};
use chrono::Datelike;
use entity::{external_id, field_provenance, issue, series};
use parsers::comicinfo::{ComicInfo, PageInfo};
use parsers::metroninfo::MetronInfo;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, Statement,
};
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
    /// The issue's junction-table content (credits per role, characters,
    /// teams, locations, story arcs, genres, tags), loaded by
    /// [`load_issue_junctions`]. Junctions are the truth for every list
    /// field; the comma-joined read-cache columns on `issue` are only the
    /// fallback for a row whose junctions were never derived. Audit
    /// 2026-11 N-M2: reading the columns split a cached
    /// `"Mike Deodato, Jr."` into two creators in the written XML.
    pub issue_junctions: &'a IssueJunctions,
    /// The apply runs in `fill_missing` mode (audit 2026-11 N-M4). The
    /// caller has already blanked every provider value the issue has a
    /// value for ([`fill_missing_payload`]), so the composer's ordinary
    /// "provider, else DB" fallback keeps those; the one field that needs
    /// the flag itself is `<Notes>`, which a provider apply otherwise
    /// replaces with the Folio audit line whatever the issue carried.
    /// `false` for provider-wins applies and every DB-only compose.
    pub fill_missing: bool,
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

/// Overlay the provider's freshly-fetched identifiers onto the existing
/// DB external-id map so a writeback apply emits them into the composed
/// XML. Without this, the sidecar apply path silently drops new external
/// IDs: the composers read IDs only from the (pre-apply) DB map, so a
/// Metron/CV pull that introduces `metron` / `gcd` / `isbn` / `upc` rows
/// never reaches the MetronInfo `<IDs>` block and never round-trips back
/// via the scanner's ingest. The legacy DB-direct path handles this via
/// `apply::apply_external_ids`; this is its writeback-path equivalent.
///
/// Mirrors that path's precedence: provider values win for any source
/// the user hasn't pinned (`external_id.<source>`). Keyed by
/// [`Source::as_str`]. The **first** provider identifier per source wins
/// — ComicVine issue detail emits the issue id *before* the volume
/// (series) id under the same `comicvine` source, so first-wins keeps
/// the issue-scoped id and drops the series-scoped collision when
/// composing an issue's IDs.
pub fn merge_provider_identifiers(
    existing: &BTreeMap<String, String>,
    provider_ids: &[Identifier],
    user_pins: &HashSet<String>,
) -> BTreeMap<String, String> {
    let mut merged = existing.clone();
    let mut seen: HashSet<&str> = HashSet::new();
    for id in provider_ids {
        let key = id.source.as_str();
        if !seen.insert(key) {
            continue;
        }
        let trimmed = id.id.trim();
        if trimmed.is_empty() {
            continue;
        }
        if user_pins.contains(&format!("external_id.{key}")) {
            continue;
        }
        merged.insert(key.to_owned(), trimmed.to_owned());
    }
    merged
}

/// Project a **series-level** provider record down to the payload a
/// series-scope apply may compose into one of the series' issues.
///
/// The composer reads `provider.description` / `title` / dates / credits
/// / … as *issue* values, so handing it the series record verbatim made
/// a series apply stamp the series description into every issue's
/// `<Summary>` (and, via the rescan, `issues.summary`) — Fantastic Four
/// (2001), 173 issues. This is an allowlist so a field added to
/// [`GenericMetadata`] later is dropped (the issue keeps its own value)
/// until someone decides it is series-shaped:
///
/// - **series identity** (carried through): series name / sort name /
///   type / volume / years / aliases, publisher / imprint, identifiers,
///   and the source bookkeeping the composer's attribution line needs;
/// - **run-wide attributes** (fill-only): `format`, `language_code`,
///   `age_rating`, `genres` describe the whole run, so they fill an issue
///   that has none but never replace the issue's own value;
/// - **everything else is dropped**: description, deck, notes, title,
///   number, dates, page count, credits / characters / teams / locations /
///   arcs / tags / concepts / objects / universes, reprints, variants,
///   cover, ratings, price / sku, and `source_url` (the series page must
///   not replace the issue's `<Web>`). Those come only from an
///   issue-scope apply. The series description reaches `series.summary`
///   through `apply::write_series_scalar_fields`, not through the XML.
pub fn series_payload_for_issue(
    series: &GenericMetadata,
    issue: &issue::Model,
    junctions: &IssueJunctions,
) -> GenericMetadata {
    fn issue_lacks(v: Option<&str>) -> bool {
        v.is_none_or(|s| s.trim().is_empty())
    }
    // Genres are a junction: the issue has some when `issue_genres` has
    // rows, and only a never-derived row is judged by its column.
    let issue_lacks_genres = junctions.genres.is_empty() && issue_lacks(issue.genre.as_deref());
    GenericMetadata {
        series_name: series.series_name.clone(),
        series_sort_name: series.series_sort_name.clone(),
        series_type: series.series_type.clone(),
        series_external_id: series.series_external_id.clone(),
        volume: series.volume,
        year_began: series.year_began,
        year_end: series.year_end,
        aliases: series.aliases.clone(),
        publisher: series.publisher.clone(),
        imprint: series.imprint.clone(),
        identifiers: series.identifiers.clone(),
        format: series
            .format
            .clone()
            .filter(|_| issue_lacks(issue.format.as_deref())),
        language_code: series
            .language_code
            .clone()
            .filter(|_| issue_lacks(issue.language_code.as_deref())),
        age_rating: series
            .age_rating
            .clone()
            .filter(|_| issue_lacks(issue.age_rating.as_deref())),
        genres: if issue_lacks_genres {
            series.genres.clone()
        } else {
            Vec::new()
        },
        source_provider: series.source_provider,
        source_external_id: series.source_external_id.clone(),
        fetched_at: series.fetched_at,
        upstream_modified_at: series.upstream_modified_at,
        ..Default::default()
    }
}

/// The `fill_missing` apply mode on the sidecar path (audit 2026-11
/// N-M4, owner decision D-5): blank every provider value for a field the
/// issue already has, so the composer's "provider, else the issue's own
/// value" fallback keeps the current value and takes the provider's only
/// where the issue is empty — the DB-direct matrix (`apply::should_apply`)
/// applied to the payload. Because a masked field carries no provider
/// value, the apply's provenance loop (which skips provider-empty fields)
/// stamps provider rows only for the fields actually filled. `replace_all`
/// never calls this, so provider-wins output is unchanged.
///
/// "Has a value" is judged from what the composer would otherwise write
/// from the DB:
///
/// - **issue scalars** the composer writes (title, number, summary, store
///   date, page count, language, format, age rating, community rating,
///   scan information, web URL): the issue column is non-blank. The cover
///   date counts as present only when year, month and day form a valid
///   date — the test the DB-direct apply and the preview pane make, so a
///   year-only date is filled. Notes count when they are a value, not a
///   tagger trace ([`notes_are_a_value`]). Deck, FOC date, price and SKU
///   reach neither sidecar, so they are not masked.
/// - **series-shaped fields** (series name, sort name, type, start / end
///   year, volume, publisher, imprint): the series row's column, plus the
///   archive's own `<Volume>` for the volume (the composer carries it
///   through). The series name is never empty, so in this mode the
///   provider's series name never replaces the issue's `<Series>`.
/// - **junctions** (credits, characters, teams, locations, story arcs,
///   genres, tags, universes, reprints): the issue has junction rows; a
///   row whose junctions were never derived falls back to its read-cache
///   column, and such a column carrying tagger ids (`"John Doe [15487]"`)
///   counts as missing, as it does for the DB-direct apply.
///
/// Identifiers, variants and the primary cover are not values the
/// archive's sidecars hold and pass through untouched (external ids are
/// additive under their own conflict rule; covers follow the overwrite
/// policy).
pub fn fill_missing_payload(
    mut provider: GenericMetadata,
    issue: &issue::Model,
    series: &series::Model,
    junctions: &IssueJunctions,
) -> GenericMetadata {
    use crate::library::scanner::metadata_rollup::csv_has_tagger_id;
    fn has(v: Option<&str>) -> bool {
        v.is_some_and(|s| !s.trim().is_empty())
    }
    fn keep<T>(slot: &mut Option<T>, present: bool) {
        if present {
            *slot = None;
        }
    }
    fn keep_list<T>(list: &mut Vec<T>, present: bool) {
        if present {
            list.clear();
        }
    }
    // A never-derived row's read-cache column: non-blank and free of
    // tagger ids.
    let clean_column =
        |csv: Option<&str>| csv.is_some_and(|c| has(Some(c)) && !csv_has_tagger_id(c));
    let refs_present =
        |rows: &[JunctionRef], csv: Option<&str>| !rows.is_empty() || clean_column(csv);
    let values_present = |rows: &[String], csv: Option<&str>| !rows.is_empty() || clean_column(csv);

    // Issue scalars.
    keep(&mut provider.title, has(issue.title.as_deref()));
    keep(&mut provider.issue_number, has(issue.number_raw.as_deref()));
    keep(&mut provider.description, has(issue.summary.as_deref()));
    keep(&mut provider.notes, notes_are_a_value(issue));
    let full_cover_date = issue
        .year
        .zip(issue.month)
        .zip(issue.day)
        .and_then(|((y, m), d)| chrono::NaiveDate::from_ymd_opt(y, m as u32, d as u32));
    keep(&mut provider.cover_date, full_cover_date.is_some());
    keep(&mut provider.store_date, issue.store_date.is_some());
    keep(&mut provider.page_count, issue.page_count.is_some());
    keep(
        &mut provider.language_code,
        has(issue.language_code.as_deref()),
    );
    keep(&mut provider.format, has(issue.format.as_deref()));
    keep(&mut provider.age_rating, has(issue.age_rating.as_deref()));
    keep(
        &mut provider.community_rating,
        issue.community_rating.is_some(),
    );
    keep(
        &mut provider.scan_information,
        has(issue.scan_information.as_deref()),
    );
    // The composer writes the provider's page URL into `<Web>`.
    keep(&mut provider.source_url, has(issue.web_url.as_deref()));

    // Series-shaped fields the composer reads from the series row.
    keep(&mut provider.series_name, !series.name.trim().is_empty());
    keep(
        &mut provider.series_sort_name,
        has(series.sort_name.as_deref()),
    );
    keep(
        &mut provider.series_type,
        has(series.series_type.as_deref()),
    );
    keep(&mut provider.year_began, series.year.is_some());
    keep(&mut provider.year_end, series.year_end.is_some());
    keep(
        &mut provider.volume,
        series.volume.is_some()
            || comic_info_raw_i32(&issue.comic_info_raw, "volume", "Volume").is_some(),
    );
    keep(&mut provider.publisher, has(series.publisher.as_deref()));
    keep(&mut provider.imprint, has(series.imprint.as_deref()));

    // Junctions.
    let credit_columns = [
        issue.writer.as_deref(),
        issue.penciller.as_deref(),
        issue.inker.as_deref(),
        issue.colorist.as_deref(),
        issue.letterer.as_deref(),
        issue.cover_artist.as_deref(),
        issue.editor.as_deref(),
        issue.translator.as_deref(),
    ];
    let credits_present = !junctions.credits.is_empty()
        || (credit_columns.iter().any(|c| has(*c))
            && !credit_columns
                .iter()
                .any(|c| c.is_some_and(csv_has_tagger_id)));
    keep_list(&mut provider.credits, credits_present);
    keep_list(
        &mut provider.characters,
        refs_present(&junctions.characters, issue.characters.as_deref()),
    );
    keep_list(
        &mut provider.teams,
        refs_present(&junctions.teams, issue.teams.as_deref()),
    );
    keep_list(
        &mut provider.locations,
        refs_present(&junctions.locations, issue.locations.as_deref()),
    );
    keep_list(
        &mut provider.story_arcs,
        refs_present(&junctions.story_arcs, issue.story_arc.as_deref()),
    );
    keep_list(
        &mut provider.genres,
        values_present(&junctions.genres, issue.genre.as_deref()),
    );
    keep_list(
        &mut provider.tags,
        values_present(&junctions.tags, issue.tags.as_deref()),
    );
    // No read-cache column for these two: the junction is all there is.
    keep_list(&mut provider.universes, !junctions.universes.is_empty());
    keep_list(&mut provider.reprints, !junctions.reprints.is_empty());
    provider
}

// ───────── public composers ─────────

/// Produce a fresh `ComicInfo` ready for [`parsers::comicinfo::serialize`].
pub fn compose_comicinfo(ctx: &ComposeContext) -> ComicInfo {
    let junctions = ctx.issue_junctions;
    let (story_arc, story_arc_number) = compose_comicinfo_arcs(ctx);
    let mut info = ComicInfo {
        // Series-level fields. Title at the series level == `<Series>`
        // in ComicInfo. The Anansi schema fuses series+issue into one
        // doc per archive.
        series: series_name_for_issue(ctx),
        // The scanner rejects implausible volumes on ingest
        // (`filename::plausible_volume`): ComicTagger stamps a ComicVine
        // series' *start year* into `<Volume>` (`2021`), which Folio
        // keeps out of `series.volume`. That is a display decision, not a
        // licence to delete the element — with no series / provider value
        // and no user pin, carry the archive's own `<Volume>` through from
        // `comic_info_raw` (WP-6.4 ComicTagger parity).
        volume: prefer_user_int(
            ctx.is_series_pinned("volume"),
            ctx.series.volume,
            ctx.provider.volume,
        )
        .or_else(|| {
            if ctx.is_series_pinned("volume") {
                None
            } else {
                comic_info_raw_i32(&ctx.issue.comic_info_raw, "volume", "Volume")
            }
        }),
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
        // Same pin as MetronInfo's `<Number>`: the scanner merges the two
        // elements, so a title pin must not keep one and release the other.
        number: prefer_user_opt_str(
            ctx.is_issue_pinned("number_raw"),
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
        // Not modelled by GenericMetadata or any issue column, but the
        // serializer treats the name as typed (so it is NOT re-emitted
        // from `raw`) — carry the archive's existing value through from
        // `comic_info_raw` or a hand edit is deleted on apply (WP-2.6 (d),
        // audit DI-11).
        main_character_or_team: comic_info_raw_str(
            &ctx.issue.comic_info_raw,
            "main_character_or_team",
            "MainCharacterOrTeam",
        ),
        review: ctx.issue.review.clone(),
        gtin: comicinfo_gtin(ctx.issue_external_ids),
        scan_information: prefer_user_opt_str(
            ctx.is_issue_pinned("scan_information"),
            ctx.issue.scan_information.as_deref(),
            ctx.provider.scan_information.as_deref(),
        ),
        web: prefer_user_opt_str(
            ctx.is_issue_pinned("web_url"),
            ctx.issue.web_url.as_deref(),
            ctx.provider.source_url.as_deref(),
        ),
        // CV / Metron IDs land in their dedicated typed fields. Issue
        // is the canonical scope for IDs we expose to other readers
        // (ComicTagger / Mylar / Komga).
        comicvine_id: external_id_i64(ctx.issue_external_ids, "comicvine"),
        metron_id: external_id_i64(ctx.issue_external_ids, "metron"),
        comicvine_series_id: external_id_i64(ctx.series_external_ids, "comicvine"),
        metron_series_id: external_id_i64(ctx.series_external_ids, "metron"),
        // Read-only: a ComicTagger notes trace supplies it at scan time;
        // the composed sidecar carries GCD ids in MetronInfo instead.
        gcd_id: None,
        // Lists: the junction rows, joined the way the read-cache rebuild
        // joins them (`; ` when a name carries a comma), so a DB-only
        // compose reproduces the column byte for byte while a stale or
        // hand-edited column never reaches the file.
        characters: compose_csv_field(
            ctx.is_issue_pinned("characters"),
            &junctions.characters(),
            ctx.issue.characters.as_deref(),
            &ctx.provider
                .characters
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
        ),
        teams: compose_csv_field(
            ctx.is_issue_pinned("teams"),
            &junctions.teams(),
            ctx.issue.teams.as_deref(),
            &ctx.provider
                .teams
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
        ),
        locations: compose_csv_field(
            ctx.is_issue_pinned("locations"),
            &junctions.locations(),
            ctx.issue.locations.as_deref(),
            &ctx.provider
                .locations
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
        ),
        tags: compose_csv_field(
            ctx.is_issue_pinned("tags"),
            &junctions.tags(),
            ctx.issue.tags.as_deref(),
            &ctx.provider
                .tags
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
        genre: compose_csv_field(
            ctx.is_issue_pinned("genres"),
            &junctions.genres(),
            ctx.issue.genre.as_deref(),
            &ctx.provider
                .genres
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
        story_arc,
        story_arc_number,
        // Per-role credits: the provider's role-tagged credits when it
        // has any for the role and the user hasn't pinned credits, else
        // the `issue_credits` rows for that role.
        writer: compose_role(ctx, "Writer", |i| i.writer.as_deref()),
        penciller: compose_role(ctx, "Penciller", |i| i.penciller.as_deref()),
        inker: compose_role(ctx, "Inker", |i| i.inker.as_deref()),
        colorist: compose_role(ctx, "Colorist", |i| i.colorist.as_deref()),
        letterer: compose_role(ctx, "Letterer", |i| i.letterer.as_deref()),
        cover_artist: compose_role(ctx, "CoverArtist", |i| i.cover_artist.as_deref()),
        editor: compose_role(ctx, "Editor", |i| i.editor.as_deref()),
        translator: compose_role(ctx, "Translator", |i| i.translator.as_deref()),
        // Pages: forward the scanner's existing per-page metadata
        // verbatim so the strip thumbnail pipeline + reader UI keep
        // working. The internal `double_page_inferred` field is
        // stripped by the M1 serializer.
        pages: extract_pages(&ctx.issue.pages),
        // Not exposed by GenericMetadata; same carry-through as
        // `main_character_or_team` above — these are typed names on the
        // serializer, so a `None` here deletes the archive's value.
        alternate_number: comic_info_raw_str(
            &ctx.issue.comic_info_raw,
            "alternate_number",
            "AlternateNumber",
        ),
        alternate_count: comic_info_raw_i32(
            &ctx.issue.comic_info_raw,
            "alternate_count",
            "AlternateCount",
        ),
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
    let junctions = ctx.issue_junctions;
    let mut credits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Provider credits — already structured as (role, name) pairs.
    // User pins on `credits` flip this to read from the issue's
    // `issue_credits` rows instead, mirroring the ComicInfo path. So does
    // a provider with no credits at all (the DB-only compose of a manual
    // edit / drift flush): pre-WP-6.4 that path wrote a MetronInfo with
    // no `<Credits>` while ComicInfo (`compose_role`'s DB fallback) kept
    // them, so the two sidecars of one archive disagreed.
    if ctx.is_issue_pinned("credits") || ctx.provider.credits.is_empty() {
        credits = junctions.credits_by_role_label();
        if credits.is_empty() {
            // Legacy row whose junctions were never derived: the per-role
            // columns, split with the scanner's own splitter.
            push_role_csv(&mut credits, "Writer", ctx.issue.writer.as_deref());
            push_role_csv(&mut credits, "Penciller", ctx.issue.penciller.as_deref());
            push_role_csv(&mut credits, "Inker", ctx.issue.inker.as_deref());
            push_role_csv(&mut credits, "Colorist", ctx.issue.colorist.as_deref());
            push_role_csv(&mut credits, "Letterer", ctx.issue.letterer.as_deref());
            push_role_csv(
                &mut credits,
                "CoverArtist",
                ctx.issue.cover_artist.as_deref(),
            );
            push_role_csv(&mut credits, "Editor", ctx.issue.editor.as_deref());
            push_role_csv(&mut credits, "Translator", ctx.issue.translator.as_deref());
        }
    } else {
        for c in &ctx.provider.credits {
            credits
                .entry(c.role.clone())
                .or_default()
                .push(c.name.clone());
        }
    }

    // MetronInfo's `<IDs>` block is issue-scoped by spec — recognised
    // `source` values are the bare provider names (metron, comicvine,
    // gcd, marvel, locg, mu, isbn, upc, asin, doi). Series-scope IDs
    // do NOT belong here under any `_series` suffix (non-standard;
    // other readers ignore or warn). They live in the DB and surface
    // via ComicInfo's typed `<ComicVineSeriesID>` / `<MetronSeriesID>`
    // extension fields (which ComicTagger does recognise).
    let ids: BTreeMap<String, String> = ctx.issue_external_ids.clone();
    // `<ID primary="true">`: the provider this apply came from when it
    // supplied an issue id; else, for a DB-only compose, the first of
    // Metron / ComicVine / GCD present — Metron-Tagger re-fetches from
    // the primary source, so the pick is deterministic.
    let primary_id_source = ctx
        .source()
        .map(Source::as_str)
        .filter(|k| ids.contains_key(*k))
        .or_else(|| {
            ["metron", "comicvine", "gcd"]
                .into_iter()
                .find(|k| ids.contains_key(*k))
        })
        .map(str::to_owned);
    // `<Series id>` is the primary source's id for the series.
    let series_id = primary_id_source
        .as_deref()
        .and_then(|k| ctx.series_external_ids.get(k))
        .cloned();
    // `<Series><Format>`: the first label that maps onto the schema's
    // `formatType` — the provider's series type, the series row's, the
    // provider's format label, the issue's own `<Format>`.
    let series_format = [
        ctx.provider.series_type.as_deref(),
        ctx.series.series_type.as_deref(),
        ctx.provider.format.as_deref(),
        ctx.issue.format.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(parsers::metroninfo::metron_series_format)
    .map(str::to_owned);
    let story_arcs = compose_list(
        ctx.is_issue_pinned("story_arcs"),
        &junctions.story_arcs(),
        ctx.issue.story_arc.as_deref(),
        ctx.provider
            .story_arcs
            .iter()
            .map(|a| a.name.clone())
            .collect(),
    );
    // `<Arc><Number>` for whichever arc list won ([`arc_slot`]; the same
    // lookup feeds ComicInfo's `<StoryArcNumber>`). A file's non-integer
    // number has no `positiveInteger` slot and stays ComicInfo-only.
    let file_slots = file_arc_slots(ctx);
    let arc_numbers: BTreeMap<String, u32> = story_arcs
        .iter()
        .filter_map(|name| {
            let n = arc_slot(ctx, &file_slots, name)?.parse::<u32>().ok()?;
            Some((name.clone(), n))
        })
        .collect();
    let store_date = if ctx.is_issue_pinned("store_date") {
        ctx.issue.store_date
    } else {
        ctx.provider.store_date.or(ctx.issue.store_date)
    }
    .map(|d| d.format("%Y-%m-%d").to_string());
    let primary_url = prefer_user_opt_str(
        ctx.is_issue_pinned("web_url"),
        ctx.issue.web_url.as_deref(),
        ctx.provider.source_url.as_deref(),
    );
    let urls: Vec<String> = ctx
        .issue
        .additional_links
        .as_array()
        .map(|links| {
            links
                .iter()
                .filter_map(|l| l.get("url").and_then(|u| u.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let last_modified = ctx
        .provider
        .fetched_at
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .or_else(|| {
            ctx.issue
                .last_metadata_sync_at
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        });

    let mut info = MetronInfo {
        title: prefer_user_opt_str(
            ctx.is_issue_pinned("title"),
            ctx.issue.title.as_deref(),
            ctx.provider.title.as_deref(),
        ),
        series: series_name_for_issue(ctx),
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
        story_arcs,
        characters: compose_list(
            ctx.is_issue_pinned("characters"),
            &junctions.characters(),
            ctx.issue.characters.as_deref(),
            ctx.provider
                .characters
                .iter()
                .map(|c| c.name.clone())
                .collect(),
        ),
        teams: compose_list(
            ctx.is_issue_pinned("teams"),
            &junctions.teams(),
            ctx.issue.teams.as_deref(),
            ctx.provider.teams.iter().map(|t| t.name.clone()).collect(),
        ),
        locations: compose_list(
            ctx.is_issue_pinned("locations"),
            &junctions.locations(),
            ctx.issue.locations.as_deref(),
            ctx.provider
                .locations
                .iter()
                .map(|l| l.name.clone())
                .collect(),
        ),
        tags: compose_list(
            ctx.is_issue_pinned("tags"),
            &junctions.tags(),
            ctx.issue.tags.as_deref(),
            ctx.provider.tags.clone(),
        ),
        genres: compose_list(
            ctx.is_issue_pinned("genres"),
            &junctions.genres(),
            ctx.issue.genre.as_deref(),
            ctx.provider.genres.clone(),
        ),
        ids,
        credits,
        // ───── v1.2 schema fields (audit 2026-11 WP A4) ─────
        primary_id_source,
        series_id,
        series_sort_name: ctx
            .provider
            .series_sort_name
            .clone()
            .or_else(|| ctx.series.sort_name.clone())
            .filter(|s| !s.trim().is_empty()),
        series_format,
        series_start_year: ctx.provider.year_began.or(ctx.series.year),
        series_end_year: ctx.provider.year_end.or(ctx.series.year_end),
        series_status: Some(ctx.series.status.clone()).filter(|s| !s.trim().is_empty()),
        series_issue_count: ctx.series.total_issues,
        series_summary: ctx.series.summary.clone().filter(|s| !s.trim().is_empty()),
        store_date,
        page_count: prefer_user_int(
            ctx.is_issue_pinned("page_count"),
            ctx.issue.page_count,
            ctx.provider.page_count,
        ),
        arc_numbers,
        universes: compose_list(
            ctx.is_issue_pinned("universes"),
            &junctions.universes(),
            None,
            ctx.provider
                .universes
                .iter()
                .map(|u| u.name.clone())
                .collect(),
        ),
        reprints: compose_list(
            ctx.is_issue_pinned("reprints"),
            &junctions.reprints(),
            None,
            ctx.provider
                .reprints
                .iter()
                .map(|r| r.label.clone())
                .collect(),
        ),
        primary_url,
        urls,
        last_modified,
        // Raw passthrough, symmetrical to ComicInfo's: the scanner stores
        // the parsed MetronInfo (with its `raw` map of top-level elements
        // Folio doesn't model) in `issue.metron_info_raw`; forwarding it
        // here means a `<MangaVolume>` or vendor `<X-…>` element survives
        // the rewrite instead of being deleted (WP-2.6 (c), audit DI-11).
        // The serializer skips any key with a typed slot, so each element
        // appears exactly once.
        raw: preserve_raw_from_issue(ctx.issue.metron_info_raw.as_ref().unwrap_or(&NULL_JSON)),
    };

    let _ = &mut info; // silence unused-mut if all conditional branches stay constant
    info
}

// ───────── helpers ─────────

/// Build the `<Notes>` element value.
///
/// The composer treats `<Notes>` differently from other fields. Notes
/// is a free-form audit-trail field — most archives in the wild carry
/// a stale "Tagged with ComicTagger 1.3.5 on YYYY-MM-DD..." line from
/// whatever tool last wrote the sidecar. Preserving that line on a
/// fresh Folio write would be actively misleading: the file *was*
/// written by Folio, just now, from a CV/Metron pull — but the text
/// would still claim ComicTagger wrote it on a date weeks ago.
///
/// Behaviour:
///   - **User-pinned**: preserve `issue.notes` verbatim, drop the
///     attribution. The user's text wins absolutely.
///   - **No provider** (DB-only compose — manual edit, drift flush):
///     preserve `issue.notes`. Nothing was re-tagged, so the archive's
///     existing notes (e.g. ComicTagger's "Tagged with … [Issue ID n]"
///     line that Mylar3 reads) are still accurate; pre-WP-6.4 a hand
///     edit of any other field silently deleted them.
///   - **Fill-missing provider apply** with notes that are not a tagger
///     trace ([`notes_are_a_value`]): preserve them, like every other field
///     the issue already has. A trace is bookkeeping, not a value: keeping
///     it would leave a stale `[CVDB n]` beside the re-matched ids, so it is
///     replaced as below.
///   - **Provider apply**: drop `issue.notes` entirely (stale historical
///     tracer). Emit a fresh Folio audit-trail line; append provider
///     notes content (if any) below.
///
/// The Folio line is Mylar3-readable: `[CVDB<id>]` and `[MetronID<id>]`
/// tokens are bracket-suffixed so Mylar3's sync regex still pulls the
/// CV ID out of the notes element.
fn compose_notes(ctx: &ComposeContext) -> Option<String> {
    if ctx.is_issue_pinned("notes") {
        return ctx
            .issue
            .notes
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);
    }
    // DB-only compose: nothing was re-tagged. Fill-missing apply: notes
    // that are a value (not a tagger trace) are kept like every other
    // non-empty field (the provider's notes were masked by
    // `fill_missing_payload`); empty notes or a stale trace get the fresh
    // Folio audit line, as in provider-wins.
    let keep_own = ctx.source().is_none() || (ctx.fill_missing && notes_are_a_value(ctx.issue));
    if keep_own {
        return ctx
            .issue
            .notes
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);
    }

    let mut lines: Vec<String> = Vec::with_capacity(2);
    if let Some(audit) = folio_audit_line(ctx) {
        lines.push(audit);
    }
    if let Some(provider_notes) = ctx
        .provider
        .notes
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        lines.push(provider_notes.to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n\n"))
}

/// One-line audit-trail entry — when the source is CV or Metron, emits
/// a line in the canonical ComicTagger/Mylar3-readable shape with the
/// CC-BY-NC-SA attribution baked in. Other sources (GCD, ComicInfo-
/// derived applies) return `None`.
///
/// Format: `Tagged with Folio on YYYY-MM-DD HH:MM:SS — sources …
/// (CC-BY-NC-SA where applicable).  [CVDB<id>][MetronID<id>]`
fn folio_audit_line(ctx: &ComposeContext) -> Option<String> {
    let source = ctx.source()?;
    if !matches!(source, Source::ComicVine | Source::Metron) {
        return None;
    }
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
    let cv = ctx.issue_external_ids.get("comicvine");
    let metron = ctx.issue_external_ids.get("metron");

    let mut sources: Vec<String> = Vec::with_capacity(2);
    let mut tokens = String::new();
    if let Some(id) = cv {
        sources.push(format!("ComicVine (id={id})"));
        tokens.push_str(&format!("[CVDB{id}]"));
    }
    if let Some(id) = metron {
        sources.push(format!("Metron (id={id})"));
        tokens.push_str(&format!("[MetronID{id}]"));
    }
    if sources.is_empty() {
        // Source set on the candidate but no ID landed in the
        // external_id map yet (rare — caller didn't populate it). Bare
        // attribution; the audit-log row carries the actual identity.
        sources.push(source.label().to_string());
    }

    let body = format!(
        "Tagged with Folio on {timestamp} — sources {sources_clause} \
        (CC-BY-NC-SA where applicable).",
        sources_clause = sources.join(", "),
    );
    if tokens.is_empty() {
        Some(body)
    } else {
        Some(format!("{body}  {tokens}"))
    }
}

/// Whether the issue's `<Notes>` is text the fill-missing mode must keep:
/// non-blank and not a tagger's audit trace (`Tagged with …` from Folio or
/// ComicTagger, or one carrying tagger ids such as `[CVDB n]`). The trace
/// describes the last tagging run, so a new apply replaces it.
pub fn notes_are_a_value(issue: &issue::Model) -> bool {
    let Some(notes) = issue
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    let trace = notes.to_ascii_lowercase().contains("tagged with")
        || parsers::comicinfo::ids_from_tagger_notes(notes)
            != parsers::comicinfo::TaggerNoteIds::default();
    !trace
}

fn prefer_user_opt_str(
    user_pinned: bool,
    db: Option<&str>,
    provider: Option<&str>,
) -> Option<String> {
    let pick = if user_pinned { db } else { provider.or(db) };
    pick.filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

/// The `<Series>` one issue's sidecar carries: the user-pinned series
/// title; else the provider's series name (a series apply's main match,
/// or the splitter a `series_provider_range` overlays); else — when the
/// archive's own `<Series>` names a materially different series — that
/// tag; else the series row's name.
///
/// The archive fallback is what keeps a renamed continuation intact:
/// `Firestorm (2004)` holds #23–35 tagged `Firestorm: The Nuclear Man`,
/// which Metron and GCD file as their own series. A series edit or an
/// apply that carries no name for those issues used to stamp the folder's
/// name over the tag, and the rescan then erased the one signal coverage
/// analysis uses to find the continuation. A tag that is merely a
/// *variant* of the series name (`Flash` under `The Flash`, scanner
/// decorations) still normalises to the series name.
fn series_name_for_issue(ctx: &ComposeContext) -> Option<String> {
    if ctx.is_series_pinned("title") {
        return Some(ctx.series.name.clone()).filter(|s| !s.trim().is_empty());
    }
    if let Some(p) = ctx
        .provider
        .series_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(p.to_owned());
    }
    if let Some(own) = special_series_name(ctx.issue, &ctx.series.name) {
        return Some(own);
    }
    if let Some(own) = archive_series_name(ctx.issue)
        && names_a_different_series(&own, &ctx.series.name)
    {
        return Some(own);
    }
    Some(ctx.series.name.clone()).filter(|s| !s.trim().is_empty())
}

/// The series a special is filed under at the providers, when the
/// folder's series is only its host: the archive's own `<Series>` when
/// it names a different series; else, for an annual, `"<Series> Annual"`;
/// else the filename's series when *it* names a different one
/// (`Wonder Woman Secret Files _ Origins 001 (1998).cbz` under Wonder
/// Woman). `None` for an ordinary issue, and for a special whose own
/// identity is nowhere to be found — the host's name is then the best
/// there is. Feeds both the composer (so a series apply never writes the
/// host's name into a special that carried none) and the issue search
/// (`orchestrator::special_query_rewrite`).
pub fn special_series_name(issue: &issue::Model, series_name: &str) -> Option<String> {
    let special = issue
        .special_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if let Some(own) =
        archive_series_name(issue).filter(|n| names_a_different_series(n, series_name))
    {
        return Some(own);
    }
    let host = series_name.trim();
    match special {
        "Annual" => Some(if crate::metadata::title_norm::has_annual_token(host) {
            host.to_owned()
        } else {
            format!("{host} Annual")
        }),
        _ => {
            let leaf = std::path::Path::new(&issue.file_path)
                .file_name()?
                .to_str()?;
            let inferred = tidy_filename_series(&parsers::filename::infer(leaf).series);
            (!inferred.is_empty() && names_a_different_series(&inferred, host)).then_some(inferred)
        }
    }
}

/// `"Wonder Woman Secret Files _ Origins"` → `"Wonder Woman Secret Files
/// & Origins"`: a lone `_` is how a filename spells the `&` it can't carry.
fn tidy_filename_series(s: &str) -> String {
    s.split_whitespace()
        .map(|t| if t == "_" { "&" } else { t })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `<Series>` the archive itself carries (`comic_info_raw`), if any.
pub fn archive_series_name(issue: &issue::Model) -> Option<String> {
    comic_info_raw_str(&issue.comic_info_raw, "series", "Series")
}

/// Does `tag` name a materially different series than `name`? Compared
/// on the matcher's sanitized similarity with the coverage name floor, so
/// articles, punctuation and scanner decorations don't count as different
/// but a renamed continuation does.
pub fn names_a_different_series(tag: &str, name: &str) -> bool {
    let (tag, name) = (strip_bracket_groups(tag), strip_bracket_groups(name));
    crate::metadata::matcher::name_similarity(&tag, &name) < crate::metadata::coverage::NAME_FLOOR
}

/// `"The Flash (1987) [digital]"` → `"The Flash"`: a tagger's year /
/// scanner groups decorate the name, they don't change which series it is.
fn strip_bracket_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// ComicInfo `<GTIN>`: the issue's barcode from its external ids, in the
/// order `gtin` → `upc` → `isbn` (audit 2026-11 WP A8).
///
/// ComicInfo has one barcode field (a single EAN / UPC / ISBN) where
/// MetronInfo has `<GTIN><ISBN/><UPC/>`. A `gtin` row is that field's own
/// value (read from a file's ComicInfo, or typed by the user), so it
/// round-trips first. Otherwise the most specific barcode a provider gave
/// wins: the UPC with its 5-digit add-on names this issue and cover,
/// while an ISBN names the book edition. Spaces and hyphens are dropped
/// (`978-1-60706-601-9` → `9781607066019`) and the value stays a string,
/// so a leading zero survives. The scanner's ingest recognises the value
/// as the `upc` / `isbn` row's echo and does not mint a `gtin` row beside
/// it (`process::dedupe_barcodes`).
fn comicinfo_gtin(ids: &BTreeMap<String, String>) -> Option<String> {
    ["gtin", "upc", "isbn"]
        .into_iter()
        .filter_map(|k| ids.get(k))
        .map(|v| normalize_barcode(v))
        .find(|v| !v.is_empty())
}

/// A barcode with whitespace and hyphens removed and an ISBN-10 check
/// digit upper-cased: the spelling ComicInfo `<GTIN>` carries, and the
/// key the scanner compares barcodes of different sources by.
pub fn normalize_barcode(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase()
}

/// The provider's arc list is the one both sidecars write: the apply
/// carries arcs and the user has not pinned `story_arcs` (the rule
/// [`compose_csv_field`] / [`compose_list`] apply to the names).
fn provider_arcs_win(ctx: &ComposeContext) -> bool {
    !ctx.is_issue_pinned("story_arcs") && !ctx.provider.story_arcs.is_empty()
}

/// An arc's number within whichever arc list won: the provider's
/// `position_in_arc` first when its list won (a fresh apply beats a
/// position an older tag left in the junction), else the junction's, each
/// falling back to the other. One lookup for MetronInfo's `<Arc><Number>`
/// and ComicInfo's `<StoryArcNumber>`, so the two sidecars agree.
fn arc_number(ctx: &ComposeContext, name: &str) -> Option<i32> {
    let junction = || ctx.issue_junctions.arc_numbers.get(name).copied();
    let provider = || {
        ctx.provider
            .story_arcs
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| a.position_in_arc)
    };
    if provider_arcs_win(ctx) {
        provider().or_else(junction)
    } else {
        junction().or_else(provider)
    }
}

/// The file's own `<StoryArc>` / `<StoryArcNumber>` pairs when the file
/// owns the arcs ([`IssueJunctions::arcs_from_file`]) and the provider's
/// list is not the one written; else empty. Read from the parsed values
/// the scanner keeps in `comic_info_raw`: lower-cased arc name (as the
/// scanner's [`split_csv`] reads it, which is how the junction row was
/// named) → the number text in the same slot, verbatim (`1.5`, `3a`).
fn file_arc_slots(ctx: &ComposeContext) -> Vec<(String, String)> {
    if provider_arcs_win(ctx) || !ctx.issue_junctions.arcs_from_file {
        return Vec::new();
    }
    let raw = &ctx.issue.comic_info_raw;
    let (Some(names), Some(numbers)) = (
        comic_info_raw_str(raw, "story_arc", "StoryArc"),
        comic_info_raw_str(raw, "story_arc_number", "StoryArcNumber"),
    ) else {
        return Vec::new();
    };
    split_csv(&names)
        .into_iter()
        .zip(numbers.split([',', ';']).map(str::trim))
        .filter(|(_, n)| !n.is_empty())
        .map(|(name, n)| (name.trim().to_lowercase(), n.to_owned()))
        .collect()
}

/// One arc's number text: its position ([`arc_number`]), else the number
/// the file paired with it ([`file_arc_slots`]).
fn arc_slot(ctx: &ComposeContext, file_slots: &[(String, String)], name: &str) -> Option<String> {
    arc_number(ctx, name).map(|p| p.to_string()).or_else(|| {
        let key = name.trim().to_lowercase();
        file_slots
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, v)| v.clone())
    })
}

/// ComicInfo `<StoryArc>` + `<StoryArcNumber>` (audit 2026-11 WP A8).
///
/// The names come from the list that wins the usual way (the provider's
/// unless the user pinned `story_arcs`, else the `issue_arcs` junction in
/// `position_in_arc` order); the numbers are a **parallel** list in the
/// same order, joined with the same separator ([`csv_separator`]: `; `
/// once a name carries a comma) so slot N of one is slot N of the other.
/// An arc without a number leaves its slot empty (`1, , 3`); trailing
/// empty slots are dropped, and a list with no number at all omits the
/// element.
///
/// A slot's number is the arc's position ([`arc_number`]); for arcs the
/// file owns ([`IssueJunctions::arcs_from_file`]) an arc with no position
/// takes the number the file itself paired with it ([`file_arc_slots`]).
/// The scanner derives a position only for a single-arc issue and never
/// updates one, so a file's multi-arc `<StoryArcNumber>` lives there
/// alone, and a rewrite must not drop it or re-pair it to the junction's
/// order. A user edit (PATCH) likewise positions a single integer arc
/// only; with no position at all, a user-owned issue keeps the number
/// column it typed. The flat `story_arc` / `story_arc_number` columns are
/// otherwise read only when the issue has no junction rows (never
/// derived; verbatim), so a stale column cannot outlive a provider's
/// junction-only write.
fn compose_comicinfo_arcs(ctx: &ComposeContext) -> (Option<String>, Option<String>) {
    let junctions = ctx.issue_junctions;
    let column = |v: Option<&str>| v.map(str::to_owned).filter(|s| !s.trim().is_empty());
    let provider_won = provider_arcs_win(ctx);
    let names: Vec<&str> = if provider_won {
        ctx.provider
            .story_arcs
            .iter()
            .map(|a| a.name.as_str())
            .collect()
    } else if !junctions.story_arcs.is_empty() {
        junctions.story_arcs()
    } else {
        return (
            column(ctx.issue.story_arc.as_deref()),
            column(ctx.issue.story_arc_number.as_deref()),
        );
    };
    let file_slots = file_arc_slots(ctx);
    let mut slots: Vec<String> = names
        .iter()
        .map(|n| arc_slot(ctx, &file_slots, n).unwrap_or_default())
        .collect();
    while slots.last().is_some_and(String::is_empty) {
        slots.pop();
    }
    let story_arc = Some(join_unambiguous_csv(names.iter().copied()));
    if slots.is_empty() {
        let number = if ctx.is_issue_pinned("story_arcs") {
            column(ctx.issue.story_arc_number.as_deref())
        } else {
            None
        };
        return (story_arc, number);
    }
    (story_arc, Some(slots.join(csv_separator(&names))))
}

/// Build a `<Characters>foo, bar</Characters>`-shaped CSV. User-pinned
/// fields use the issue's junction rows; otherwise we synthesize from
/// the provider's structured Vec, falling back to the junction rows when
/// the provider has none. Order is preserved (the junction's ordinal, the
/// provider's sequence); an empty result yields `None` so the serializer
/// omits the element. `db` — the read-cache column — is read only for a
/// row with no junction rows at all (never derived), verbatim.
fn compose_csv_field(
    user_pinned: bool,
    junction: &[&str],
    db: Option<&str>,
    provider_names: &[&str],
) -> Option<String> {
    let from_db = || {
        if junction.is_empty() {
            return db.map(str::to_owned).filter(|s| !s.trim().is_empty());
        }
        Some(join_unambiguous_csv(junction.iter().copied()))
    };
    if user_pinned || provider_names.is_empty() {
        return from_db();
    }
    Some(join_unambiguous_csv(provider_names.iter().copied()))
}

/// Join names into a ComicInfo-shaped flat CSV. ComicInfo's `<Teams>` /
/// `<Characters>` / etc. are inherently a single string — names with
/// embedded commas (e.g. `"Capes, Inc."`) would round-trip to multiple
/// pieces under any `,`-only split. Folio's parser already splits on
/// both `,` and `;`, so when any name contains a comma we switch the
/// joiner to `; `. Readers that only split on `,` will see one
/// concatenated string for that field — same lossy outcome as before;
/// our own scanner now recovers the boundaries.
fn join_unambiguous_csv<'a, I: IntoIterator<Item = &'a str>>(names: I) -> String {
    let names: Vec<&str> = names.into_iter().collect();
    names.join(csv_separator(&names))
}

/// The joiner [`join_unambiguous_csv`] picks for `names`: `; ` once any
/// name carries a comma, else `, `. A list parallel to `names`
/// (`<StoryArcNumber>`) uses the same one so its slots line up.
fn csv_separator(names: &[&str]) -> &'static str {
    if names.iter().any(|n| n.contains(',')) {
        "; "
    } else {
        ", "
    }
}

/// MetronInfo list variant — same logic as the CSV path but emits a
/// `Vec<String>` (the schema uses container/leaf form, not CSV). The
/// never-derived fallback splits the column with the scanner's
/// [`split_csv`], so the file and the DB agree on what one name is.
fn compose_list(
    user_pinned: bool,
    junction: &[&str],
    db_csv: Option<&str>,
    provider_names: Vec<String>,
) -> Vec<String> {
    let from_db = || {
        if junction.is_empty() {
            return split_csv_opt(db_csv);
        }
        junction.iter().map(|s| (*s).to_owned()).collect()
    };
    if user_pinned || provider_names.is_empty() {
        return from_db();
    }
    provider_names
}

/// [`split_csv`] over a nullable column.
fn split_csv_opt(csv: Option<&str>) -> Vec<String> {
    csv.map(split_csv).unwrap_or_default()
}

fn push_role_csv(map: &mut BTreeMap<String, Vec<String>>, role: &str, csv: Option<&str>) {
    let names = split_csv_opt(csv);
    if !names.is_empty() {
        map.entry(role.to_owned()).or_default().extend(names);
    }
}

/// Compose a per-role ComicInfo column (`writer`, `penciller`, …).
///
/// Pinning lookup is two-tier:
///
///   - `"credits"` — the apply path's canonical bucket; if set, *every*
///     role reads from the issue's `issue_credits` rows.
///   - `role_label.to_ascii_lowercase()` — legacy per-column keys
///     (`"writer"`, `"penciller"`, …); pin a single role without
///     affecting the rest.
///
/// `role_label` is the ComicInfo spelling (`"CoverArtist"`): it names the
/// provider's role tag for the provider filter and, through
/// [`canonical_credit_role`], the junction's storage key. `db_column` — the
/// read-cache column — is read only for a row with no `issue_credits` rows
/// at all.
fn compose_role<F: Fn(&issue::Model) -> Option<&str>>(
    ctx: &ComposeContext,
    role_label: &str,
    db_column: F,
) -> Option<String> {
    let from_db = || {
        if ctx.issue_junctions.credits.is_empty() {
            return db_column(ctx.issue)
                .map(str::to_owned)
                .filter(|s| !s.trim().is_empty());
        }
        let names = ctx.issue_junctions.credits_for_role(role_label);
        (!names.is_empty()).then(|| join_unambiguous_csv(names))
    };
    let role_pin = role_label.to_ascii_lowercase();
    if ctx.is_issue_pinned("credits") || ctx.is_issue_pinned(&role_pin) {
        return from_db();
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
        return from_db();
    }
    Some(join_unambiguous_csv(names.iter().map(String::as_str)))
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

/// Stand-in for a NULL `metron_info_raw` column so the raw-map reader
/// can take a `&Value` uniformly (a NULL / non-object yields no entries).
const NULL_JSON: serde_json::Value = serde_json::Value::Null;

/// A string-valued ComicInfo element the composer has no column or
/// provider source for, read back from the scanner-stored
/// `issue.comic_info_raw` JSON. Prefers the typed struct key (`typed`,
/// e.g. `main_character_or_team`) the parser populated; falls back to
/// the element name in the `raw` map (`raw_key`, e.g.
/// `MainCharacterOrTeam`) for rows written before the typed field
/// existed. Empty strings count as absent.
fn comic_info_raw_str(json: &serde_json::Value, typed: &str, raw_key: &str) -> Option<String> {
    json.get(typed)
        .and_then(|v| v.as_str())
        .or_else(|| {
            json.get("raw")
                .and_then(|r| r.get(raw_key))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Integer sibling of [`comic_info_raw_str`]: the typed key holds a
/// number, the `raw` fallback holds the element's text.
fn comic_info_raw_i32(json: &serde_json::Value, typed: &str, raw_key: &str) -> Option<i32> {
    json.get(typed)
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .or_else(|| {
            json.get("raw")
                .and_then(|r| r.get(raw_key))
                .and_then(|v| v.as_str())
                .and_then(|s| s.trim().parse::<i32>().ok())
        })
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
    Ok(rows
        .into_iter()
        .map(|r| (r.source, r.external_id))
        .collect())
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

/// One `issue_credits` row: the storage role key (`writer`,
/// `cover_artist`, …), the creator's name (the `person` row's when linked,
/// else the junction's own text) and the person's landing-page slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunctionCredit {
    pub role: String,
    pub person: String,
    pub slug: Option<String>,
}

/// One cast / setting / arc row: the entity's name (the entity row's when
/// linked, else the junction's own text) and its landing-page slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunctionRef {
    pub name: String,
    pub slug: Option<String>,
}

/// An issue's junction-table content — the truth for every list field
/// (CLAUDE.md "Junctions are the truth, CSV columns are derived"). Read by
/// [`load_issue_junctions`] for the composer and the issue detail endpoint
/// alike, in the order the read-cache rebuild
/// (`writers::rebuild_issue_csv_cache`) joins the columns in, so joining a
/// list back reproduces the column byte for byte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueJunctions {
    /// Ordered by role, then the stored ordinal, then name.
    pub credits: Vec<JunctionCredit>,
    pub characters: Vec<JunctionRef>,
    pub teams: Vec<JunctionRef>,
    pub locations: Vec<JunctionRef>,
    /// Ordered by `position_in_arc` (nulls last), then name.
    pub story_arcs: Vec<JunctionRef>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    /// `issue_arcs.position_in_arc` by arc name — MetronInfo's
    /// `<Arc><Number>` (audit 2026-11 WP A4).
    pub arc_numbers: BTreeMap<String, i32>,
    /// `issue_universes` names, in name order.
    pub universes: Vec<String>,
    /// `issue_reprints` as display labels (the stored label, else
    /// `Series #N` of the linked local issue), in label order.
    pub reprints: Vec<String>,
    /// The issue's arcs are file-owned: no `story_arcs` provenance row
    /// above the file tier (the scanner's `arcs_file_owned` rule). Only
    /// then may an arc with no position take the number the file paired
    /// with it ([`file_arc_slots`]). `false` by default: the strict
    /// junction-only reading.
    pub arcs_from_file: bool,
}

impl IssueJunctions {
    fn names(refs: &[JunctionRef]) -> Vec<&str> {
        refs.iter().map(|r| r.name.as_str()).collect()
    }

    pub fn characters(&self) -> Vec<&str> {
        Self::names(&self.characters)
    }

    pub fn teams(&self) -> Vec<&str> {
        Self::names(&self.teams)
    }

    pub fn locations(&self) -> Vec<&str> {
        Self::names(&self.locations)
    }

    pub fn story_arcs(&self) -> Vec<&str> {
        Self::names(&self.story_arcs)
    }

    pub fn genres(&self) -> Vec<&str> {
        self.genres.iter().map(String::as_str).collect()
    }

    pub fn tags(&self) -> Vec<&str> {
        self.tags.iter().map(String::as_str).collect()
    }

    pub fn universes(&self) -> Vec<&str> {
        self.universes.iter().map(String::as_str).collect()
    }

    pub fn reprints(&self) -> Vec<&str> {
        self.reprints.iter().map(String::as_str).collect()
    }

    /// The creators credited under one role, in stored order. `role` may be
    /// spelled any way [`canonical_credit_role`] understands (`"Writer"`,
    /// `"CoverArtist"`, `"cover_artist"`).
    pub fn credits_for_role(&self, role: &str) -> Vec<&str> {
        let Some(key) = canonical_credit_role(role) else {
            return Vec::new();
        };
        self.credits
            .iter()
            .filter(|c| c.role == key)
            .map(|c| c.person.as_str())
            .collect()
    }

    /// Every credit grouped by role, keyed by the ComicInfo role label
    /// (`Writer`, `CoverArtist`, …; a role outside the ComicInfo set keeps
    /// its storage key), names in stored order — the shape
    /// [`MetronInfo::credits`] takes.
    pub fn credits_by_role_label(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in &self.credits {
            out.entry(comicinfo_role_label(&c.role))
                .or_default()
                .push(c.person.clone());
        }
        out
    }
}

/// `writer` → `Writer`, `cover_artist` → `CoverArtist`; a role with no
/// ComicInfo column (`journalist`) stays as stored.
fn comicinfo_role_label(key: &str) -> String {
    canonicalize_role(key).map_or_else(|| key.to_owned(), str::to_owned)
}

/// Load the issue's junction-table content (see [`IssueJunctions`]). A read
/// failure is an error, not an empty list: a composer handed empty lists
/// would write a file without them and the rescan would then drop the rows.
pub async fn load_issue_junctions(
    db: &DatabaseConnection,
    issue_id: &str,
) -> Result<IssueJunctions, sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct CreditRow {
        role: String,
        person: String,
        slug: Option<String>,
    }
    #[derive(Debug, FromQueryResult)]
    struct RefRow {
        name: String,
        slug: Option<String>,
    }
    #[derive(Debug, FromQueryResult)]
    struct ValueRow {
        value: String,
    }
    let backend = db.get_database_backend();
    let stmt = |sql: &str| Statement::from_sql_and_values(backend, sql, [issue_id.into()]);
    let refs = async |sql: &str| -> Result<Vec<JunctionRef>, sea_orm::DbErr> {
        Ok(RefRow::find_by_statement(stmt(sql))
            .all(db)
            .await?
            .into_iter()
            .map(|r| JunctionRef {
                name: r.name,
                slug: r.slug,
            })
            .collect())
    };
    let values = async |sql: &str| -> Result<Vec<String>, sea_orm::DbErr> {
        Ok(ValueRow::find_by_statement(stmt(sql))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    };

    // LEFT JOIN + COALESCE, as the read-cache rebuild does: a row whose
    // entity id the series rollup hasn't linked yet still contributes its
    // own stored name.
    let credits = async {
        Ok::<_, sea_orm::DbErr>(
            CreditRow::find_by_statement(stmt(
                "SELECT ic.role AS role, COALESCE(p.name, ic.person) AS person, p.slug AS slug \
                   FROM issue_credits ic LEFT JOIN person p ON p.id = ic.person_id \
                  WHERE ic.issue_id = $1 \
                  ORDER BY ic.role, ic.ordinal, COALESCE(p.name, ic.person)",
            ))
            .all(db)
            .await?
            .into_iter()
            .map(|r| JunctionCredit {
                role: r.role,
                person: r.person,
                slug: r.slug,
            })
            .collect::<Vec<_>>(),
        )
    };
    #[derive(Debug, FromQueryResult)]
    struct ArcRow {
        name: String,
        slug: Option<String>,
        position_in_arc: Option<i32>,
    }
    let arcs = ArcRow::find_by_statement(stmt(
        "SELECT sa.name AS name, sa.slug AS slug, ia.position_in_arc AS position_in_arc \
           FROM issue_arcs ia JOIN story_arc sa ON sa.id = ia.arc_id \
          WHERE ia.issue_id = $1 ORDER BY ia.position_in_arc NULLS LAST, sa.name",
    ))
    .all(db);
    // The scanner's own `arcs_file_owned` rule, so the composer's column
    // fallback and the rollup's reconcile agree on who owns the arcs.
    #[derive(Debug, FromQueryResult)]
    struct FlagRow {
        flag: bool,
    }
    static ARCS_FROM_FILE_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "SELECT {} AS flag",
            crate::library::scanner::metadata_rollup::arcs_file_owned("$1")
        )
    });
    let arcs_from_file = async {
        Ok::<_, sea_orm::DbErr>(
            FlagRow::find_by_statement(stmt(&ARCS_FROM_FILE_SQL))
                .one(db)
                .await?
                .is_some_and(|r| r.flag),
        )
    };
    // Ten independent reads; the series fan-out runs this per issue.
    let (
        credits,
        characters,
        teams,
        locations,
        arcs,
        genres,
        tags,
        universes,
        reprints,
        arcs_from_file,
    ) = tokio::try_join!(
        credits,
        refs(
            "SELECT COALESCE(c.name, ic.\"character\") AS name, c.slug AS slug \
                   FROM issue_characters ic LEFT JOIN character c ON c.id = ic.character_id \
                  WHERE ic.issue_id = $1 ORDER BY ic.ordinal, 1",
        ),
        refs(
            "SELECT COALESCE(t.name, it.team) AS name, t.slug AS slug \
                   FROM issue_teams it LEFT JOIN team t ON t.id = it.team_id \
                  WHERE it.issue_id = $1 ORDER BY it.ordinal, 1",
        ),
        refs(
            "SELECT COALESCE(l.name, il.location) AS name, l.slug AS slug \
                   FROM issue_locations il LEFT JOIN location l ON l.id = il.location_id \
                  WHERE il.issue_id = $1 ORDER BY il.ordinal, 1",
        ),
        arcs,
        values(
            "SELECT genre AS value FROM issue_genres WHERE issue_id = $1 ORDER BY ordinal, genre",
        ),
        values("SELECT tag AS value FROM issue_tags WHERE issue_id = $1 ORDER BY ordinal, tag"),
        values(
            "SELECT u.name AS value FROM issue_universes iu JOIN universe u ON u.id = iu.universe_id \
                  WHERE iu.issue_id = $1 ORDER BY u.name",
        ),
        // A label-only row keeps its label; a row linked to a local issue
        // is spelled `Series #N` from that issue (the label when both).
        values(
            "SELECT COALESCE(NULLIF(ir.reprinted_label, ''), \
                                s.name || ' #' || COALESCE(i.number_raw, '')) AS value \
                   FROM issue_reprints ir \
                   LEFT JOIN issues i ON i.id = ir.reprinted_issue_id \
                   LEFT JOIN series s ON s.id = i.series_id \
                  WHERE ir.issue_id = $1 \
                    AND (NULLIF(ir.reprinted_label, '') IS NOT NULL OR s.name IS NOT NULL) \
                  ORDER BY 1",
        ),
        arcs_from_file,
    )?;
    let arc_numbers = arcs
        .iter()
        .filter_map(|a| a.position_in_arc.map(|n| (a.name.clone(), n)))
        .collect();
    let story_arcs = arcs
        .into_iter()
        .map(|a| JunctionRef {
            name: a.name,
            slug: a.slug,
        })
        .collect();
    Ok(IssueJunctions {
        credits,
        characters,
        teams,
        locations,
        story_arcs,
        genres,
        tags,
        arc_numbers,
        universes,
        reprints,
        arcs_from_file,
    })
}

/// List of field keys whose composer output WOULD differ from the
/// provider's, because the user pinned the DB value. Drives M3's
/// audit-payload `suppressed_user_pins` array so retrospective drill-
/// downs surface exactly which fields were preserved against the
/// provider's offering.
pub fn enumerate_suppressed_pins(ctx: &ComposeContext) -> Vec<String> {
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
        "web_url",
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

    pub(super) fn make_series(name: &str) -> series::Model {
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
            series_json_present: None,
            created_at: now,
            updated_at: now,
            folder_path: None,
            last_scanned_at: None,
            match_key: None,
            removed_at: None,
            removal_confirmed_at: None,
            status_user_set_at: None,
            reading_direction: None,
            text_language: None,
            preserve_canonical_order: false,
        }
    }

    pub(super) fn make_issue(title: &str) -> issue::Model {
        let now = chrono::Utc::now().fixed_offset();
        issue::Model {
            id: "fixture-issue".into(),
            library_id: uuid::Uuid::nil(),
            series_id: uuid::Uuid::nil(),
            slug: "fixture".into(),
            file_path: "/tmp/fixture.cbz".into(),
            file_size: 1,
            file_mtime: now,
            state: entity::issue::IssueState::Active,
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
            metroninfo_present: None,
            hash_algorithm: 1,
            thumbnails_generated_at: None,
            thumbnail_version: 0,
            thumbnails_error: None,
            additional_links: serde_json::json!([]),
            comicinfo_count: Some(12),
            last_rewrite_at: None,
            last_rewrite_kind: None,
            last_sidecar_rewrite_at: None,
            metron_info_raw: None,
            cover_page_index: 0,
            metadata_review_accepted_at: None,
            metadata_review_accepted_by: None,
        }
    }

    fn special(file: &str, special_type: &str, archive_series: Option<&str>) -> issue::Model {
        let mut i = make_issue("");
        i.file_path = format!("/library/Wonder Woman (1987)/Specials/{file}");
        i.special_type = Some(special_type.into());
        if let Some(s) = archive_series {
            i.comic_info_raw = serde_json::json!({ "series": s });
        }
        i
    }

    #[test]
    fn special_series_name_finds_the_specials_own_series() {
        // An annual with no sidecar: the host's name plus "Annual".
        let annual = special("Wonder Woman Annual 007 (1998) (scan).cbz", "Annual", None);
        assert_eq!(
            special_series_name(&annual, "Wonder Woman").as_deref(),
            Some("Wonder Woman Annual")
        );
        // A special with no sidecar: the filename's series, `_` read as `&`.
        let sf = special(
            "Wonder Woman Secret Files _ Origins 003 (2002) (scan).cbz",
            "Special",
            None,
        );
        assert_eq!(
            special_series_name(&sf, "Wonder Woman").as_deref(),
            Some("Wonder Woman Secret Files & Origins")
        );
        // The archive's own `<Series>` wins when it names another series.
        let tagged = special(
            "Wonder Woman Secret Files _ Origins 001 (1998).cbz",
            "Special",
            Some("Wonder Woman: Secret Files & Origins"),
        );
        assert_eq!(
            special_series_name(&tagged, "Wonder Woman").as_deref(),
            Some("Wonder Woman: Secret Files & Origins")
        );
        // A one-shot whose filename only says the host's name: nothing better
        // than the host, so nothing.
        let bare = special("Wonder Woman 001 (1992).cbz", "OneShot", None);
        assert_eq!(special_series_name(&bare, "Wonder Woman"), None);
        // An ordinary issue is never rewritten.
        let mut plain = special("Wonder Woman V1987 003 (April 1987).cbz", "", None);
        plain.special_type = None;
        assert_eq!(special_series_name(&plain, "Wonder Woman"), None);
    }

    #[test]
    fn series_apply_composes_a_specials_own_series_not_the_hosts() {
        // The series-scope payload reaches a special with its identity
        // stripped (`apply::strip_series_identity`): the composed `<Series>`
        // must be the special's own, not the host run's.
        let series = make_series("Wonder Woman");
        let issue = special("Wonder Woman Annual 007 (1998) (scan).cbz", "Annual", None);
        let mut provider = make_provider();
        provider.series_name = None;
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(
            compose_comicinfo(&ctx).series.as_deref(),
            Some("Wonder Woman Annual")
        );
        assert_eq!(
            compose_metroninfo(&ctx).series.as_deref(),
            Some("Wonder Woman Annual")
        );
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci
            .notes
            .expect("attribution emits Notes even when none existed");
        // New shape: single Folio-tag line, Mylar3-readable [CVDBxxx] token.
        assert!(notes.starts_with("Tagged with Folio on "), "{notes}");
        assert!(notes.contains("ComicVine (id=12345)"), "{notes}");
        assert!(notes.contains("CC-BY-NC-SA"), "{notes}");
        assert!(
            notes.contains("[CVDB12345]"),
            "Mylar3 sync token missing: {notes}"
        );
    }

    #[test]
    fn compose_audit_line_includes_both_cv_and_metron_tokens() {
        let series = make_series("Saga");
        let issue = make_issue("X");
        let provider = make_provider();
        let mut ids = empty_ids();
        ids.insert("comicvine".into(), "12345".into());
        ids.insert("metron".into(), "987".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci.notes.unwrap();
        assert!(notes.contains("ComicVine (id=12345)"));
        assert!(notes.contains("Metron (id=987)"));
        assert!(notes.contains("[CVDB12345]"));
        assert!(notes.contains("[MetronID987]"));
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(
            ci.notes.as_deref(),
            Some("Hand-curated notes — keep me intact."),
            "user-pinned Notes must NOT receive the attribution suffix",
        );
    }

    #[test]
    fn compose_notes_drops_stale_historical_db_text_on_unpinned_path() {
        // Real-world case: a previous ComicTagger write left a "Tagged
        // with the ninjas.walk.alone fork of ComicTagger 1.3.5 on
        // 2026-05-17 21:44:24.  [CVDB1144240]" line in `issue.notes`.
        // When Folio composes a fresh sidecar from a provider pull, the
        // stale ComicTagger trace MUST be dropped — propagating it would
        // claim ComicTagger wrote a file Folio just wrote.
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.notes = Some(
            "Tagged with the ninjas.walk.alone fork of ComicTagger 1.3.5 \
             using info from Comic Vine on 2026-05-17 21:44:24.  [CVDB1144240]"
                .into(),
        );

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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci.notes.unwrap();
        assert!(
            !notes.contains("ComicTagger"),
            "stale ComicTagger trace must be dropped: {notes}",
        );
        assert!(
            !notes.contains("ninjas.walk.alone"),
            "stale tool fork must be dropped: {notes}",
        );
        // Folio's fresh attribution + CVDB token replace it.
        assert!(notes.starts_with("Tagged with Folio on "), "{notes}");
        assert!(notes.contains("[CVDB12345]"), "{notes}");
        // The historical [CVDB1144240] from the prior tagger MUST NOT
        // appear — the Folio-tag line carries the current CV id, not
        // the historical one.
        assert!(
            !notes.contains("[CVDB1144240]"),
            "stale historical CV id must be dropped: {notes}",
        );
    }

    #[test]
    fn compose_notes_appends_provider_notes_below_audit_line() {
        // When Metron returns editorial notes, append them below the
        // Folio-tag line with a blank-line separator. (Metron does have
        // a `notes` field in their schema; CV typically doesn't.)
        let series = make_series("Saga");
        let issue = make_issue("X");
        let mut provider = make_provider();
        provider.source_provider = Some(Source::Metron);
        provider.notes = Some("Editorial: variant covers by 5 artists.".into());

        let mut ids = empty_ids();
        ids.insert("metron".into(), "987".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        let notes = ci.notes.unwrap();
        let mut iter = notes.split("\n\n");
        assert!(iter.next().unwrap().starts_with("Tagged with Folio on "));
        assert_eq!(iter.next(), Some("Editorial: variant covers by 5 artists."),);
        assert!(
            iter.next().is_none(),
            "expected exactly two blocks: {notes}"
        );
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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

    /// WP-2.6 (d) / audit DI-11: `MainCharacterOrTeam`, `AlternateNumber`
    /// and `AlternateCount` are typed names on the serializer (so they are
    /// never re-emitted from `raw`) but have no column or provider source.
    /// Pre-fix the composer set them to `None`, which deleted the
    /// archive's hand-edited values on every apply. They must carry
    /// through from `comic_info_raw` — the typed struct keys the parser
    /// writes, or the `raw` element names for older rows — and land in
    /// the serialized XML exactly once.
    #[test]
    fn compose_carries_unmodelled_comicinfo_fields_through() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        // Shape written by the scanner: `serde_json::to_value(&ComicInfo)`.
        issue.comic_info_raw = serde_json::json!({
            "main_character_or_team": "Alana",
            "alternate_number": "12A",
            "alternate_count": 24,
            "raw": {
                "MainCharacterOrTeam": "Alana",
                "AlternateNumber": "12A",
                "AlternateCount": "24"
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.main_character_or_team.as_deref(), Some("Alana"));
        assert_eq!(ci.alternate_number.as_deref(), Some("12A"));
        assert_eq!(ci.alternate_count, Some(24));

        let xml = parsers::comicinfo::serialize(&ci);
        assert_eq!(xml.matches("<MainCharacterOrTeam>").count(), 1, "{xml}");
        assert!(
            xml.contains("<AlternateNumber>12A</AlternateNumber>"),
            "{xml}"
        );
        assert!(xml.contains("<AlternateCount>24</AlternateCount>"), "{xml}");

        // Older rows: only the `raw` map carries them (typed keys absent).
        let mut older = make_issue("X");
        older.comic_info_raw = serde_json::json!({
            "raw": { "MainCharacterOrTeam": "Marko", "AlternateCount": "7" }
        });
        let ctx = ComposeContext {
            issue: &older,
            ..ctx
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.main_character_or_team.as_deref(), Some("Marko"));
        assert_eq!(ci.alternate_number, None);
        assert_eq!(ci.alternate_count, Some(7));
    }

    /// WP-2.6 (c) / audit DI-11: top-level MetronInfo elements Folio
    /// doesn't model round-trip through `issue.metron_info_raw` into the
    /// composed XML instead of being deleted. Typed names are not
    /// duplicated; a NULL column composes as before.
    #[test]
    fn compose_metroninfo_passes_unknown_elements_through() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        let parsed = parsers::metroninfo::parse(
            br#"<?xml version="1.0"?>
<MetronInfo>
  <Title>Old title</Title>
  <MangaVolume>3</MangaVolume>
  <X-Vendor-Custom>keep me</X-Vendor-Custom>
</MetronInfo>"#,
        )
        .unwrap();
        issue.metron_info_raw = Some(serde_json::to_value(&parsed).unwrap());
        let provider = make_provider();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let mi = compose_metroninfo(&ctx);
        assert_eq!(mi.raw.get("MangaVolume").map(String::as_str), Some("3"));
        assert_eq!(
            mi.raw.get("X-Vendor-Custom").map(String::as_str),
            Some("keep me")
        );
        let xml = parsers::metroninfo::serialize(&mi);
        assert!(xml.contains("<MangaVolume>3</MangaVolume>"), "{xml}");
        assert!(
            xml.contains("<X-Vendor-Custom>keep me</X-Vendor-Custom>"),
            "{xml}"
        );
        // The typed title comes from the composer (as the schema's
        // `<Stories><Story>`), not the stale pre-schema `<Title>` in raw.
        assert_eq!(xml.matches("<Story>").count(), 1, "{xml}");
        assert!(!xml.contains("<Title>"), "{xml}");
        assert!(!xml.contains("Old title"), "{xml}");

        // NULL column → no passthrough, no panic.
        let bare = make_issue("X");
        assert!(bare.metron_info_raw.is_none());
        let ctx = ComposeContext {
            issue: &bare,
            ..ctx
        };
        assert!(compose_metroninfo(&ctx).raw.is_empty());
    }

    #[test]
    fn compose_metroninfo_drops_series_ids_from_ids_map() {
        // MetronInfo's `<IDs>` block is issue-scoped by spec — no
        // recognised `*_series` source. Series-scope IDs live in DB +
        // (for ComicInfo) the typed `<ComicVineSeriesID>` slot. The
        // composer must NOT inject `_series`-suffixed entries.
        let series = make_series("Saga");
        let issue = make_issue("X");
        let provider = make_provider();
        let mut iids = empty_ids();
        iids.insert("comicvine".into(), "12345".into());
        let mut sids = empty_ids();
        sids.insert("comicvine".into(), "49901".into());
        sids.insert("gcd".into(), "229870".into());
        sids.insert("metron".into(), "13281".into());

        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &iids,
            series_external_ids: &sids,
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let mi = compose_metroninfo(&ctx);
        // Issue-level CV ID lands in MetronInfo.
        assert_eq!(mi.ids.get("comicvine").map(String::as_str), Some("12345"));
        // No `*_series` suffix anywhere — non-standard and confuses
        // other readers (Komga, Mylar3, KOReader).
        for key in mi.ids.keys() {
            assert!(
                !key.ends_with("_series"),
                "MetronInfo ids must not carry a _series suffix; got `{key}`",
            );
        }
        assert!(!mi.ids.contains_key("comicvine_series"));
        assert!(!mi.ids.contains_key("gcd_series"));
        assert!(!mi.ids.contains_key("metron_series"));
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
        assert_eq!(
            mi.tags,
            vec!["science-fiction".to_string(), "romance".into()]
        );
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
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
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        // No notes anywhere — no provider notes, no DB notes, GCD source.
        assert!(
            ci.notes.is_none(),
            "GCD source must not emit CC-BY-NC-SA line"
        );
    }

    #[test]
    fn merge_adds_new_provider_ids_and_refreshes_existing() {
        let mut existing = empty_ids();
        existing.insert("comicvine".into(), "321297".into());
        let provider = vec![
            Identifier::with_canonical_url(Source::Metron, "7997", "issue"),
            Identifier::with_canonical_url(Source::ComicVine, "321297", "issue"),
            Identifier::new(Source::Gcd, "111"),
            Identifier::new(Source::Isbn, "9781607066019"),
            Identifier::new(Source::Upc, "70985300621"),
        ];
        let merged = merge_provider_identifiers(&existing, &provider, &empty_pins());
        assert_eq!(merged.get("metron").map(String::as_str), Some("7997"));
        assert_eq!(merged.get("comicvine").map(String::as_str), Some("321297"));
        assert_eq!(merged.get("gcd").map(String::as_str), Some("111"));
        assert_eq!(
            merged.get("isbn").map(String::as_str),
            Some("9781607066019")
        );
        assert_eq!(merged.get("upc").map(String::as_str), Some("70985300621"));
    }

    #[test]
    fn merge_first_provider_id_per_source_wins_cv_issue_over_series() {
        // ComicVine issue detail emits the issue id before the volume
        // (series) id under the same `comicvine` source. First-wins keeps
        // the issue-scoped id out of the issue's external-id map.
        let provider = vec![
            Identifier::with_canonical_url(Source::ComicVine, "321297", "issue"),
            Identifier::with_canonical_url(Source::ComicVine, "18166", "series"),
        ];
        let merged = merge_provider_identifiers(&empty_ids(), &provider, &empty_pins());
        assert_eq!(merged.get("comicvine").map(String::as_str), Some("321297"));
    }

    #[test]
    fn merge_respects_user_pin_on_external_id() {
        let mut existing = empty_ids();
        existing.insert("comicvine".into(), "user-set".into());
        let mut pins = empty_pins();
        pins.insert("external_id.comicvine".into());
        let provider = vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "999",
            "issue",
        )];
        let merged = merge_provider_identifiers(&existing, &provider, &pins);
        // Pinned source keeps the user value; provider does not override.
        assert_eq!(
            merged.get("comicvine").map(String::as_str),
            Some("user-set")
        );
    }

    #[test]
    fn merge_skips_blank_ids() {
        let provider = vec![Identifier::new(Source::Metron, "   ")];
        let merged = merge_provider_identifiers(&empty_ids(), &provider, &empty_pins());
        assert!(merged.is_empty());
    }

    #[test]
    fn join_unambiguous_csv_uses_semicolon_when_a_name_has_a_comma() {
        // No commas → standard ", " joiner stays.
        assert_eq!(join_unambiguous_csv(["Alpha", "Beta"]), "Alpha, Beta");
        // "Capes, Inc." would shred under any ","-split — switch to "; ".
        assert_eq!(
            join_unambiguous_csv(["Capes, Inc.", "Comet Twins"]),
            "Capes, Inc.; Comet Twins",
        );
        // Round-trip through the rollup's split_csv recovers two pieces:
        // `split_csv` prefers `;` when present so embedded commas survive.
        let csv = join_unambiguous_csv(["Capes, Inc.", "Comet Twins"]);
        let parts = crate::library::scanner::metadata_rollup::split_csv(&csv);
        assert_eq!(
            parts,
            vec!["Capes, Inc.".to_owned(), "Comet Twins".to_owned()]
        );
    }

    #[test]
    fn compose_db_only_keeps_the_archive_notes() {
        // WP-6.4: a manual edit / drift flush composes with an empty
        // provider. Nothing was re-tagged, so ComicTagger's notes line
        // (which Mylar3 reads for the CV id) must survive the rewrite.
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.notes = Some(
            "Tagged with ComicTagger 1.5.5 using info from Comic Vine on \
             2026-05-17 21:44:24.  [Issue ID 1144240]"
                .into(),
        );
        let provider = GenericMetadata::default();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(compose_comicinfo(&ctx).notes, issue.notes);
        assert_eq!(compose_metroninfo(&ctx).notes, issue.notes);
    }

    #[test]
    fn compose_carries_a_year_shaped_volume_through_from_the_archive() {
        // ComicTagger writes a ComicVine series' start year into
        // `<Volume>`; the scanner keeps it out of `series.volume`
        // (`plausible_volume`), but the rewrite must not delete it.
        let mut series = make_series("Saga");
        series.volume = None;
        let mut issue = make_issue("X");
        issue.comic_info_raw = serde_json::json!({ "volume": 2021, "raw": { "Volume": "2021" } });
        let provider = GenericMetadata::default();
        let pins = empty_pins();
        let mut ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &pins,
            series_user_pins: &pins,
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(compose_comicinfo(&ctx).volume, Some(2021));

        // A plausible series volume (or a provider's) still wins.
        let mut with_volume = series.clone();
        with_volume.volume = Some(2);
        ctx.series = &with_volume;
        assert_eq!(compose_comicinfo(&ctx).volume, Some(2));

        // A user who cleared the series volume keeps it cleared.
        let pinned: HashSet<String> = ["volume".to_owned()].into();
        ctx.series = &series;
        ctx.series_user_pins = &pinned;
        assert_eq!(compose_comicinfo(&ctx).volume, None);
    }

    #[test]
    fn compose_metroninfo_db_only_keeps_the_issue_credits() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.writer = Some("Wanda Writer, Second Scribe".into());
        issue.editor = Some("Eddie Editor".into());
        let provider = GenericMetadata::default();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let mi = compose_metroninfo(&ctx);
        assert_eq!(mi.writer().as_deref(), Some("Wanda Writer, Second Scribe"));
        assert_eq!(mi.editor().as_deref(), Some("Eddie Editor"));
    }

    /// A series-scope apply hands each issue only the series-shaped part
    /// of the series record: the series description / title / URL / notes
    /// never reach issue slots, run-wide attributes only fill gaps.
    #[test]
    fn series_payload_for_issue_keeps_issue_slots() {
        let series = make_series("Saga");
        let mut issue = make_issue("Own title");
        issue.summary = Some("Own summary.".into());
        issue.genre = Some("Space Opera".into());
        issue.web_url = Some("https://example.com/issue-1".into());
        issue.language_code = None;
        let mut record = make_provider();
        record.notes = Some("Series notes.".into());
        record.source_url = Some("https://example.com/series".into());
        record.genres = vec!["Science Fiction".into()];
        record.language_code = Some("en".into());

        let p = series_payload_for_issue(&record, &issue, &IssueJunctions::default());
        assert_eq!(p.description, None);
        assert_eq!(p.title, None);
        assert_eq!(p.notes, None);
        assert_eq!(p.source_url, None);
        assert_eq!(p.cover_date, None);
        assert!(p.credits.is_empty() && p.characters.is_empty());
        assert!(p.genres.is_empty(), "issue has its own genres");
        assert_eq!(p.language_code.as_deref(), Some("en"), "fills a gap");
        assert_eq!(p.series_name.as_deref(), Some("Saga"));
        assert_eq!(p.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(p.source_provider, record.source_provider);

        let ids = empty_ids();
        let ctx = ComposeContext {
            provider: &p,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.summary.as_deref(), Some("Own summary."));
        assert_eq!(ci.title.as_deref(), Some("Own title"));
        assert_eq!(ci.genre.as_deref(), Some("Space Opera"));
        assert_eq!(ci.web.as_deref(), Some("https://example.com/issue-1"));
        assert_eq!(ci.publisher.as_deref(), Some("Image Comics"));
        let mi = compose_metroninfo(&ctx);
        assert_eq!(mi.summary.as_deref(), Some("Own summary."));

        issue.genre = None;
        let p = series_payload_for_issue(&record, &issue, &IssueJunctions::default());
        assert_eq!(p.genres, vec!["Science Fiction".to_owned()]);
        // The junction, not the column, says whether the issue has genres.
        let with_rows = IssueJunctions {
            genres: vec!["Horror".into()],
            ..Default::default()
        };
        let p = series_payload_for_issue(&record, &issue, &with_rows);
        assert!(p.genres.is_empty(), "issue_genres rows block the fill");
    }
    // ───── junctions are the truth (audit 2026-11 N-M2) ─────

    fn credit(role: &str, person: &str) -> JunctionCredit {
        JunctionCredit {
            role: role.into(),
            person: person.into(),
            slug: None,
        }
    }

    fn named(name: &str) -> JunctionRef {
        JunctionRef {
            name: name.into(),
            slug: None,
        }
    }

    /// With junction rows present the read-cache columns are never read:
    /// a stale column loses to the junction in both sidecars, and a
    /// comma-bearing creator name stays one creator.
    #[test]
    fn compose_db_only_reads_the_junctions_not_the_csv_columns() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.writer = Some("Stale Writer".into());
        issue.penciller = Some("Mike Deodato, Jr.".into());
        issue.characters = Some("Stale Character".into());
        issue.genre = Some("Stale Genre".into());
        let junctions = IssueJunctions {
            credits: vec![
                credit("penciller", "Mike Deodato, Jr."),
                credit("penciller", "Pat Penciller"),
                credit("writer", "Wanda Writer"),
            ],
            characters: vec![named("Alana"), named("Marko")],
            genres: vec!["Science Fiction".into(), "Romance".into()],
            ..Default::default()
        };
        let provider = GenericMetadata::default();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &junctions,
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.writer.as_deref(), Some("Wanda Writer"));
        // Two pencillers, one with a comma: the `; ` joiner keeps the
        // boundary the scanner's splitter recovers.
        assert_eq!(
            ci.penciller.as_deref(),
            Some("Mike Deodato, Jr.; Pat Penciller")
        );
        assert_eq!(ci.characters.as_deref(), Some("Alana, Marko"));
        assert_eq!(ci.genre.as_deref(), Some("Science Fiction, Romance"));
        assert_eq!(ci.teams, None, "no junction rows and no column → omitted");

        let mi = compose_metroninfo(&ctx);
        assert_eq!(
            mi.credits.get("Penciller").map(Vec::as_slice),
            Some(["Mike Deodato, Jr.".to_string(), "Pat Penciller".into()].as_slice())
        );
        assert_eq!(
            mi.credits.get("Writer").map(Vec::as_slice),
            Some(["Wanda Writer".to_string()].as_slice())
        );
        assert_eq!(mi.characters, ["Alana", "Marko"]);
        assert_eq!(mi.genres, ["Science Fiction", "Romance"]);
        let xml = parsers::metroninfo::serialize(&mi);
        assert_eq!(xml.matches("<Credit>").count(), 3, "{xml}");
        assert!(!xml.contains("<Creator>Jr.</Creator>"), "{xml}");
    }

    /// A user pin on `credits` reads the junction too — the pin says
    /// "keep the DB's value", and the junction is the DB's value.
    #[test]
    fn compose_credits_pin_reads_the_junction_over_the_provider() {
        let series = make_series("Saga");
        let issue = make_issue("X");
        let junctions = IssueJunctions {
            credits: vec![credit("writer", "Hand-fixed Writer")],
            ..Default::default()
        };
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
            issue_junctions: &junctions,
            fill_missing: false,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.writer.as_deref(), Some("Hand-fixed Writer"));
        assert_eq!(ci.penciller, None, "pinned, and the junction has none");
        let mi = compose_metroninfo(&ctx);
        assert_eq!(mi.writer().as_deref(), Some("Hand-fixed Writer"));
        assert_eq!(mi.penciller(), None);
    }

    /// A row whose junctions were never derived falls back to the columns,
    /// split with the scanner's splitter: a comma-bearing suffix is one
    /// person there too.
    #[test]
    fn compose_without_junction_rows_splits_the_columns_like_the_scanner() {
        let series = make_series("Saga");
        let mut issue = make_issue("X");
        issue.inker = Some("J. P. Mayer, Mike Deodato, Jr.".into());
        issue.teams = Some("Capes, Inc.; Avengers".into());
        let provider = GenericMetadata::default();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &empty_ids(),
            series_external_ids: &empty_ids(),
            issue_user_pins: &empty_pins(),
            series_user_pins: &empty_pins(),
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        let mi = compose_metroninfo(&ctx);
        assert_eq!(
            mi.credits.get("Inker").map(Vec::as_slice),
            Some(["J. P. Mayer".to_string(), "Mike Deodato Jr.".into()].as_slice())
        );
        assert_eq!(mi.teams, ["Capes, Inc.", "Avengers"]);
        // ComicInfo keeps the column verbatim on this path.
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.inker.as_deref(), Some("J. P. Mayer, Mike Deodato, Jr."));
    }

    #[test]
    fn junction_credits_group_by_comicinfo_role_label() {
        let junctions = IssueJunctions {
            credits: vec![
                credit("cover_artist", "Cover Carl"),
                credit("journalist", "Jo"),
                credit("writer", "Wanda"),
            ],
            ..Default::default()
        };
        let by_role = junctions.credits_by_role_label();
        assert_eq!(
            by_role.keys().map(String::as_str).collect::<Vec<_>>(),
            ["CoverArtist", "Writer", "journalist"]
        );
        assert_eq!(junctions.credits_for_role("CoverArtist"), ["Cover Carl"]);
        assert_eq!(junctions.credits_for_role("cover_artist"), ["Cover Carl"]);
        assert!(junctions.credits_for_role("").is_empty());
    }

    /// `fill_missing_payload` (audit 2026-11 N-M4): the issue's and the
    /// series row's own values blank the provider's; gaps keep it. A
    /// year-only cover date and a never-derived credit column carrying
    /// tagger ids count as gaps, as on the DB-direct path.
    #[test]
    fn fill_missing_payload_masks_only_what_the_issue_has() {
        let series = make_series("Saga");
        let mut issue = make_issue("Own title");
        issue.summary = None;
        issue.month = None;
        issue.day = None;
        issue.writer = Some("John Doe [15487]".into());
        let junctions = IssueJunctions {
            characters: vec![named("Marko")],
            ..Default::default()
        };
        let masked = fill_missing_payload(make_provider(), &issue, &series, &junctions);

        // Kept: the issue / series already have these.
        assert_eq!(masked.title, None);
        assert_eq!(masked.issue_number, None);
        assert_eq!(masked.series_name, None);
        assert_eq!(masked.publisher, None, "series row has a publisher");
        assert_eq!(masked.volume, None, "series row has a volume");
        assert_eq!(masked.year_began, None, "series row has a year");
        assert_eq!(masked.page_count, None);
        assert!(masked.characters.is_empty(), "junction rows exist");
        // Filled: gaps.
        assert_eq!(
            masked.description.as_deref(),
            Some("An interplanetary love story.")
        );
        assert!(masked.cover_date.is_some(), "year-only date is a gap");
        assert_eq!(masked.credits.len(), 2, "tagger-id column is a gap");
        assert_eq!(masked.tags.len(), 2);
        assert_eq!(masked.age_rating.as_deref(), Some("Mature 17+"));
        assert_eq!(masked.series_type.as_deref(), Some("ongoing"));
        // Untouched: not archive values.
        assert_eq!(masked.source_provider, Some(Source::ComicVine));
        assert_eq!(masked.source_external_id.as_deref(), Some("4000-12345"));

        // Composed with the flag, the issue keeps its own notes.
        issue.notes = Some("Scanned by Somebody".into());
        let masked = fill_missing_payload(make_provider(), &issue, &series, &junctions);
        let (ids, pins) = (empty_ids(), empty_pins());
        let ctx = ComposeContext {
            provider: &masked,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &pins,
            series_user_pins: &pins,
            issue_junctions: &junctions,
            fill_missing: true,
        };
        let ci = compose_comicinfo(&ctx);
        assert_eq!(ci.notes.as_deref(), Some("Scanned by Somebody"));
        assert_eq!(ci.title.as_deref(), Some("Own title"));
        assert_eq!(ci.characters.as_deref(), Some("Marko"));
        assert_eq!(ci.writer.as_deref(), Some("Brian K. Vaughan"));
    }
}

#[cfg(test)]
mod archive_series_name_tests {
    use super::*;
    use std::collections::{BTreeMap, HashSet};

    fn ctx_parts() -> (series::Model, issue::Model) {
        let now = chrono::Utc::now().fixed_offset();
        let series = series::Model {
            id: uuid::Uuid::nil(),
            library_id: uuid::Uuid::nil(),
            name: "Firestorm".into(),
            normalized_name: "firestorm".into(),
            ..tests::make_series("Firestorm")
        };
        let mut issue = tests::make_issue("A Hawk in the Nest");
        issue.comic_info_raw =
            serde_json::json!({ "series": "Firestorm: The Nuclear Man", "number": "23" });
        let _ = now;
        (series, issue)
    }

    #[test]
    fn archive_tag_naming_another_series_survives_a_db_only_compose() {
        let (series, issue) = ctx_parts();
        let provider = GenericMetadata::default();
        let pins: HashSet<String> = HashSet::new();
        let ids: BTreeMap<String, String> = BTreeMap::new();
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &pins,
            series_user_pins: &pins,
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(
            compose_comicinfo(&ctx).series.as_deref(),
            Some("Firestorm: The Nuclear Man")
        );
        assert_eq!(
            compose_metroninfo(&ctx).series.as_deref(),
            Some("Firestorm: The Nuclear Man")
        );
    }

    #[test]
    fn provider_name_and_user_pin_still_win_and_variants_normalise() {
        let (series, mut issue) = ctx_parts();
        let pins: HashSet<String> = HashSet::new();
        let ids: BTreeMap<String, String> = BTreeMap::new();
        // A provider name (main match or range splitter) wins.
        let provider = GenericMetadata {
            series_name: Some("Firestorm: The Nuclear Man (Metron)".into()),
            ..Default::default()
        };
        let ctx = ComposeContext {
            provider: &provider,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &pins,
            series_user_pins: &pins,
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(
            compose_comicinfo(&ctx).series.as_deref(),
            Some("Firestorm: The Nuclear Man (Metron)")
        );
        // A user-pinned series title beats the archive tag.
        let none = GenericMetadata::default();
        let pinned: HashSet<String> = ["title".to_owned()].into();
        let ctx = ComposeContext {
            provider: &none,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &pins,
            series_user_pins: &pinned,
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(compose_comicinfo(&ctx).series.as_deref(), Some("Firestorm"));
        // A mere variant of the series name normalises to it.
        issue.comic_info_raw = serde_json::json!({ "series": "firestorm (2004)" });
        let ctx = ComposeContext {
            provider: &none,
            issue: &issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: &pins,
            series_user_pins: &pins,
            issue_junctions: &IssueJunctions::default(),
            fill_missing: false,
        };
        assert_eq!(compose_comicinfo(&ctx).series.as_deref(), Some("Firestorm"));
    }
}

/// ComicInfo `<GTIN>` + `<StoryArcNumber>` (audit 2026-11 WP A8).
#[cfg(test)]
mod comicinfo_gtin_arc_tests {
    use super::*;
    use crate::metadata::provider::{EntityCandidate, GenericMetadata};
    use std::collections::{BTreeMap, HashSet};

    fn named(name: &str) -> JunctionRef {
        JunctionRef {
            name: name.into(),
            slug: None,
        }
    }

    #[test]
    fn comicinfo_gtin_prefers_gtin_then_upc_then_isbn() {
        let ids = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        // UPC only: the provider's issue barcode reaches ComicInfo, leading
        // zero intact.
        assert_eq!(
            comicinfo_gtin(&ids(&[("upc", "07098530047000111")])).as_deref(),
            Some("07098530047000111")
        );
        // UPC beats ISBN; a `gtin` row beats both.
        assert_eq!(
            comicinfo_gtin(&ids(&[
                ("isbn", "9781607066019"),
                ("upc", "70985300470200111")
            ]))
            .as_deref(),
            Some("70985300470200111")
        );
        assert_eq!(
            comicinfo_gtin(&ids(&[
                ("gtin", "9781632154002"),
                ("isbn", "9781607066019"),
                ("upc", "70985300470200111"),
            ]))
            .as_deref(),
            Some("9781632154002")
        );
        // ISBN only, hyphenated and spaced: normalised, check digit upper.
        assert_eq!(
            comicinfo_gtin(&ids(&[("isbn", " 1-60706-601-x ")])).as_deref(),
            Some("160706601X")
        );
        // A blank `gtin` row does not hide the UPC.
        assert_eq!(
            comicinfo_gtin(&ids(&[("gtin", "  "), ("upc", "70985300470200111")])).as_deref(),
            Some("70985300470200111")
        );
        assert_eq!(comicinfo_gtin(&ids(&[("comicvine", "1")])), None);
    }

    fn arcs_ctx_compose(
        issue: &issue::Model,
        provider: &GenericMetadata,
        junctions: &IssueJunctions,
    ) -> (ComicInfo, MetronInfo) {
        arcs_ctx_compose_pinned(issue, provider, junctions, &HashSet::new())
    }

    fn arcs_ctx_compose_pinned(
        issue: &issue::Model,
        provider: &GenericMetadata,
        junctions: &IssueJunctions,
        pins: &HashSet<String>,
    ) -> (ComicInfo, MetronInfo) {
        let series = tests::make_series("Saga");
        let ids: BTreeMap<String, String> = BTreeMap::new();
        let none: HashSet<String> = HashSet::new();
        let ctx = ComposeContext {
            provider,
            issue,
            series: &series,
            issue_external_ids: &ids,
            series_external_ids: &ids,
            issue_user_pins: pins,
            series_user_pins: &none,
            issue_junctions: junctions,
            fill_missing: false,
        };
        (compose_comicinfo(&ctx), compose_metroninfo(&ctx))
    }

    fn arcs(rows: &[(&str, Option<i32>)]) -> IssueJunctions {
        IssueJunctions {
            story_arcs: rows.iter().map(|(n, _)| named(n)).collect(),
            arc_numbers: rows
                .iter()
                .filter_map(|(n, p)| p.map(|p| ((*n).to_owned(), p)))
                .collect(),
            ..Default::default()
        }
    }

    /// A junction-only arc write (provider apply, hand edit) leaves the flat
    /// columns empty; ComicInfo's parallel lists come from the junction, in
    /// the same order, and agree with MetronInfo's `<Arc><Number>`.
    #[test]
    fn story_arc_number_is_the_junction_positions_in_parallel() {
        let mut issue = tests::make_issue("X");
        issue.story_arc = Some("Stale Arc".into());
        issue.story_arc_number = Some("9".into());
        let none = GenericMetadata::default();

        let (ci, mi) = arcs_ctx_compose(
            &issue,
            &none,
            &arcs(&[("Volume One", Some(1)), ("The War", Some(3))]),
        );
        assert_eq!(ci.story_arc.as_deref(), Some("Volume One, The War"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("1, 3"));
        assert_eq!(mi.story_arcs, ["Volume One", "The War"]);
        assert_eq!(mi.arc_numbers.get("The War"), Some(&3));

        // A comma in a name switches both lists to `; `.
        let (ci, _) = arcs_ctx_compose(
            &issue,
            &none,
            &arcs(&[("Volume One", Some(1)), ("Blood, Sweat", Some(4))]),
        );
        assert_eq!(ci.story_arc.as_deref(), Some("Volume One; Blood, Sweat"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("1; 4"));

        // An unnumbered arc keeps its slot; trailing empties are dropped.
        let (ci, _) = arcs_ctx_compose(
            &issue,
            &none,
            &arcs(&[("A", Some(1)), ("B", None), ("C", Some(3)), ("D", None)]),
        );
        assert_eq!(ci.story_arc.as_deref(), Some("A, B, C, D"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("1, , 3"));

        // No positions on provider-owned arcs: no number, and the stale
        // column never reaches the file.
        let (ci, _) = arcs_ctx_compose(&issue, &none, &arcs(&[("Volume One", None)]));
        assert_eq!(ci.story_arc.as_deref(), Some("Volume One"));
        assert_eq!(ci.story_arc_number, None);
    }

    /// With no junction rows the flat columns pass through verbatim.
    /// For file-owned arcs an arc with no position takes the number the
    /// file paired with *that arc* (`comic_info_raw`), re-ordered with the
    /// junction's list rather than zipped against it.
    #[test]
    fn story_arc_number_takes_the_files_own_pairing_for_file_owned_arcs() {
        let mut issue = tests::make_issue("X");
        issue.story_arc = Some("Alpha, Zeta".into());
        issue.story_arc_number = Some("3, 7".into());
        issue.comic_info_raw = serde_json::json!({
            "story_arc": "Zeta, Alpha, Omega",
            "story_arc_number": "3, 7, 1.5",
        });
        let none = GenericMetadata::default();

        let (ci, _) = arcs_ctx_compose(&issue, &none, &IssueJunctions::default());
        assert_eq!(ci.story_arc.as_deref(), Some("Alpha, Zeta"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("3, 7"));

        // No positions (a multi-arc file): junction order is by name, the
        // numbers follow their arcs; a non-integer stays ComicInfo-only.
        let mut file_owned = arcs(&[("Alpha", None), ("Omega", None), ("Zeta", None)]);
        file_owned.arcs_from_file = true;
        let (ci, mi) = arcs_ctx_compose(&issue, &none, &file_owned);
        assert_eq!(ci.story_arc.as_deref(), Some("Alpha, Omega, Zeta"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("7, 1.5, 3"));
        assert_eq!(mi.arc_numbers.get("Alpha"), Some(&7));
        assert_eq!(mi.arc_numbers.get("Zeta"), Some(&3));
        assert!(!mi.arc_numbers.contains_key("Omega"));

        // A position kept from an earlier single-arc tag does not hide the
        // file's number for the arc added since.
        let mut file_owned = arcs(&[("Alpha", Some(9)), ("Zeta", None)]);
        file_owned.arcs_from_file = true;
        let (ci, _) = arcs_ctx_compose(&issue, &none, &file_owned);
        assert_eq!(ci.story_arc_number.as_deref(), Some("9, 3"));

        // Not file-owned (a provider wrote the arcs): the file's pairing is
        // not consulted.
        let (ci, _) = arcs_ctx_compose(&issue, &none, &arcs(&[("Alpha", None), ("Zeta", None)]));
        assert_eq!(ci.story_arc_number, None);
    }

    /// A user edit positions a single integer arc only; a user-owned issue
    /// with no position keeps the number column the user typed.
    #[test]
    fn story_arc_number_keeps_a_user_typed_column_without_positions() {
        let mut issue = tests::make_issue("X");
        issue.story_arc_number = Some("2, 5".into());
        let none = GenericMetadata::default();
        let pins: HashSet<String> = ["story_arcs".to_owned()].into();
        let (ci, _) =
            arcs_ctx_compose_pinned(&issue, &none, &arcs(&[("A", None), ("B", None)]), &pins);
        assert_eq!(ci.story_arc.as_deref(), Some("A, B"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("2, 5"));
    }

    /// When the provider's list wins, its fresh position beats one an
    /// older tag left in the junction, in both sidecars.
    #[test]
    fn story_arc_number_prefers_the_providers_position_when_its_list_wins() {
        let issue = tests::make_issue("X");
        let provider = GenericMetadata {
            story_arcs: vec![EntityCandidate {
                name: "The War".into(),
                identifiers: vec![],
                is_first_appearance: false,
                died_in_issue: None,
                disbanded_in_issue: None,
                position_in_arc: Some(2),
            }],
            ..Default::default()
        };
        let (ci, mi) = arcs_ctx_compose(&issue, &provider, &arcs(&[("The War", Some(1))]));
        assert_eq!(ci.story_arc_number.as_deref(), Some("2"));
        assert_eq!(mi.arc_numbers.get("The War"), Some(&2));
    }

    /// A provider apply's arc list wins with its own positions, in the
    /// provider's order, in both sidecars.
    #[test]
    fn story_arc_number_follows_the_provider_list_when_it_wins() {
        let issue = tests::make_issue("X");
        let arc = |name: &str, pos: Option<i32>| EntityCandidate {
            name: name.into(),
            identifiers: vec![],
            is_first_appearance: false,
            died_in_issue: None,
            disbanded_in_issue: None,
            position_in_arc: pos,
        };
        let provider = GenericMetadata {
            story_arcs: vec![arc("The War", Some(2)), arc("Volume One", Some(6))],
            ..Default::default()
        };
        let (ci, mi) = arcs_ctx_compose(&issue, &provider, &IssueJunctions::default());
        assert_eq!(ci.story_arc.as_deref(), Some("The War, Volume One"));
        assert_eq!(ci.story_arc_number.as_deref(), Some("2, 6"));
        assert_eq!(mi.arc_numbers.get("Volume One"), Some(&6));
    }
}
