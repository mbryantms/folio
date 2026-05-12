use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum AuditLog {
    Table,
    Id,
    ActorId,
    ActorType,
    Action,
    TargetType,
    TargetId,
    Payload,
    Ip,
    UserAgent,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(AuditLog::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(AuditLog::ActorId).uuid().not_null())
                    .col(ColumnDef::new(AuditLog::ActorType).text().not_null())
                    .col(ColumnDef::new(AuditLog::Action).text().not_null())
                    .col(ColumnDef::new(AuditLog::TargetType).text().null())
                    .col(ColumnDef::new(AuditLog::TargetId).text().null())
                    .col(
                        ColumnDef::new(AuditLog::Payload)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(AuditLog::Ip).text().null())
                    .col(ColumnDef::new(AuditLog::UserAgent).text().null())
                    .col(
                        ColumnDef::new(AuditLog::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // Common access patterns: per-actor history, per-target history, recent activity.
        for (name, col_a, col_b) in [
            (
                "audit_log_actor_idx",
                AuditLog::ActorId,
                AuditLog::CreatedAt,
            ),
            (
                "audit_log_target_idx",
                AuditLog::TargetId,
                AuditLog::CreatedAt,
            ),
            (
                "audit_log_action_idx",
                AuditLog::Action,
                AuditLog::CreatedAt,
            ),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(AuditLog::Table)
                        .col(col_a)
                        .col(col_b)
                        .to_owned(),
                )
                .await?;
        }

        // Append-only enforcement: revoke UPDATE / DELETE at the role level happens in deploy.
        // We add a row-level NOTICE here so a future migration that needs to alter audit_log
        // is forced to think about it.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AuditLog::Table).to_owned())
            .await
    }
}
