//! Response cache for normalized provider payloads.
//!
//! Stores `GenericMetadata` JSON keyed by (provider, entity,
//! external_id) so repeat fetches inside the TTL window avoid burning
//! quota. ComicVine encourages caching; Metron's response shape is
//! stable enough that caching is safe.
//!
//! TTLs are policy (`metadata.cache_ttl_hours.{issues,series,publishers,...}`
//! settings) — defaults: issues=24h, series=168h, publishers/people/etc=720h.
//! TTL is checked at read time; expired rows are treated as misses and
//! overwritten on the next fetch. A nightly cleanup job (M4) sweeps
//! rows older than `max(TTLs) * 2` to bound table growth.

use crate::metadata::identifier::Source;
use crate::metadata::provider::GenericMetadata;
use chrono::{DateTime, Duration, Utc};
use entity::metadata_cache;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

/// Logical entity types that get cached separately. Keys map 1:1 to
/// the `entity` column on `metadata_cache` and the settings-registry
/// TTL keys.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CacheEntity {
    Series,
    Issue,
    Publisher,
    Person,
    Character,
    Team,
    StoryArc,
    Location,
    Concept,
    Object,
    Imprint,
    Universe,
}

impl CacheEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheEntity::Series => "series",
            CacheEntity::Issue => "issue",
            CacheEntity::Publisher => "publisher",
            CacheEntity::Person => "person",
            CacheEntity::Character => "character",
            CacheEntity::Team => "team",
            CacheEntity::StoryArc => "story_arc",
            CacheEntity::Location => "location",
            CacheEntity::Concept => "concept",
            CacheEntity::Object => "object",
            CacheEntity::Imprint => "imprint",
            CacheEntity::Universe => "universe",
        }
    }

    /// Default TTL used when no `metadata.cache_ttl_hours.<key>`
    /// setting has been written. Matches the values documented in the
    /// metadata-providers-1.0 plan.
    pub fn default_ttl(self) -> Duration {
        match self {
            CacheEntity::Issue => Duration::hours(24),
            CacheEntity::Series => Duration::hours(168),
            _ => Duration::hours(720),
        }
    }
}

/// Read a cached payload — returns `None` if missing OR stale.
pub async fn get<C: ConnectionTrait>(
    db: &C,
    provider: Source,
    entity: CacheEntity,
    external_id: &str,
    ttl: Duration,
) -> Result<Option<GenericMetadata>, sea_orm::DbErr> {
    let Some(row) = metadata_cache::Entity::find_by_id((
        provider.as_str().to_string(),
        entity.as_str().to_string(),
        external_id.to_string(),
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };

    let fetched: DateTime<Utc> = row.fetched_at.with_timezone(&Utc);
    if Utc::now() - fetched > ttl {
        return Ok(None);
    }

    match serde_json::from_value::<GenericMetadata>(row.payload) {
        Ok(m) => Ok(Some(m)),
        Err(e) => {
            // Schema drift — log and treat as miss so the next fetch
            // overwrites with the current shape.
            tracing::warn!(
                provider = provider.as_str(),
                entity = entity.as_str(),
                external_id,
                error = %e,
                "metadata_cache payload failed to deserialize; treating as miss"
            );
            Ok(None)
        }
    }
}

/// Upsert a freshly-fetched payload.
pub async fn put<C: ConnectionTrait>(
    db: &C,
    provider: Source,
    entity: CacheEntity,
    external_id: &str,
    payload: &GenericMetadata,
) -> Result<(), sea_orm::DbErr> {
    let json = serde_json::to_value(payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize GenericMetadata: {e}")))?;
    let am = metadata_cache::ActiveModel {
        provider: Set(provider.as_str().to_string()),
        entity: Set(entity.as_str().to_string()),
        external_id: Set(external_id.to_string()),
        payload: Set(json),
        fetched_at: Set(Utc::now().into()),
    };
    metadata_cache::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([
                metadata_cache::Column::Provider,
                metadata_cache::Column::Entity,
                metadata_cache::Column::ExternalId,
            ])
            .update_columns([
                metadata_cache::Column::Payload,
                metadata_cache::Column::FetchedAt,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Delete every cached payload for a provider — used when the operator
/// rotates the API key, so the next fetch goes live.
pub async fn purge_provider<C: ConnectionTrait>(
    db: &C,
    provider: Source,
) -> Result<u64, sea_orm::DbErr> {
    let res = metadata_cache::Entity::delete_many()
        .filter(metadata_cache::Column::Provider.eq(provider.as_str()))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ttls_match_plan() {
        assert_eq!(CacheEntity::Issue.default_ttl(), Duration::hours(24));
        assert_eq!(CacheEntity::Series.default_ttl(), Duration::hours(168));
        assert_eq!(CacheEntity::Publisher.default_ttl(), Duration::hours(720));
        assert_eq!(CacheEntity::Character.default_ttl(), Duration::hours(720));
    }

    #[test]
    fn cache_entity_strings_are_snake_case() {
        assert_eq!(CacheEntity::StoryArc.as_str(), "story_arc");
        assert_eq!(CacheEntity::Person.as_str(), "person");
    }
}
