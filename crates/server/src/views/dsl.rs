//! Wire format for the filter DSL.
//!
//! The DSL is shipped over JSON as `saved_views.conditions` and as the
//! `conditions` field on the create/update request bodies. Validation
//! against the field/op registry happens in `compile::compile`.
//!
//! `value` is intentionally typed as `serde_json::Value` because its
//! shape varies by `(field, op)` — a scalar for `equals`/`gt`, a 2-tuple
//! array for `between`, an array of strings for `includes_any`, etc.
//! Centralizing that polymorphism in the compiler keeps the wire types
//! flat and avoids a combinatorial enum.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Which table a filter view's DSL is compiled against (WP-5.4). Derived
/// from `saved_views.kind` (`filter_series` → `Series`, `filter_issues` →
/// `Issue`) — never sent on the wire, so the discriminator has one home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewEntity {
    Series,
    Issue,
}

impl ViewEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewEntity::Series => "series",
            ViewEntity::Issue => "issue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    All,
    Any,
}

/// All filterable fields. Per-field metadata (kind, allowed ops, SQL
/// column) lives in [`super::registry`]. Adding a field is a two-step:
/// add a variant here + a registry entry. The OpenAPI schema mirrors this
/// enum so the M5 field-picker can iterate variants without a separate
/// JSON constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    // ───── identity ─────
    Library,
    Name,
    // ───── numeric ─────
    Year,
    Volume,
    TotalIssues,
    // ───── text ─────
    Publisher,
    Imprint,
    // ───── enum ─────
    Status,
    AgeRating,
    LanguageCode,
    // ───── date ─────
    CreatedAt,
    UpdatedAt,
    // ───── multi (junction-backed) ─────
    Genres,
    Tags,
    Writer,
    Penciller,
    Inker,
    Colorist,
    Letterer,
    CoverArtist,
    Editor,
    Translator,
    Characters,
    Teams,
    Locations,
    // ───── per-user reading-state (joined via user_series_progress) ─────
    ReadProgress,
    LastRead,
    ReadCount,
    // ───── derived per-user rollups (library-filters-richer-1.0 M2+M3) ─
    /// Three-state rollup: `read` (all issues finished) /
    /// `in_progress` (some finished) / `unread` (none finished or
    /// empty series).
    ReadStatus,
    /// Number of unfinished issues in the series for the caller —
    /// `total_count − finished_count`.
    UnreadIssues,
    // ───── derived series-level rollups (library-filters-richer-1.0 M4) ─
    /// Three-state local-collection completeness: `complete` (active
    /// issue count >= `series.total_issues`) / `incomplete` (less) /
    /// `unknown` (`series.total_issues` IS NULL — no canonical
    /// expected count from ComicInfo).
    CollectionCompleteness,
    /// Three-state metadata completeness rolled up across the series'
    /// active issues: `complete` (every issue meets the issue core
    /// criteria) / `partial` (some do) / `needs_metadata` (none do, or
    /// the series has no active issues). Mirrors the per-issue scorer in
    /// `metadata::completeness`.
    MetadataCompleteness,
    // ───── issue-level fields (WP-5.4; `filter_issues` views only) ─────
    /// Spec §6.5 classification: `Special` / `Annual` / `OneShot` /
    /// `TPB`; NULL for an ordinary numbered issue (`is_empty`).
    SpecialType,
    /// ComicInfo `<Format>` (free text: "Annual", "Trade Paperback", …).
    Format,
    /// ComicInfo `<StoryArc>` (the issue's CSV read-cache column).
    StoryArc,
    /// The issue's own title (`issues.title`). Issue views only — on an
    /// issue view `name` is the parent series' name.
    Title,
    // ───── per-user rating (WP-5.4; series and issue views) ─────
    /// The caller's own 0..=5 star rating from `user_ratings` — the series
    /// rating on series views, the issue rating on issue views. Unrated
    /// rows are NULL (`is_empty`).
    Rating,
    // ───── per-user annotations (WP-5.7; series and issue views) ─────
    /// The caller has at least one `note` marker on the issue (issue
    /// views) or on any issue of the series (series views).
    HasNotes,
    /// Same, for `bookmark` markers.
    HasBookmarks,
    /// Same, for `highlight` markers.
    HasHighlights,
    /// The caller has starred something on the issue / series (WP-8.4):
    /// a `favorite` marker or any marker with `is_favorite` set — the
    /// same union the /bookmarks "Favorites" chip shows.
    HasFavorites,
    // ───── filter-parity-and-builtin-views-1.0 M1: finish timestamps ─────
    /// When the caller finished the row: the issue's own
    /// `progress_records.finished_at` on issue views, the most recent
    /// finish across the series' active issues on series views. NULL
    /// until something is finished (`is_empty`).
    FinishedAt,
    // ───── M4: junction-backed entities (both views) ─────
    /// Story arcs by name (`issue_arcs` / `series_arcs` → `story_arc`).
    StoryArcs,
    /// Universes by name (`issue_universes` / `series_universes`).
    Universes,
    /// Concepts by name (`issue_concepts` / `series_concepts`).
    Concepts,
    /// Objects by name (`issue_objects` / `series_objects`).
    Objects,
    /// Any credit role: the person holds *some* credit on the row
    /// (the grid's any-role `credits` facet).
    AnyCredit,
    // ───── M4: provider linkage + release date ─────
    /// `matched` when the row has a provider id in `external_ids`
    /// (the same source list the admin metadata dashboard counts),
    /// `unmatched` otherwise.
    ProviderMatch,
    /// Release date: `(year, month, day)` composed into a date on
    /// issue views (missing month / day fall back to 1); `year`-01-01
    /// on series views. NULL when the year is unknown.
    ReleaseDate,
    // ───── M4: more scalar columns ─────
    /// Series type (MetronInfo vocabulary: "Ongoing Series", "Limited
    /// Series", "Annual Series", "Trade Paperback", …). Free text; the
    /// parent series' value on issue views.
    SeriesType,
    /// ComicInfo `<SeriesGroup>`; the parent series' value on issue views.
    SeriesGroup,
    /// Last publication year of the series (`series.year_end`); the
    /// parent series' value on issue views.
    YearEnd,
    /// `issues.page_count` (issue views only).
    PageCount,
    /// The row's own summary / description text.
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    // text
    Contains,
    NotContains,
    StartsWith,
    /// Suffix match (`LIKE '%value'`), the mirror of [`Op::StartsWith`].
    EndsWith,
    // text + enum + number
    Equals,
    NotEquals,
    // enum (alias of equals/not_equals; preserved for UI clarity)
    Is,
    IsNot,
    // enum + text
    In,
    NotIn,
    // number + date
    Gt,
    Gte,
    Lt,
    Lte,
    Between,
    // date (alias of lt/gt; kept for UI clarity)
    Before,
    After,
    /// Date relative to now: `value` is a positive integer count of days
    /// (e.g., `7` ≡ "in the last 7 days").
    Relative,
    /// Negation of [`Op::Relative`]: strictly older than `value` days
    /// ("not in the last N days"). NULL dates never match — combine
    /// with `is_empty` under `any` to include never-read rows.
    NotRelative,
    // multi
    IncludesAny,
    IncludesAll,
    Excludes,
    // bool
    IsTrue,
    IsFalse,
    /// NULL (or blank text / no junction row / no rating). Takes no
    /// value. WP-5.4.
    IsEmpty,
    /// Negation of [`Op::IsEmpty`]. WP-5.4.
    IsNotEmpty,
}

// ───── size bounds (enforced on writes and previews only) ─────
//
// Every condition compiles to its own predicate and a list value to one
// bound parameter per element, so an unbounded filter compiles to an
// unbounded query. These caps are enforced by `garde` on the request
// bodies that carry a `FilterDsl` (saved-view create / update and the two
// preview routes, through `Validated<T>`); a failure is a 422 with an
// `error.details` entry. Stored views are NOT re-checked on read: rows
// written before the caps existed keep running (one WARN per run, see
// `log_if_over_bounds`) with the filter statement timeout as the backstop.
// Mirrored by hand in `web/lib/saved-views/filter-bounds.ts`.

/// Most conditions one filter may hold. The DSL is a flat list today
/// (`group_id` is always 0, no nested groups), so this is simply
/// `conditions.len()`; once groups land, count every leaf condition in
/// every group against the same cap.
pub const MAX_FILTER_CONDITIONS: usize = 50;
/// Most elements in one list-valued condition (`in`, `includes_any`, …).
pub const MAX_FILTER_LIST_VALUES: usize = 200;
/// Longest single string value, in UTF-8 bytes (the same 200-byte cap the
/// free-text search endpoints put on their query).
pub const MAX_FILTER_VALUE_LEN: usize = 200;

/// One condition row in a filter DSL. `group_id` always 0 in v1; reserved
/// for nested-group support without a wire-format break.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, garde::Validate)]
pub struct Condition {
    #[serde(default)]
    #[garde(skip)]
    pub group_id: u8,
    #[garde(skip)]
    pub field: Field,
    #[garde(skip)]
    pub op: Op,
    /// Shape varies by `(field, op)`. The compiler validates and rejects
    /// mismatches with a clear error. See [`super::registry`] for the
    /// matrix of allowed combinations. Bounded by
    /// [`MAX_FILTER_LIST_VALUES`] / [`MAX_FILTER_VALUE_LEN`].
    #[serde(default = "default_value")]
    #[garde(custom(value_within_bounds))]
    pub value: serde_json::Value,
}

fn default_value() -> serde_json::Value {
    serde_json::Value::Null
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, garde::Validate)]
pub struct FilterDsl {
    #[garde(skip)]
    pub match_mode: MatchMode,
    /// At most [`MAX_FILTER_CONDITIONS`] on a write or preview.
    #[garde(custom(conditions_within_bounds), dive)]
    pub conditions: Vec<Condition>,
}

fn conditions_within_bounds(conditions: &[Condition], _: &()) -> garde::Result {
    if conditions.len() > MAX_FILTER_CONDITIONS {
        return Err(garde::Error::new(format!(
            "a filter can have at most {MAX_FILTER_CONDITIONS} conditions"
        )));
    }
    Ok(())
}

fn value_within_bounds(value: &serde_json::Value, _: &()) -> garde::Result {
    let too_long = || {
        garde::Error::new(format!(
            "a value can be at most {MAX_FILTER_VALUE_LEN} bytes long"
        ))
    };
    match value {
        serde_json::Value::String(s) if s.len() > MAX_FILTER_VALUE_LEN => Err(too_long()),
        serde_json::Value::Array(items) => {
            if items.len() > MAX_FILTER_LIST_VALUES {
                return Err(garde::Error::new(format!(
                    "a condition can list at most {MAX_FILTER_LIST_VALUES} values"
                )));
            }
            if items
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s.len() > MAX_FILTER_VALUE_LEN))
            {
                return Err(too_long());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

impl FilterDsl {
    /// Size of the filter against the write-time caps:
    /// `(conditions, largest list value)`.
    pub fn size(&self) -> (usize, usize) {
        let largest_list = self
            .conditions
            .iter()
            .filter_map(|c| c.value.as_array().map(Vec::len))
            .max()
            .unwrap_or(0);
        (self.conditions.len(), largest_list)
    }

    /// Stored-view grandfathering: a view read from the database is never
    /// rejected on the size caps (they apply to new writes and previews
    /// only), but one over them is logged so an operator can find it.
    /// Called from every place that compiles a STORED view
    /// (`saved_views::dsl_from_view`, `opds::dsl_from_view`); to start
    /// enforcing the caps on stored views, reject here instead.
    pub fn log_if_over_bounds(&self, view_id: uuid::Uuid) {
        let (conditions, largest_list) = self.size();
        if conditions > MAX_FILTER_CONDITIONS || largest_list > MAX_FILTER_LIST_VALUES {
            tracing::warn!(
                view_id = %view_id,
                conditions,
                largest_list,
                max_conditions = MAX_FILTER_CONDITIONS,
                max_list_values = MAX_FILTER_LIST_VALUES,
                "saved view filter exceeds the write-time bounds; running it anyway"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        }
    }
}

/// Sort axes a saved view can choose. Distinct from [`Field`] because not
/// every filterable field is a sensible sort axis (e.g., multi-valued
/// genres can't sort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    Name,
    Year,
    CreatedAt,
    UpdatedAt,
    /// Per-user; series views sorted by this trigger the reading-state
    /// join; issue views join the caller's `progress_records` row.
    LastRead,
    /// Per-user (series rollup percent / the issue's own percent).
    ReadProgress,
    /// Per-user finish timestamp (filter-parity-and-builtin-views-1.0):
    /// the caller's `progress_records.finished_at` on issue views, the
    /// latest finish across the series on series views. Unfinished rows
    /// sort last on `desc`.
    FinishedAt,
    /// Per-user star rating; unrated rows sort last on `desc`.
    Rating,
    /// `issues.page_count` (issue views only).
    PageCount,
    /// `issues.sort_number` (issue views only).
    Number,
}

impl SortField {
    pub fn as_str(self) -> &'static str {
        match self {
            SortField::Name => "name",
            SortField::Year => "year",
            SortField::CreatedAt => "created_at",
            SortField::UpdatedAt => "updated_at",
            SortField::LastRead => "last_read",
            SortField::ReadProgress => "read_progress",
            SortField::FinishedAt => "finished_at",
            SortField::Rating => "rating",
            SortField::PageCount => "page_count",
            SortField::Number => "number",
        }
    }

    /// Parse the string form persisted in `saved_views.sort_field`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "name" => SortField::Name,
            "year" => SortField::Year,
            "created_at" => SortField::CreatedAt,
            "updated_at" => SortField::UpdatedAt,
            "last_read" => SortField::LastRead,
            "read_progress" => SortField::ReadProgress,
            "finished_at" => SortField::FinishedAt,
            "rating" => SortField::Rating,
            "page_count" => SortField::PageCount,
            "number" => SortField::Number,
            _ => return None,
        })
    }
}
