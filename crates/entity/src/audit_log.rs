use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Append-only admin / security audit log (§5.9).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub actor_id: Uuid,

    /// `user` | `app_password` | `system`
    pub actor_type: String,

    /// Dotted: `review.delete`, `app_password.create`, ...
    pub action: String,

    #[sea_orm(nullable)]
    pub target_type: Option<String>,
    #[sea_orm(nullable)]
    pub target_id: Option<String>,

    pub payload: Json,

    #[sea_orm(nullable)]
    pub ip: Option<String>,
    #[sea_orm(nullable)]
    pub user_agent: Option<String>,

    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
