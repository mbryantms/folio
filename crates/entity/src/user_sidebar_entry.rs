use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `user_sidebar_entries.kind` stays `TEXT`, pinned
// by `user_sidebar_entries_kind_chk` to exactly these strings;
// `string_value` and the serde name are the same string (tests below). An
// unknown stored string fails to load with a `DbErr`.

/// What a sidebar entry points at; decides what `ref_id` means.
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
pub enum SidebarEntryKind {
    /// A built-in nav entry; `ref_id` is the registry key.
    #[sea_orm(string_value = "builtin")]
    Builtin,
    /// A library; `ref_id` is the library UUID (or the synthetic
    /// "All Libraries" key).
    #[sea_orm(string_value = "library")]
    Library,
    /// A saved view; `ref_id` is the saved-view UUID.
    #[sea_orm(string_value = "view")]
    View,
    /// A user-created page; `ref_id` is the `user_page` id.
    #[sea_orm(string_value = "page")]
    Page,
    /// A section title row; `label` carries the text.
    #[sea_orm(string_value = "header")]
    Header,
    /// A visual gap row.
    #[sea_orm(string_value = "spacer")]
    Spacer,
}

impl SidebarEntryKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Library => "library",
            Self::View => "view",
            Self::Page => "page",
            Self::Header => "header",
            Self::Spacer => "spacer",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// Per-user override of one sidebar entry (navigation customization M1).
/// Missing rows mean "default behavior" — the layout resolver
/// (`server::sidebar_layout::compute_layout`) merges these overrides on
/// top of a computed default list, so adding a new library or saved view
/// doesn't require a backfill.
///
/// `kind` is a [`SidebarEntryKind`]; `ref_id` is the registry key,
/// library UUID, saved-view UUID, page id, or a client-generated UUID for
/// headers and spacers (TEXT because the builtin keys are strings).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_sidebar_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: SidebarEntryKind,
    #[sea_orm(primary_key, auto_increment = false)]
    pub ref_id: String,
    pub visible: bool,
    pub position: i32,
    /// Optional label override. For `kind='header'` this is the section
    /// title; for `kind='spacer'` it's ignored; for everything else it
    /// overrides the server-resolved label (e.g. rename "Bookmarks"
    /// → "Pins"). `NULL` falls back to the default.
    #[sea_orm(nullable)]
    pub label: Option<String>,
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
                (SidebarEntryKind::Builtin, "builtin"),
                (SidebarEntryKind::Library, "library"),
                (SidebarEntryKind::View, "view"),
                (SidebarEntryKind::Page, "page"),
                (SidebarEntryKind::Header, "header"),
                (SidebarEntryKind::Spacer, "spacer"),
            ],
            SidebarEntryKind::as_str,
            SidebarEntryKind::parse,
            &["bogus", "Header", "views", ""],
        );
    }
}
