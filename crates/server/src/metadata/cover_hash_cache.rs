//! Search-time cover perceptual-hash cache (WP-2.9, audit DI-16).
//!
//! The matcher's primary discriminant is cover pHash, so every search
//! used to download + decode up to 25 candidate covers per provider —
//! and a second search for the same series did it all again. The three
//! hashes are a pure function of the image bytes and provider cover
//! URLs change when the cover changes, so caching by URL for
//! [`TTL`] turns a repeat search into zero cover downloads.
//!
//! Only successful decodes are cached; a failed fetch or decode writes
//! nothing so a CDN blip doesn't pin a `None` for a month.

use chrono::{DateTime, Duration, Utc};
use entity::metadata_cover_hash;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

/// How long a cached hash is trusted. Provider covers are effectively
/// immutable per URL; 30 days bounds the table without ever really
/// re-downloading a live cover in practice.
pub const TTL: Duration = Duration::days(30);

/// The three hashes a cover decodes to. Mirrors
/// [`crate::metadata::phash::all_hashes`]'s tuple, named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverHashes {
    pub phash: i64,
    pub dhash: i64,
    pub ahash: i64,
}

/// Cached hashes for `url`, or `None` when missing / older than [`TTL`].
pub async fn get<C: ConnectionTrait>(
    db: &C,
    url: &str,
) -> Result<Option<CoverHashes>, sea_orm::DbErr> {
    let Some(row) = metadata_cover_hash::Entity::find_by_id(url.to_owned())
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let fetched: DateTime<Utc> = row.fetched_at.with_timezone(&Utc);
    if Utc::now() - fetched > TTL {
        return Ok(None);
    }
    Ok(Some(CoverHashes {
        phash: row.phash,
        dhash: row.dhash,
        ahash: row.ahash,
    }))
}

/// Upsert the hashes for `url`, restarting its TTL.
pub async fn put<C: ConnectionTrait>(
    db: &C,
    url: &str,
    hashes: CoverHashes,
) -> Result<(), sea_orm::DbErr> {
    let am = metadata_cover_hash::ActiveModel {
        url: Set(url.to_owned()),
        phash: Set(hashes.phash),
        dhash: Set(hashes.dhash),
        ahash: Set(hashes.ahash),
        fetched_at: Set(Utc::now().into()),
    };
    metadata_cover_hash::Entity::insert(am)
        .on_conflict(
            OnConflict::column(metadata_cover_hash::Column::Url)
                .update_columns([
                    metadata_cover_hash::Column::Phash,
                    metadata_cover_hash::Column::Dhash,
                    metadata_cover_hash::Column::Ahash,
                    metadata_cover_hash::Column::FetchedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Drop rows past [`TTL`]. Returns the row count removed. Cheap enough
/// to run from the nightly cache sweep; the `fetched_at` index makes it
/// a range delete.
pub async fn sweep_expired<C: ConnectionTrait>(db: &C) -> Result<u64, sea_orm::DbErr> {
    let cutoff: sea_orm::prelude::DateTimeWithTimeZone = (Utc::now() - TTL).into();
    let res = metadata_cover_hash::Entity::delete_many()
        .filter(metadata_cover_hash::Column::FetchedAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}
