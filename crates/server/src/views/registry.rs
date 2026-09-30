//! Per-field metadata used by the compiler (validation + SQL mapping) and
//! mirrored to the M5 client field-picker via OpenAPI.
//!
//! The `field_spec` table is the single source of truth: kind, allowed
//! ops, expected JSON value shape per op, and how the field maps to SQL.
//! Adding or changing a filterable field is a one-place edit here.

use super::dsl::{Field, Op, ViewEntity};

/// High-level value family. The compiler maps these to SQL operators and
/// dispatches the value validator per `(kind, op)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Free text, scalar value.
    Text,
    /// Integer / float scalar.
    Number,
    /// Timestamp (ISO 8601 / RFC 3339 strings on the wire).
    Date,
    /// Closed enum — see `enum_values` for legal scalar choices.
    Enum,
    /// FK lookup: scalar value is a UUID matched against `enum_values` at
    /// validate time only when the caller wants to constrain choices.
    Uuid,
    /// Multi-valued junction-backed field — ops act on a set.
    Multi,
}

/// Where the field lives in SQL. `Series(col)` filters on a column of the
/// `series` table — on a series view that is the row itself, on an issue
/// view it is the issue's parent series (joined as `series`).
/// `Issue(col)` filters on a column of `issues` (issue views only).
/// `JunctionExists{...}` compiles to an `EXISTS (SELECT 1 FROM table WHERE
/// table.series_id = series.id AND ...)` on series views, or
/// `table.issue_id = issues.id` on issue views (or `NOT EXISTS` for
/// `Excludes`). `Reading(col)` reads from the `user_series_progress`
/// LEFT JOIN, COALESCE'd to a sensible zero for unstarted series.
/// `ReadingComputed(tag)` is the same JOIN but evaluates a CASE / arithmetic
/// expression built per-tag inside `reading_computed_predicate` — used for
/// derived per-user fields like `read_status` (3-state rollup) and
/// `unread_issues` (`total_count - finished_count`).
/// `SeriesComputed(tag)` joins an aggregate against another table and
/// evaluates a per-tag expression — used for `collection_completeness`,
/// which compares `series.total_issues` to a `COUNT(issues)` aggregate.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// Identifier matches a `series::Column` variant by name (snake_case).
    Series(&'static str),
    /// Identifier matches an `issue::Column` variant by name (snake_case).
    /// Issue views only (WP-5.4).
    Issue(&'static str),
    /// `(table_name, value_column)` — the join column is `series_id` for
    /// `series_*` tables and `issue_id` for `issue_*` tables.
    /// For credits the lookup is by `(role, person)` (see `role` field).
    JunctionExists {
        table: &'static str,
        value_col: &'static str,
        /// Some(role) restricts the EXISTS to that credit role; None for
        /// genres/tags where there is no role.
        role: Option<&'static str>,
    },
    /// Pulled from the `user_series_progress` view.
    Reading(&'static str),
    /// Derived per-user expression over `user_series_progress`. The tag
    /// disambiguates which expression — handled in
    /// `compile::reading_computed_predicate`. Wire format: `read_status`
    /// (CASE rollup), `unread_issues` (`total_count - finished_count`).
    ReadingComputed(&'static str),
    /// Derived series-level expression over an extra join. The tag
    /// disambiguates the predicate — handled in
    /// `compile::series_computed_predicate`. Wire format:
    /// `collection_completeness` (`series.total_issues` vs active issue
    /// count).
    SeriesComputed(&'static str),
    /// Derived per-user expression over the caller's `progress_records`
    /// row for the issue (issue views only, WP-5.4). Tag `read_status` is
    /// the same three-state rollup `GET /issues?read_status=` uses.
    IssueComputed(&'static str),
    /// The caller's own `user_ratings.rating` for the row — target type
    /// `series` on series views, `issue` on issue views. NULL when unrated.
    UserRating,
}

#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub field: Field,
    pub kind: FieldKind,
    /// Lower-snake-case identifier used on the wire (matches the serde
    /// rename of the `Field` variant). Centralized so the compiler can
    /// stringify `Field` once for SQL parameter binding.
    pub id: &'static str,
    /// Human label for the M5 field-picker; en-US for now.
    pub label: &'static str,
    /// Mapping on series views (`filter_series`). `None` → the field is
    /// not available there (422 at compile time).
    pub source: Option<Source>,
    /// Mapping on issue views (`filter_issues`, WP-5.4). `None` → the
    /// field is series-only (per-series rollups like `unread_issues`).
    pub issue_source: Option<Source>,
    pub allowed_ops: &'static [Op],
    /// For `FieldKind::Enum`: the legal scalar values. Empty for other
    /// kinds.
    pub enum_values: &'static [&'static str],
}

// Nullable-column op sets carry `is_empty` / `is_not_empty` (WP-5.4). The
// `*_REQUIRED` / `COMPUTED_*` variants omit them: those fields are NOT NULL
// columns or CASE / COALESCE expressions that can never be empty, so the
// op would be a constant.
const TEXT_OPS: &[Op] = &[
    Op::Contains,
    Op::NotContains,
    Op::StartsWith,
    Op::Equals,
    Op::NotEquals,
    Op::IsEmpty,
    Op::IsNotEmpty,
];
const TEXT_OPS_REQUIRED: &[Op] = &[
    Op::Contains,
    Op::NotContains,
    Op::StartsWith,
    Op::Equals,
    Op::NotEquals,
];
const NUMBER_OPS: &[Op] = &[
    Op::Equals,
    Op::NotEquals,
    Op::Gt,
    Op::Gte,
    Op::Lt,
    Op::Lte,
    Op::Between,
    Op::IsEmpty,
    Op::IsNotEmpty,
];
const COMPUTED_NUMBER_OPS: &[Op] = &[
    Op::Equals,
    Op::NotEquals,
    Op::Gt,
    Op::Gte,
    Op::Lt,
    Op::Lte,
    Op::Between,
];
const DATE_OPS: &[Op] = &[
    Op::Before,
    Op::After,
    Op::Between,
    Op::Relative,
    Op::Lt,
    Op::Gt,
    Op::IsEmpty,
    Op::IsNotEmpty,
];
const DATE_OPS_REQUIRED: &[Op] = &[
    Op::Before,
    Op::After,
    Op::Between,
    Op::Relative,
    Op::Lt,
    Op::Gt,
];
const ENUM_OPS: &[Op] = &[
    Op::Is,
    Op::IsNot,
    Op::In,
    Op::NotIn,
    Op::IsEmpty,
    Op::IsNotEmpty,
];
const COMPUTED_ENUM_OPS: &[Op] = &[Op::Is, Op::IsNot, Op::In, Op::NotIn];
const MULTI_OPS: &[Op] = &[
    Op::IncludesAny,
    Op::IncludesAll,
    Op::Excludes,
    Op::IsEmpty,
    Op::IsNotEmpty,
];

/// `issues.special_type` values the scanner writes (spec §6.5,
/// `scanner::process::detect_special_type`). NULL = ordinary issue.
const SPECIAL_TYPE_VALUES: &[&str] = &["Annual", "Special", "OneShot", "TPB"];

/// Status enum values come from `series.status`; kept in sync with the
/// scanner-side default ('continuing'). Limited list so the UI can render
/// a select.
const SERIES_STATUS_VALUES: &[&str] = &["continuing", "ended", "cancelled", "hiatus", "limited"];
/// Three-state read rollup over `user_series_progress`. Computed via a
/// CASE expression in `compile::reading_computed_predicate`.
/// library-filters-richer-1.0 M2.
const READ_STATUS_VALUES: &[&str] = &["read", "in_progress", "unread"];
/// Three-state local-collection completeness over `series.total_issues`
/// vs `COUNT(issues)`. `unknown` covers `total_issues IS NULL` (series
/// without a canonical expected count from ComicInfo).
/// library-filters-richer-1.0 M4.
const COLLECTION_COMPLETENESS_VALUES: &[&str] = &["complete", "incomplete", "unknown"];
/// Three-state metadata completeness rolled up over a series' active issues
/// (every / some / no issue meets the issue core criteria). Mirrors the
/// per-issue tiers in `metadata::completeness::CompletenessTier`.
const METADATA_COMPLETENESS_VALUES: &[&str] = &["complete", "partial", "needs_metadata"];
/// ComicInfo `AgeRating` values per the Anansi schema. Open-ended in the
/// data (the scanner stores any string), but the UI restricts choices to
/// the known set — unknown values still match via direct equality.
const AGE_RATING_VALUES: &[&str] = &[
    "Unknown",
    "Adults Only 18+",
    "Early Childhood",
    "Everyone",
    "Everyone 10+",
    "G",
    "Kids to Adults",
    "M",
    "MA15+",
    "Mature 17+",
    "PG",
    "R18+",
    "Rating Pending",
    "Teen",
    "X18+",
];

const SPECS: &[FieldSpec] = &[
    FieldSpec {
        field: Field::Library,
        kind: FieldKind::Uuid,
        id: "library",
        label: "Library",
        source: Some(Source::Series("library_id")),
        issue_source: Some(Source::Issue("library_id")),
        allowed_ops: &[Op::Equals, Op::NotEquals, Op::In, Op::NotIn],
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Name,
        kind: FieldKind::Text,
        id: "name",
        label: "Name",
        source: Some(Source::Series("name")),
        issue_source: Some(Source::Series("name")),
        allowed_ops: TEXT_OPS_REQUIRED,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Year,
        kind: FieldKind::Number,
        id: "year",
        label: "Year",
        source: Some(Source::Series("year")),
        issue_source: Some(Source::Issue("year")),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Volume,
        kind: FieldKind::Number,
        id: "volume",
        label: "Volume",
        source: Some(Source::Series("volume")),
        issue_source: Some(Source::Issue("volume")),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::TotalIssues,
        kind: FieldKind::Number,
        id: "total_issues",
        label: "Total Issues",
        source: Some(Source::Series("total_issues")),
        issue_source: Some(Source::Series("total_issues")),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Publisher,
        kind: FieldKind::Text,
        id: "publisher",
        label: "Publisher",
        source: Some(Source::Series("publisher")),
        issue_source: Some(Source::Issue("publisher")),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Imprint,
        kind: FieldKind::Text,
        id: "imprint",
        label: "Imprint",
        source: Some(Source::Series("imprint")),
        issue_source: Some(Source::Issue("imprint")),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Status,
        kind: FieldKind::Enum,
        id: "status",
        label: "Status",
        source: Some(Source::Series("status")),
        issue_source: Some(Source::Series("status")),
        allowed_ops: COMPUTED_ENUM_OPS,
        enum_values: SERIES_STATUS_VALUES,
    },
    FieldSpec {
        field: Field::AgeRating,
        kind: FieldKind::Enum,
        id: "age_rating",
        label: "Age Rating",
        source: Some(Source::Series("age_rating")),
        issue_source: Some(Source::Issue("age_rating")),
        allowed_ops: ENUM_OPS,
        enum_values: AGE_RATING_VALUES,
    },
    FieldSpec {
        field: Field::LanguageCode,
        kind: FieldKind::Text,
        id: "language_code",
        label: "Language",
        source: Some(Source::Series("language_code")),
        issue_source: Some(Source::Issue("language_code")),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::CreatedAt,
        kind: FieldKind::Date,
        id: "created_at",
        label: "Created At",
        source: Some(Source::Series("created_at")),
        issue_source: Some(Source::Issue("created_at")),
        allowed_ops: DATE_OPS_REQUIRED,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::UpdatedAt,
        kind: FieldKind::Date,
        id: "updated_at",
        label: "Updated At",
        source: Some(Source::Series("updated_at")),
        issue_source: Some(Source::Issue("updated_at")),
        allowed_ops: DATE_OPS_REQUIRED,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Genres,
        kind: FieldKind::Multi,
        id: "genres",
        label: "Genres",
        source: Some(Source::JunctionExists {
            table: "series_genres",
            value_col: "genre",
            role: None,
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_genres",
            value_col: "genre",
            role: None,
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Tags,
        kind: FieldKind::Multi,
        id: "tags",
        label: "Tags",
        source: Some(Source::JunctionExists {
            table: "series_tags",
            value_col: "tag",
            role: None,
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_tags",
            value_col: "tag",
            role: None,
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Writer,
        kind: FieldKind::Multi,
        id: "writer",
        label: "Writers",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("writer"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("writer"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Penciller,
        kind: FieldKind::Multi,
        id: "penciller",
        label: "Pencillers",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("penciller"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("penciller"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Inker,
        kind: FieldKind::Multi,
        id: "inker",
        label: "Inkers",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("inker"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("inker"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Colorist,
        kind: FieldKind::Multi,
        id: "colorist",
        label: "Colorists",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("colorist"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("colorist"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Letterer,
        kind: FieldKind::Multi,
        id: "letterer",
        label: "Letterers",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("letterer"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("letterer"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::CoverArtist,
        kind: FieldKind::Multi,
        id: "cover_artist",
        label: "Cover Artists",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("cover_artist"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("cover_artist"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Editor,
        kind: FieldKind::Multi,
        id: "editor",
        label: "Editors",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("editor"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("editor"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Translator,
        kind: FieldKind::Multi,
        id: "translator",
        label: "Translators",
        source: Some(Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("translator"),
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_credits",
            value_col: "person",
            role: Some("translator"),
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Characters,
        kind: FieldKind::Multi,
        id: "characters",
        label: "Characters",
        source: Some(Source::JunctionExists {
            table: "series_characters",
            value_col: "character",
            role: None,
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_characters",
            value_col: "character",
            role: None,
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Teams,
        kind: FieldKind::Multi,
        id: "teams",
        label: "Teams",
        source: Some(Source::JunctionExists {
            table: "series_teams",
            value_col: "team",
            role: None,
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_teams",
            value_col: "team",
            role: None,
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Locations,
        kind: FieldKind::Multi,
        id: "locations",
        label: "Locations",
        source: Some(Source::JunctionExists {
            table: "series_locations",
            value_col: "location",
            role: None,
        }),
        issue_source: Some(Source::JunctionExists {
            table: "issue_locations",
            value_col: "location",
            role: None,
        }),
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::ReadProgress,
        kind: FieldKind::Number,
        id: "read_progress",
        label: "Read Progress",
        source: Some(Source::Reading("percent")),
        issue_source: None,
        allowed_ops: COMPUTED_NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::LastRead,
        kind: FieldKind::Date,
        id: "last_read",
        label: "Last Read",
        source: Some(Source::Reading("last_read_at")),
        issue_source: None,
        allowed_ops: DATE_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::ReadCount,
        kind: FieldKind::Number,
        id: "read_count",
        label: "Read Count",
        source: Some(Source::Reading("finished_count")),
        issue_source: None,
        allowed_ops: COMPUTED_NUMBER_OPS,
        enum_values: &[],
    },
    // ─── library-filters-richer-1.0 M2: read_status enum rollup ──────────
    FieldSpec {
        field: Field::ReadStatus,
        kind: FieldKind::Enum,
        id: "read_status",
        label: "Read Status",
        source: Some(Source::ReadingComputed("read_status")),
        issue_source: Some(Source::IssueComputed("read_status")),
        allowed_ops: COMPUTED_ENUM_OPS,
        enum_values: READ_STATUS_VALUES,
    },
    // ─── library-filters-richer-1.0 M3: unread_issues numeric ─────────────
    FieldSpec {
        field: Field::UnreadIssues,
        kind: FieldKind::Number,
        id: "unread_issues",
        label: "Unread Issues",
        source: Some(Source::ReadingComputed("unread_issues")),
        issue_source: None,
        allowed_ops: COMPUTED_NUMBER_OPS,
        enum_values: &[],
    },
    // ─── library-filters-richer-1.0 M4: collection_completeness enum ─────
    FieldSpec {
        field: Field::CollectionCompleteness,
        kind: FieldKind::Enum,
        id: "collection_completeness",
        label: "Collection Completeness",
        source: Some(Source::SeriesComputed("collection_completeness")),
        issue_source: None,
        allowed_ops: COMPUTED_ENUM_OPS,
        enum_values: COLLECTION_COMPLETENESS_VALUES,
    },
    // ─── metadata-completeness enum (needs-metadata filter) ──────────────
    FieldSpec {
        field: Field::MetadataCompleteness,
        kind: FieldKind::Enum,
        id: "metadata_completeness",
        label: "Metadata Completeness",
        source: Some(Source::SeriesComputed("metadata_completeness")),
        issue_source: None,
        allowed_ops: COMPUTED_ENUM_OPS,
        enum_values: METADATA_COMPLETENESS_VALUES,
    },
    // ─── WP-5.4: issue-level fields ──────────────────────────────────────
    FieldSpec {
        field: Field::SpecialType,
        kind: FieldKind::Enum,
        id: "special_type",
        label: "Special Type",
        source: None,
        issue_source: Some(Source::Issue("special_type")),
        allowed_ops: ENUM_OPS,
        enum_values: SPECIAL_TYPE_VALUES,
    },
    FieldSpec {
        field: Field::Format,
        kind: FieldKind::Text,
        id: "format",
        label: "Format",
        source: None,
        issue_source: Some(Source::Issue("format")),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::StoryArc,
        kind: FieldKind::Text,
        id: "story_arc",
        label: "Story Arc",
        source: None,
        issue_source: Some(Source::Issue("story_arc")),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    // ─── WP-5.4: the caller's own star rating (both entities) ────────────
    FieldSpec {
        field: Field::Rating,
        kind: FieldKind::Number,
        id: "rating",
        label: "My Rating",
        source: Some(Source::UserRating),
        issue_source: Some(Source::UserRating),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
];

pub fn spec_for(field: Field) -> &'static FieldSpec {
    SPECS
        .iter()
        .find(|s| s.field == field)
        .expect("every Field variant has a registry entry")
}

pub fn all_specs() -> &'static [FieldSpec] {
    SPECS
}

/// The SQL mapping for `field` on a view of `entity`, or `None` when the
/// field isn't available there (e.g. `special_type` on a series view).
pub fn source_for(spec: &FieldSpec, entity: ViewEntity) -> Option<Source> {
    match entity {
        ViewEntity::Series => spec.source,
        ViewEntity::Issue => spec.issue_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The static `expect` in `spec_for` would only fire at runtime if a
    // `Field` variant was added without a matching registry row. Anchor
    // the spec count so that gap surfaces during `cargo test`, not in
    // production.
    #[test]
    fn every_field_variant_has_a_spec() {
        // Update `KNOWN_FIELD_COUNT` whenever you add a `Field` variant
        // and a matching `FieldSpec` row. Forgetting both leaves the
        // count unchanged but `spec_for` would panic at runtime — the
        // mismatch is the alarm.
        const KNOWN_FIELD_COUNT: usize = 36;
        assert_eq!(SPECS.len(), KNOWN_FIELD_COUNT);
        for spec in SPECS {
            let looked_up = spec_for(spec.field);
            assert_eq!(looked_up.field, spec.field);
            assert!(
                spec.source.is_some() || spec.issue_source.is_some(),
                "{:?} must be available on at least one entity",
                spec.field
            );
        }
    }

    #[test]
    fn issue_only_fields_have_no_series_mapping() {
        for f in [Field::SpecialType, Field::Format, Field::StoryArc] {
            assert!(source_for(spec_for(f), ViewEntity::Series).is_none());
            assert!(source_for(spec_for(f), ViewEntity::Issue).is_some());
        }
    }

    #[test]
    fn series_rollups_have_no_issue_mapping() {
        for f in [
            Field::ReadProgress,
            Field::LastRead,
            Field::ReadCount,
            Field::UnreadIssues,
            Field::CollectionCompleteness,
            Field::MetadataCompleteness,
        ] {
            assert!(source_for(spec_for(f), ViewEntity::Issue).is_none());
        }
    }
}
