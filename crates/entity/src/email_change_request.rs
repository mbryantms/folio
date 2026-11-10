//! A pending email change (`PATCH /me/account` with a new address while
//! mail delivery is configured). One row per user; the confirmation link's
//! token embeds `token_id`.
//! A cancelled request keeps its row (expired) so its hourly count holds.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "email_change_requests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub token_id: Uuid,
    #[sea_orm(unique)]
    pub user_id: Uuid,
    pub new_email: String,
    pub token_hash: String,
    pub expires_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub consumed_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    /// `users.token_version` when the request was made. The link works only
    /// while the account's is unchanged (password change or reset,
    /// sign-out-everywhere and admin disable all bump it).
    pub token_version: i64,
    /// Start of the hourly window `request_count` counts in. Carried over
    /// when a new request replaces this one.
    pub window_started_at: DateTimeWithTimeZone,
    /// Requests made in the window, this one included.
    pub request_count: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
