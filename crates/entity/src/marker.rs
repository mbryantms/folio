use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `markers.kind` stays `TEXT`, pinned by the
// `markers_kind_chk` CHECK to exactly these strings
// (`m20260520_000001_marker_kind_favorite`); `string_value` and the serde
// name are the same string (tests below). An unknown stored string fails
// to load with a `DbErr`.

/// What a `markers` row is. The markers API adds a shape rule per kind:
/// a note needs `body`, a highlight needs `region`.
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
pub enum MarkerKind {
    /// A page bookmark, optionally region-scoped.
    #[sea_orm(string_value = "bookmark")]
    Bookmark,
    /// A Markdown note; `body` is required.
    #[sea_orm(string_value = "note")]
    Note,
    /// A page-level favorite (the reader's page star).
    #[sea_orm(string_value = "favorite")]
    Favorite,
    /// A region highlight; `region` is required.
    #[sea_orm(string_value = "highlight")]
    Highlight,
}

impl MarkerKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bookmark => "bookmark",
            Self::Note => "note",
            Self::Favorite => "favorite",
            Self::Highlight => "highlight",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// One marker — bookmark, note, favorite, or highlight — anchored on a
/// `(user, issue, page)` triple. Region is normalized to 0–100% of the
/// page's natural pixel dims so the reader overlay survives resize and
/// fit-mode changes without recomputation. Per-kind invariants
/// (`body` required for notes, `region` required for highlights) are
/// enforced by the `markers_*_chk` CHECK constraints in the schema.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "markers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub series_id: Uuid,
    /// `issues.id` is BLAKE3 hex (TEXT), matching the rest of the
    /// schema.
    pub issue_id: String,
    pub page_index: i32,
    /// `'bookmark' | 'note' | 'favorite' | 'highlight'`. Favorite was
    /// briefly a flag (2026-05-12 → 2026-05-20) and is now a kind
    /// again, this time *independent* of bookmark — the reader's
    /// page-level star creates a standalone `kind='favorite'` row.
    /// See [`m20260520_000001_marker_kind_favorite`](../../migration/src/m20260520_000001_marker_kind_favorite.rs)
    /// and [`MarkerKind`].
    pub kind: MarkerKind,
    /// Legacy star flag from the 2026-05-12 "favorite as a boolean"
    /// architecture. Still set by `MarkerEditor` for per-marker
    /// starring (e.g. "I want to remember THIS highlight"), but new
    /// page-level favoriting goes through `kind='favorite'` rows
    /// instead. The favorites-list query unions both shapes so no
    /// historical data is lost.
    pub is_favorite: bool,
    /// Per-user freeform tag list. Empty array (not NULL) when unset,
    /// so `tags @> ARRAY[...]` filter queries don't need null guards.
    pub tags: Vec<String>,
    /// `{ x, y, w, h, shape }` — rect dims as 0–100 percent floats
    /// against the page's natural pixel dims; `shape ∈ rect | text |
    /// image`. NULL for whole-page markers.
    #[sea_orm(nullable)]
    pub region: Option<Json>,
    /// `{ text?, image_hash?, ocr_confidence? }` — populated by M5's
    /// client-side OCR / SHA-256 hashing for `shape='text'|'image'`
    /// highlights.
    #[sea_orm(nullable)]
    pub selection: Option<Json>,
    /// Markdown body. Required when `kind='note'`. Capped at 10 KB by
    /// the `markers_body_size_chk` constraint.
    #[sea_orm(nullable)]
    pub body: Option<String>,
    /// Palette name (`yellow | green | blue | red | violet`) or a
    /// `#RRGGBB` / `#RRGGBBAA` hex; NULL for "no colour". Validated by the
    /// markers API and bounded by the `markers_color_chk` CHECK
    /// (`m20271101_000002_marker_color_check`).
    #[sea_orm(nullable)]
    pub color: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// "Hide this event from the reading-log feed." User-controlled
    /// per-row flag set by `POST /me/reading-log/hide`. Reading-log
    /// queries filter `hidden_from_log = false` by default; users can
    /// opt in to seeing hidden rows via `?include_hidden=true` so they
    /// can audit / unhide. The marker itself stays available on the
    /// reader overlay + `/bookmarks` regardless of this flag — hiding
    /// only affects activity-feed visibility.
    pub hidden_from_log: bool,
    /// Hex BLAKE3 of the page image this marker was created on (WP-6.2).
    /// On a rescan that finds new archive bytes, `page_index` is
    /// re-resolved to wherever this image now sits; NULL for markers
    /// written before page hashing (ordinal-only, WP-1.2 remap).
    #[sea_orm(nullable)]
    pub page_hash: Option<String>,
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
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
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
                (MarkerKind::Bookmark, "bookmark"),
                (MarkerKind::Note, "note"),
                (MarkerKind::Favorite, "favorite"),
                (MarkerKind::Highlight, "highlight"),
            ],
            MarkerKind::as_str,
            MarkerKind::parse,
            &["Bookmark", "bookmarks", "star", "closing_note", ""],
        );
    }
}
