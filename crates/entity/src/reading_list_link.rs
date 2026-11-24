//! A link from one reading list to another (linked reading lists, PR 1).
//!
//! `kind = event`: the child list is read after the parent entry named by
//! the anchor key. `kind = continues`: the child list is the next part,
//! read after the parent's last entry (no anchor).
//!
//! The anchor is an issue identity, never a `cbl_entries` id — a refresh
//! rewrites every entry. See `m20271124_000001_reading_list_links`.

use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Read the child list after the anchor issue (a detour).
    #[sea_orm(string_value = "event")]
    Event,
    /// The child list is the next part, read after the parent ends.
    #[sea_orm(string_value = "continues")]
    Continues,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Continues => "continues",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    /// The anchor issue is in the parent list.
    #[sea_orm(string_value = "placed")]
    Placed,
    /// A refresh removed the anchor issue; drawn at the position hint.
    #[sea_orm(string_value = "lost")]
    Lost,
}

impl AnchorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Placed => "placed",
            Self::Lost => "lost",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum LinkPlacement {
    /// Derived by diffing a with-events list against its no-events twin.
    #[sea_orm(string_value = "auto_twin")]
    AutoTwin,
    /// Derived from consecutive part numbers.
    #[sea_orm(string_value = "auto_chain")]
    AutoChain,
    /// Placed by a person.
    #[sea_orm(string_value = "manual")]
    Manual,
}

impl LinkPlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoTwin => "auto_twin",
            Self::AutoChain => "auto_chain",
            Self::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "reading_list_links")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub parent_list_id: Uuid,
    pub child_list_id: Uuid,
    pub kind: LinkKind,
    #[sea_orm(nullable)]
    pub anchor_cv_issue_id: Option<i32>,
    #[sea_orm(nullable)]
    pub anchor_series: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_number: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_volume: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_year: Option<String>,
    /// The anchor's `cbl_entries.position` (0-based) the last time it
    /// resolved. Where a lost anchor is drawn.
    #[sea_orm(nullable)]
    pub anchor_position_hint: Option<i32>,
    pub anchor_status: AnchorStatus,
    pub placement: LinkPlacement,
    pub sort_within_anchor: i32,
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cbl_list::Entity",
        from = "Column::ParentListId",
        to = "super::cbl_list::Column::Id",
        on_delete = "Cascade"
    )]
    Parent,
    #[sea_orm(
        belongs_to = "super::cbl_list::Entity",
        from = "Column::ChildListId",
        to = "super::cbl_list::Column::Id",
        on_delete = "Cascade"
    )]
    Child,
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn enums_round_trip_exact_db_strings() {
        assert_exact_strings(
            &[
                (LinkKind::Event, "event"),
                (LinkKind::Continues, "continues"),
            ],
            LinkKind::as_str,
            LinkKind::parse,
            &["Event", ""],
        );
        assert_exact_strings(
            &[
                (AnchorStatus::Placed, "placed"),
                (AnchorStatus::Lost, "lost"),
            ],
            AnchorStatus::as_str,
            AnchorStatus::parse,
            &["Lost", ""],
        );
        assert_exact_strings(
            &[
                (LinkPlacement::AutoTwin, "auto_twin"),
                (LinkPlacement::AutoChain, "auto_chain"),
                (LinkPlacement::Manual, "manual"),
            ],
            LinkPlacement::as_str,
            LinkPlacement::parse,
            &["auto", ""],
        );
    }
}
