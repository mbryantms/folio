use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "auth_sessions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,

    /// SHA-256 of the refresh token; raw token never persisted.
    pub refresh_token_hash: String,

    pub created_at: DateTimeWithTimeZone,
    pub last_used_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,

    #[sea_orm(nullable)]
    pub user_agent: Option<String>,
    #[sea_orm(nullable)]
    pub ip: Option<String>,

    #[sea_orm(nullable)]
    pub revoked_at: Option<DateTimeWithTimeZone>,

    /// Stored OIDC `id_token` (compact JWT) used as the `id_token_hint`
    /// parameter to the issuer's `end_session_endpoint` on logout.
    /// `None` for local sessions and for OIDC sessions issued before
    /// the hint column was added.
    #[sea_orm(nullable)]
    pub id_token_hint: Option<String>,
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
