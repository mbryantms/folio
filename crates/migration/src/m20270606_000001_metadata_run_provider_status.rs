//! Per-provider search bookkeeping on `metadata_run` (provider-complete
//! search).
//!
//! A search used to "fall through" a quota-denied provider: the run
//! finalized with whatever the other providers returned and the denied
//! provider was never asked again for that run. Two columns change that:
//!
//! - `provider_status` — one entry per provider the run was started with:
//!   `pending` (not asked yet), `answered` (searched; `candidates` says how
//!   many it produced), `quota` (denied by the local bucket, owed a retry)
//!   or `failed` (hard error). Survives finalize so the Review queue and the
//!   match dialog can say whether a match covers every provider.
//! - `partial_results` — while a run is parked `awaiting_quota`, the ranked
//!   candidates (+ lookup notes) the answering providers already produced.
//!   The resume sweep re-runs only the owed providers on the same run and
//!   merges into this stash; cleared on finalize.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum MetadataRun {
    Table,
    ProviderStatus,
    PartialResults,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataRun::Table)
                    .add_column(
                        ColumnDef::new(MetadataRun::ProviderStatus)
                            .json_binary()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(MetadataRun::PartialResults)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataRun::Table)
                    .drop_column(MetadataRun::ProviderStatus)
                    .drop_column(MetadataRun::PartialResults)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
