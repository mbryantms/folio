//! An integration's ingest token (migration `m20271119`), stored as the
//! SHA-256 digest of the plaintext. Not a user: it opens exactly one
//! endpoint, named by `name`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// `name` of the Mylar snapshot ingest token.
pub const MYLAR: &str = "mylar";

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "integration_tokens")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    #[serde(skip)]
    pub digest: Vec<u8>,
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub last_used_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub last_used_ip: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
