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
    // ───── per-user reading-state (joined via user_series_progress) ─────
    ReadProgress,
    LastRead,
    ReadCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    // text
    Contains,
    StartsWith,
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
    // multi
    IncludesAny,
    IncludesAll,
    Excludes,
    // bool
    IsTrue,
    IsFalse,
}

/// One condition row in a filter DSL. `group_id` always 0 in v1; reserved
/// for nested-group support without a wire-format break.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Condition {
    #[serde(default)]
    pub group_id: u8,
    pub field: Field,
    pub op: Op,
    /// Shape varies by `(field, op)`. The compiler validates and rejects
    /// mismatches with a clear error. See [`super::registry`] for the
    /// matrix of allowed combinations.
    #[serde(default = "default_value")]
    pub value: serde_json::Value,
}

fn default_value() -> serde_json::Value {
    serde_json::Value::Null
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FilterDsl {
    pub match_mode: MatchMode,
    pub conditions: Vec<Condition>,
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
    /// Per-user; views sorted by this trigger the reading-state join.
    LastRead,
    /// Per-user.
    ReadProgress,
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
            _ => return None,
        })
    }
}
