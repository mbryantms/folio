//! User-pages data helpers.
//!
//! M1 of multi-page rails introduces `user_page` with a per-user system
//! row. Until the page-CRUD HTTP surface lands in M2, the only consumer
//! is the existing pin code in [`crate::api::saved_views`] /
//! [`crate::api::collections`] — those handlers still operate as
//! "the user's home" but now have to write `page_id` onto every
//! `user_view_pin` row. This module exposes the helper they need:
//! [`system_page_id`], which resolves the user's auto-created Home page
//! and lazy-creates it on first access for users who somehow lack one
//! (the M1 migration seeds every existing user; this guards new sign-ups
//! and any user-creation paths that get added later).

use entity::user_page;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
};
use uuid::Uuid;

/// Resolve the user's system "Home" page id, creating it on first access
/// if missing. Race-safe: concurrent inserts collide on the
/// `user_page_system_idx` partial unique index; the loser silently re-
/// reads the winner's row.
pub async fn system_page_id<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Uuid, DbErr> {
    if let Some(row) = fetch_system_page(db, user_id).await? {
        return Ok(row.id);
    }

    let am = user_page::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        name: Set("Home".into()),
        slug: Set("home".into()),
        is_system: Set(true),
        position: Set(0),
        description: Set(None),
        created_at: NotSet,
        updated_at: NotSet,
    };
    // Either succeeds, or fails with a unique violation when a concurrent
    // caller beat us. Either outcome is fine — re-fetch and use whatever
    // row is there.
    let _ = user_page::Entity::insert(am).exec(db).await;

    fetch_system_page(db, user_id)
        .await?
        .map(|m| m.id)
        .ok_or_else(|| DbErr::Custom(format!("system page resolution failed for user {user_id}")))
}

async fn fetch_system_page<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Option<user_page::Model>, DbErr> {
    user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user_id))
        .filter(user_page::Column::IsSystem.eq(true))
        .one(db)
        .await
}
