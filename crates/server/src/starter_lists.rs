//! Starter reading lists for the first-run tour (cover-stacks-and-onboarding-tour-1.0, PR C).
//!
//! A new reader's question is "where do I start?", so the tour ends on a
//! short shelf of community-catalog reading lists that are good *entry
//! points*: the run people point newcomers to, not the longest or most
//! complete order. Length is deliberately not a criterion.
//!
//! - [`MANIFEST`] ships the curated picks (paths in the bundled DieselTech
//!   catalog source). The server admin edits the set per install through
//!   `starter_list_overrides`: hide a shipped pick, or add any other
//!   catalog file with their own blurb.
//! - Coverage — how much of a list this library holds — runs the normal
//!   CBL matcher over the catalog file *without importing it*, and caches
//!   the matched issue ids in `starter_list_coverage`. Ids, not counts, so
//!   each caller's number is filtered by their own library grants. Rows go
//!   stale after [`COVERAGE_TTL`] (new scans can add matches) and are
//!   recomputed in the background, one list at a time.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use chrono::{Duration, Utc};
use entity::catalog_source;
use sea_orm::{ConnectionTrait, EntityTrait, Statement, Value};
use uuid::Uuid;

use crate::cbl::{catalog, matcher};
use crate::state::AppState;

/// The bundled DieselTech catalog source (seeded by
/// `m20261206_000001_cbl_backend`).
pub const DIESELTECH_SOURCE_ID: Uuid = Uuid::from_u128(0x1001);

/// Recompute coverage this often; scans in between can add matches.
pub const COVERAGE_TTL: Duration = Duration::hours(24);
/// A failed fetch/parse is retried sooner than a good result goes stale.
pub const COVERAGE_ERROR_TTL: Duration = Duration::hours(1);

#[derive(Debug, Clone, Copy)]
pub struct ShippedList {
    pub path: &'static str,
    pub title: &'static str,
    pub publisher: &'static str,
    pub reason: &'static str,
}

/// Curated starter lists, in display order within each publisher. Picks
/// follow the community's usual "where do I start?" answers.
pub const MANIFEST: &[ShippedList] = &[
    ShippedList {
        path: "Marvel/Creator Runs/[Marvel] [2012-2015] Hawkeye by Fraction and Aja.cbl",
        title: "Hawkeye by Fraction & Aja",
        publisher: "Marvel",
        reason: "Clint Barton on his days off. Funny, gorgeous and self-contained, so no homework.",
    },
    ShippedList {
        path: "Marvel/Teams/X-Men/Age of Krakoa/Age of Krakoa - Eras Lists/[X-Men Krakoa 0.1] House of X Powers of X.cbl",
        title: "House of X / Powers of X",
        publisher: "Marvel",
        reason: "The 2019 relaunch that rebuilt the X-Men from scratch. The usual answer to \"where do I start with X-Men?\"",
    },
    ShippedList {
        path: "Marvel/Creator Runs/[Marvel] [1962-1966] Spider-Man by Stan Lee & Steve Ditko.cbl",
        title: "Spider-Man by Lee & Ditko",
        publisher: "Marvel",
        reason: "Where Spider-Man begins. The original run still reads quickly, and everything after builds on it.",
    },
    ShippedList {
        path: "Marvel/Creator Runs/[Marvel] [2004-2013] Captain America by Ed Brubaker.cbl",
        title: "Captain America by Ed Brubaker",
        publisher: "Marvel",
        reason: "A spy thriller that brought back the Winter Soldier. Starts fresh at issue #1.",
    },
    ShippedList {
        path: "Marvel/Creator Runs/[Marvel] [2012-2019] Thor by Jason Aaron.cbl",
        title: "Thor by Jason Aaron",
        publisher: "Marvel",
        reason: "One seven-year saga from God of Thunder to War of the Realms, with a clear beginning and end.",
    },
    ShippedList {
        path: "Marvel/Characters/Ms Marvel/[Marvel] Kamala Khan no events (WEB-CBRO).cbl",
        title: "Ms. Marvel (Kamala Khan)",
        publisher: "Marvel",
        reason: "A teenager from Jersey City becomes a hero. Warm, funny, and written for new readers.",
    },
    ShippedList {
        path: "DC/Characters/Batman/Batman - Modern Age/[DC] Batman Modern Age - Part 01 Year One [1987-2009].cbl",
        title: "Batman: Year One and the early years",
        publisher: "DC",
        reason: "Modern Batman starts with Year One, then moves on to the classic early cases.",
    },
    ShippedList {
        path: "DC/Creator Runs/[DC] [2006-2008] All-Star Superman by Grant Morrison.cbl",
        title: "All-Star Superman",
        publisher: "DC",
        reason: "Superman's whole legend in 12 issues, outside continuity. The most common answer for Superman.",
    },
    ShippedList {
        path: "DC/Creator Runs/[DC] [2011-2021] Batman by Scott Snyder.cbl",
        title: "Batman by Scott Snyder",
        publisher: "DC",
        reason: "Begins with The Court of Owls, a clean 2011 starting point for Batman.",
    },
    ShippedList {
        path: "DC/Creator Runs/[DC] [2004-2013] Green Lantern by Geoff Johns.cbl",
        title: "Green Lantern by Geoff Johns",
        publisher: "DC",
        reason: "Starts with Rebirth and builds to Blackest Night. The run that made Green Lantern a headliner.",
    },
    ShippedList {
        path: "DC/Creator Runs/[DC] [2017-2019] Mister Miracle by Tom King.cbl",
        title: "Mister Miracle",
        publisher: "DC",
        reason: "A 12-issue story that needs no background, often recommended to people who don't read superheroes.",
    },
    ShippedList {
        path: "Image/Master Reading Order/Invincible Universe/[Image] Invincible Universe (WEB-KCV).cbl",
        title: "Invincible",
        publisher: "Image",
        reason: "A complete superhero universe with a real ending. Starts at Invincible #1.",
    },
    ShippedList {
        path: "Dark Horse/Characters/Hellboy/[Dark Horse] Hellboy (Mignolaverse) TPB Reading Order.cbl",
        title: "Hellboy",
        publisher: "Dark Horse",
        reason: "Folklore, monsters and Mike Mignola's art, starting with Seed of Destruction.",
    },
    ShippedList {
        path: "Other/unsorted/Locke and Key.cbl",
        title: "Locke & Key",
        publisher: "IDW",
        reason: "A finished horror-fantasy series about a house full of magic keys.",
    },
    ShippedList {
        path: "Vertigo/Master Reading Order/Fables/[Vertigo] [2002-Present] Fables (CBRO).cbl",
        title: "Fables",
        publisher: "Vertigo",
        reason: "Fairy-tale characters in exile in New York. A long run that opens with a self-contained mystery.",
    },
];

pub fn shipped(source_id: Uuid, path: &str) -> Option<&'static ShippedList> {
    if source_id != DIESELTECH_SOURCE_ID {
        return None;
    }
    MANIFEST.iter().find(|m| m.path == path)
}

/// Lists whose coverage is being computed right now (per process), so
/// overlapping requests never start a second run for the same file.
fn inflight() -> &'static Mutex<HashSet<(Uuid, String)>> {
    static SET: OnceLock<Mutex<HashSet<(Uuid, String)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Start a background coverage run for `keys` (skipping any already in
/// flight). One task, one list at a time: the matcher's fallback query
/// is per entry, so a burst of parallel runs would hog the pool.
pub fn spawn_coverage(app: &AppState, keys: Vec<(Uuid, String)>) {
    let mut todo = Vec::new();
    {
        let mut set = inflight().lock().expect("inflight lock");
        for k in keys {
            if set.insert(k.clone()) {
                todo.push(k);
            }
        }
    }
    if todo.is_empty() {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        for (source_id, path) in todo {
            if let Err(e) = compute_and_store(&app, source_id, &path).await {
                tracing::warn!(error = %e, %path, "starter_lists: coverage run failed");
            }
            inflight()
                .lock()
                .expect("inflight lock")
                .remove(&(source_id, path));
        }
    });
}

/// Fetch, parse and match one catalog file; store the outcome (or the
/// error) in `starter_list_coverage`.
pub async fn compute_and_store(
    app: &AppState,
    source_id: Uuid,
    path: &str,
) -> Result<(), sea_orm::DbErr> {
    let outcome = compute(app, source_id, path).await;
    let (total, ids, err): (i32, Vec<String>, Option<String>) = match outcome {
        Ok((total, ids)) => (total, ids, None),
        Err(e) => (0, Vec::new(), Some(e)),
    };
    store(&app.db, source_id, path, total, &ids, err.as_deref()).await
}

async fn compute(
    app: &AppState,
    source_id: Uuid,
    path: &str,
) -> Result<(i32, Vec<String>), String> {
    let source = catalog_source::Entity::find_by_id(source_id)
        .one(&app.db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "catalog source not found".to_owned())?;
    let blob = catalog::fetch_blob(&app.db, &source, path, false)
        .await
        .map_err(|e| e.to_string())?;
    let parsed = parsers::cbl::parse(&blob.bytes).map_err(|e| e.to_string())?;
    let to_match: Vec<matcher::EntryToMatch> = parsed
        .books
        .iter()
        .map(|b| matcher::EntryToMatch {
            series_name: b.series.clone(),
            number: b.number.clone(),
            volume: b.volume.clone(),
            cv_issue_id: b.comicvine_issue_id(),
            metron_issue_id: b.metron_issue_id(),
        })
        .collect();
    let outcomes = matcher::match_entries(&app.db, &to_match)
        .await
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let ids = outcomes
        .into_iter()
        .filter(|o| o.status == matcher::MatchStatus::Matched)
        .filter_map(|o| o.issue_id)
        .filter(|id| seen.insert(id.clone()))
        .collect();
    Ok((i32::try_from(to_match.len()).unwrap_or(i32::MAX), ids))
}

/// Upsert one coverage row. Public so tests can seed coverage without a
/// network fetch.
pub async fn store<C: ConnectionTrait>(
    db: &C,
    source_id: Uuid,
    path: &str,
    total: i32,
    matched_issue_ids: &[String],
    error: Option<&str>,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "INSERT INTO starter_list_coverage \
           (catalog_source_id, catalog_path, total, matched_issue_ids, error, computed_at) \
         VALUES ($1, $2, $3, $4, $5, now()) \
         ON CONFLICT (catalog_source_id, catalog_path) DO UPDATE SET \
           total = EXCLUDED.total, matched_issue_ids = EXCLUDED.matched_issue_ids, \
           error = EXCLUDED.error, computed_at = EXCLUDED.computed_at",
        [
            source_id.into(),
            path.into(),
            total.into(),
            Value::Array(
                sea_orm::sea_query::ArrayType::String,
                Some(Box::new(
                    matched_issue_ids
                        .iter()
                        .map(|s| Value::String(Some(s.clone())))
                        .collect(),
                )),
            ),
            error.map(str::to_owned).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Is a row computed at `computed_at` (with or without an error) due for
/// a rerun?
pub fn is_stale(computed_at: chrono::DateTime<chrono::FixedOffset>, errored: bool) -> bool {
    let ttl = if errored {
        COVERAGE_ERROR_TTL
    } else {
        COVERAGE_TTL
    };
    Utc::now().fixed_offset() - computed_at > ttl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_are_unique_cbl_files() {
        let mut seen = HashSet::new();
        for m in MANIFEST {
            assert!(m.path.ends_with(".cbl"), "{}", m.path);
            assert!(seen.insert(m.path), "duplicate {}", m.path);
            assert!(!m.reason.is_empty() && !m.title.is_empty());
        }
    }

    #[test]
    fn dieseltech_id_matches_the_seeded_source() {
        assert_eq!(
            DIESELTECH_SOURCE_ID.to_string(),
            "00000000-0000-0000-0000-000000001001"
        );
    }
}
