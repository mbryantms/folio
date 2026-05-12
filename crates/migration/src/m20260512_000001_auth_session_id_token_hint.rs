//! Auth-hardening M6: persist the OIDC `id_token` on each session row so we
//! can pass it as `id_token_hint` to the issuer's `end_session_endpoint`
//! during RP-initiated logout. Without it, RP-initiated logout would have
//! to silently fall back to local-only revoke, leaving a stranded session
//! at the IdP that surveys "Are you really logging out of Folio?" prompts
//! never reach.
//!
//! Nullable: local-auth sessions never set it, and existing OIDC sessions
//! at migration time were issued without one (their `id_token` is gone).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum AuthSessions {
    Table,
    IdTokenHint,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AuthSessions::Table)
                    .add_column(ColumnDef::new(AuthSessions::IdTokenHint).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AuthSessions::Table)
                    .drop_column(AuthSessions::IdTokenHint)
                    .to_owned(),
            )
            .await
    }
}
