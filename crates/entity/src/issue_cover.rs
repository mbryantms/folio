//! Per-issue cover storage. Primary + variants + back covers, per-row
//! provenance, per-cover perceptual hash (populated by M9). The cover
//! served as the issue's display thumbnail is `(kind='primary',
//! ordinal=0, is_active=true)`.

use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `issue_cover.kind` stays `TEXT` (no CHECK)
// holding exactly these strings; `string_value` and the serde name are the
// same string (tests below). An unknown stored string fails to load with a
// `DbErr`.

/// What an `issue_cover` row is. Writers today store `primary` (the
/// archive / provider / admin-picked display cover) and `variant`
/// (provider variant covers); `back` and `incentive` are the gallery
/// kinds the M0 schema reserved.
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
pub enum IssueCoverKind {
    /// The display cover; `(primary, 0, active)` is the thumbnail.
    #[sea_orm(string_value = "primary")]
    Primary,
    /// A provider variant cover.
    #[sea_orm(string_value = "variant")]
    Variant,
    /// A back cover.
    #[sea_orm(string_value = "back")]
    Back,
    /// A retailer incentive (ratio) cover.
    #[sea_orm(string_value = "incentive")]
    Incentive,
}

impl IssueCoverKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Variant => "variant",
            Self::Back => "back",
            Self::Incentive => "incentive",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_cover")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub issue_id: String,
    /// `'primary' | 'variant' | 'back' | 'incentive'`. See [`IssueCoverKind`].
    pub kind: IssueCoverKind,
    pub ordinal: i32,
    /// `'comicvine' | 'metron' | 'archive_extracted' | 'user_upload'`
    /// or NULL for legacy backfill rows.
    #[sea_orm(nullable)]
    pub source_provider: Option<String>,
    #[sea_orm(nullable)]
    pub source_external_id: Option<String>,
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    #[sea_orm(nullable)]
    pub variant_label: Option<String>,
    #[sea_orm(nullable)]
    pub variant_artist_person_id: Option<Uuid>,
    /// Relative to `{data_path}`. The cover-serving code prepends the
    /// configured data root.
    pub local_path: String,
    #[sea_orm(nullable)]
    pub width: Option<i32>,
    #[sea_orm(nullable)]
    pub height: Option<i32>,
    #[sea_orm(nullable)]
    pub phash: Option<i64>,
    #[sea_orm(nullable)]
    pub dhash: Option<i64>,
    #[sea_orm(nullable)]
    pub ahash: Option<i64>,
    pub fetched_at: DateTimeWithTimeZone,
    pub is_active: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
    #[sea_orm(
        belongs_to = "super::person::Entity",
        from = "Column::VariantArtistPersonId",
        to = "super::person::Column::Id",
        on_delete = "SetNull"
    )]
    VariantArtist,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (IssueCoverKind::Primary, "primary"),
                (IssueCoverKind::Variant, "variant"),
                (IssueCoverKind::Back, "back"),
                (IssueCoverKind::Incentive, "incentive"),
            ],
            IssueCoverKind::as_str,
            IssueCoverKind::parse,
            &["Primary", "pick", "logo", "front", ""],
        );
    }
}
