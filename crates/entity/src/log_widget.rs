use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `log_widgets.kind` stays `TEXT` (no CHECK)
// holding exactly these strings; `string_value` and the serde name are the
// same string (tests below). An unknown stored string fails to load with a
// `DbErr`. Adding a kind: a variant here, a config schema + match arm in
// `api::log_widgets::validate_config`, and a renderer on the web side.

/// Which widget a `/log` page slot renders.
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
pub enum LogWidgetKind {
    #[sea_orm(string_value = "chrono_feed")]
    ChronoFeed,
    #[sea_orm(string_value = "stats_hero")]
    StatsHero,
    #[sea_orm(string_value = "heatmap")]
    Heatmap,
    #[sea_orm(string_value = "top_creators")]
    TopCreators,
    #[sea_orm(string_value = "top_publishers")]
    TopPublishers,
    #[sea_orm(string_value = "top_imprints")]
    TopImprints,
    #[sea_orm(string_value = "series_finishes")]
    SeriesFinishes,
    #[sea_orm(string_value = "pace_chart")]
    PaceChart,
    #[sea_orm(string_value = "time_of_day")]
    TimeOfDay,
    #[sea_orm(string_value = "recent_bookmarks")]
    RecentBookmarks,
    #[sea_orm(string_value = "currently_reading")]
    CurrentlyReading,
    #[sea_orm(string_value = "note")]
    Note,
}

impl LogWidgetKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChronoFeed => "chrono_feed",
            Self::StatsHero => "stats_hero",
            Self::Heatmap => "heatmap",
            Self::TopCreators => "top_creators",
            Self::TopPublishers => "top_publishers",
            Self::TopImprints => "top_imprints",
            Self::SeriesFinishes => "series_finishes",
            Self::PaceChart => "pace_chart",
            Self::TimeOfDay => "time_of_day",
            Self::RecentBookmarks => "recent_bookmarks",
            Self::CurrentlyReading => "currently_reading",
            Self::Note => "note",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// One widget pinned to the user's `/log` page. Reading-Log M3.
///
/// `kind` is a [`LogWidgetKind`]. Renderer registry lives on the web
/// side; the server validates the per-kind `config` blob's shape on
/// every write.
///
/// `position` is a 0-based dense rank — reorders rewrite every row's
/// position in one transaction so we never have to sort by
/// `created_at` as a tiebreaker.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "log_widgets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: LogWidgetKind,
    pub position: i32,
    /// Kind-specific JSON config. Server validates it per kind
    /// (`api::log_widgets::validate_config`) on POST/PATCH; the renderer
    /// re-validates client-side with Zod. `{}` (the column default)
    /// is always a legal value — kinds with no config (e.g.
    /// `time_of_day`) use it.
    pub config: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
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
                (LogWidgetKind::ChronoFeed, "chrono_feed"),
                (LogWidgetKind::StatsHero, "stats_hero"),
                (LogWidgetKind::Heatmap, "heatmap"),
                (LogWidgetKind::TopCreators, "top_creators"),
                (LogWidgetKind::TopPublishers, "top_publishers"),
                (LogWidgetKind::TopImprints, "top_imprints"),
                (LogWidgetKind::SeriesFinishes, "series_finishes"),
                (LogWidgetKind::PaceChart, "pace_chart"),
                (LogWidgetKind::TimeOfDay, "time_of_day"),
                (LogWidgetKind::RecentBookmarks, "recent_bookmarks"),
                (LogWidgetKind::CurrentlyReading, "currently_reading"),
                (LogWidgetKind::Note, "note"),
            ],
            LogWidgetKind::as_str,
            LogWidgetKind::parse,
            &["not_a_real_widget", "Heatmap", "top-creators", ""],
        );
    }
}
