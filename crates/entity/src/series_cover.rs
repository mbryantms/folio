//! Per-series cover (banner / logo / catalog image). Mirrors
//! [`super::issue_cover`] without variants; series typically have a
//! single canonical banner per source.

use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `series_cover.kind` stays `TEXT` (no CHECK)
// holding exactly these strings; `string_value` and the serde name are the
// same string (tests below). An unknown stored string fails to load with a
// `DbErr`.

/// What a `series_cover` row is. The only writer today is the admin
/// cover pick (`pick`); `primary` / `logo` / `banner` are the provider
/// image kinds the M0 schema reserved, distinct from `pick` so a pick
/// never collides with a downloaded banner on `(series_id, kind, ordinal)`.
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
pub enum SeriesCoverKind {
    /// A provider's canonical series image.
    #[sea_orm(string_value = "primary")]
    Primary,
    /// A provider series logo.
    #[sea_orm(string_value = "logo")]
    Logo,
    /// A provider series banner.
    #[sea_orm(string_value = "banner")]
    Banner,
    /// An admin-picked series cover (`source_provider = 'user'`).
    #[sea_orm(string_value = "pick")]
    Pick,
}

impl SeriesCoverKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Logo => "logo",
            Self::Banner => "banner",
            Self::Pick => "pick",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_cover")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub series_id: Uuid,
    /// `'primary' | 'logo' | 'banner' | 'pick'`. See [`SeriesCoverKind`].
    pub kind: SeriesCoverKind,
    pub ordinal: i32,
    #[sea_orm(nullable)]
    pub source_provider: Option<String>,
    #[sea_orm(nullable)]
    pub source_external_id: Option<String>,
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    pub local_path: String,
    #[sea_orm(nullable)]
    pub width: Option<i32>,
    #[sea_orm(nullable)]
    pub height: Option<i32>,
    pub fetched_at: DateTimeWithTimeZone,
    pub is_active: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
}

impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
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
                (SeriesCoverKind::Primary, "primary"),
                (SeriesCoverKind::Logo, "logo"),
                (SeriesCoverKind::Banner, "banner"),
                (SeriesCoverKind::Pick, "pick"),
            ],
            SeriesCoverKind::as_str,
            SeriesCoverKind::parse,
            &["Pick", "variant", "user", ""],
        );
    }
}
