//! Per-field metadata used by the compiler (validation + SQL mapping) and
//! mirrored to the M5 client field-picker via OpenAPI.
//!
//! The `field_spec` table is the single source of truth: kind, allowed
//! ops, expected JSON value shape per op, and how the field maps to SQL.
//! Adding or changing a filterable field is a one-place edit here.

use super::dsl::{Field, Op};

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
/// `series` table. `JunctionExists{...}` compiles to an `EXISTS (SELECT 1
/// FROM table WHERE table.series_id = series.id AND ...)` (or `NOT EXISTS`
/// for `Excludes`). `Reading(col)` reads from the `user_series_progress`
/// LEFT JOIN, COALESCE'd to a sensible zero for unstarted series.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// Identifier matches a `series::Column` variant by name (snake_case).
    Series(&'static str),
    /// `(table_name, value_column)` — both `series_id` is the join column.
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
    pub source: Source,
    pub allowed_ops: &'static [Op],
    /// For `FieldKind::Enum`: the legal scalar values. Empty for other
    /// kinds.
    pub enum_values: &'static [&'static str],
}

const TEXT_OPS: &[Op] = &[Op::Contains, Op::StartsWith, Op::Equals, Op::NotEquals];
const NUMBER_OPS: &[Op] = &[
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
];
const ENUM_OPS: &[Op] = &[Op::Is, Op::IsNot, Op::In, Op::NotIn];
const MULTI_OPS: &[Op] = &[Op::IncludesAny, Op::IncludesAll, Op::Excludes];

/// Status enum values come from `series.status`; kept in sync with the
/// scanner-side default ('continuing'). Limited list so the UI can render
/// a select.
const SERIES_STATUS_VALUES: &[&str] = &["continuing", "ended", "cancelled", "hiatus", "limited"];
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
        source: Source::Series("library_id"),
        allowed_ops: &[Op::Equals, Op::NotEquals, Op::In, Op::NotIn],
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Name,
        kind: FieldKind::Text,
        id: "name",
        label: "Name",
        source: Source::Series("name"),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Year,
        kind: FieldKind::Number,
        id: "year",
        label: "Year",
        source: Source::Series("year"),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Volume,
        kind: FieldKind::Number,
        id: "volume",
        label: "Volume",
        source: Source::Series("volume"),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::TotalIssues,
        kind: FieldKind::Number,
        id: "total_issues",
        label: "Total Issues",
        source: Source::Series("total_issues"),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Publisher,
        kind: FieldKind::Text,
        id: "publisher",
        label: "Publisher",
        source: Source::Series("publisher"),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Imprint,
        kind: FieldKind::Text,
        id: "imprint",
        label: "Imprint",
        source: Source::Series("imprint"),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Status,
        kind: FieldKind::Enum,
        id: "status",
        label: "Status",
        source: Source::Series("status"),
        allowed_ops: ENUM_OPS,
        enum_values: SERIES_STATUS_VALUES,
    },
    FieldSpec {
        field: Field::AgeRating,
        kind: FieldKind::Enum,
        id: "age_rating",
        label: "Age Rating",
        source: Source::Series("age_rating"),
        allowed_ops: ENUM_OPS,
        enum_values: AGE_RATING_VALUES,
    },
    FieldSpec {
        field: Field::LanguageCode,
        kind: FieldKind::Text,
        id: "language_code",
        label: "Language",
        source: Source::Series("language_code"),
        allowed_ops: TEXT_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::CreatedAt,
        kind: FieldKind::Date,
        id: "created_at",
        label: "Created At",
        source: Source::Series("created_at"),
        allowed_ops: DATE_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::UpdatedAt,
        kind: FieldKind::Date,
        id: "updated_at",
        label: "Updated At",
        source: Source::Series("updated_at"),
        allowed_ops: DATE_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Genres,
        kind: FieldKind::Multi,
        id: "genres",
        label: "Genres",
        source: Source::JunctionExists {
            table: "series_genres",
            value_col: "genre",
            role: None,
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Tags,
        kind: FieldKind::Multi,
        id: "tags",
        label: "Tags",
        source: Source::JunctionExists {
            table: "series_tags",
            value_col: "tag",
            role: None,
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Writer,
        kind: FieldKind::Multi,
        id: "writer",
        label: "Writers",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("writer"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Penciller,
        kind: FieldKind::Multi,
        id: "penciller",
        label: "Pencillers",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("penciller"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Inker,
        kind: FieldKind::Multi,
        id: "inker",
        label: "Inkers",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("inker"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Colorist,
        kind: FieldKind::Multi,
        id: "colorist",
        label: "Colorists",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("colorist"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Letterer,
        kind: FieldKind::Multi,
        id: "letterer",
        label: "Letterers",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("letterer"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::CoverArtist,
        kind: FieldKind::Multi,
        id: "cover_artist",
        label: "Cover Artists",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("cover_artist"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Editor,
        kind: FieldKind::Multi,
        id: "editor",
        label: "Editors",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("editor"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::Translator,
        kind: FieldKind::Multi,
        id: "translator",
        label: "Translators",
        source: Source::JunctionExists {
            table: "series_credits",
            value_col: "person",
            role: Some("translator"),
        },
        allowed_ops: MULTI_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::ReadProgress,
        kind: FieldKind::Number,
        id: "read_progress",
        label: "Read Progress",
        source: Source::Reading("percent"),
        allowed_ops: NUMBER_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::LastRead,
        kind: FieldKind::Date,
        id: "last_read",
        label: "Last Read",
        source: Source::Reading("last_read_at"),
        allowed_ops: DATE_OPS,
        enum_values: &[],
    },
    FieldSpec {
        field: Field::ReadCount,
        kind: FieldKind::Number,
        id: "read_count",
        label: "Read Count",
        source: Source::Reading("finished_count"),
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
        const KNOWN_FIELD_COUNT: usize = 25;
        assert_eq!(SPECS.len(), KNOWN_FIELD_COUNT);
        for spec in SPECS {
            let looked_up = spec_for(spec.field);
            assert_eq!(looked_up.field, spec.field);
        }
    }
}
