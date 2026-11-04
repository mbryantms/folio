//! Visual effect preferences (folio-flair-1.0, phase 1).
//!
//! A handful of decorative effects (cover tilt, page scrub on hover, hero
//! backdrop, page curl, ...) are a matter of taste, so each one is a
//! per-account on/off switch under Settings > Theme > Effects. They live in
//! one JSON object, `users.effects JSONB NOT NULL DEFAULT '{}'`, keyed by
//! effect. A missing key means "use the web's built-in default", so adding
//! an effect later needs no migration. The server validates keys and value
//! types on write (`auth::preferences::EffectPrefs`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS effects JSONB \
             NOT NULL DEFAULT '{}'::jsonb",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_effects_object_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE users ADD CONSTRAINT users_effects_object_check \
             CHECK (jsonb_typeof(effects) = 'object')",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_effects_object_check",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE users DROP COLUMN IF EXISTS effects")
            .await?;
        Ok(())
    }
}
