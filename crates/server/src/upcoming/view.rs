//! Loading what [`super::reconcile`] needs for one viewer and one span
//! of dates.
//!
//! **Cost.** A fixed number of statements however many releases the span
//! holds: every lookup is one `IN (…)` over the ids on hand, and the
//! watchlist is asked only about those ids, never read whole. A
//! query-count test (`tests/upcoming_query_count.rs`) holds that.
//!
//! **Who sees Mylar data.** Mylar's watchlist covers the operator's whole
//! collection, and Folio answers a hidden series exactly as it answers a
//! missing one. So nothing from the `mylar_*` tables is loaded unless
//! [`access::sees_whole_library`] holds for the viewer (an admin, or a
//! user granted every library with no age-rating cap), the integration is
//! on, and the snapshot is not expired. Everyone else gets the calendar
//! with library links for the series they can see.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use entity::{
    external_id, issue, mylar_pull, mylar_wanted, mylar_watch, release_calendar_issue,
    release_calendar_series, series, series_provider_range,
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::mylar::{self, Freshness};
use super::reconcile::{
    self, CalendarRow, Entry, Facts, LocalIssue, MylarStatus, PAIR_WINDOW_DAYS, PullLine,
    SeriesLink,
};
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::metadata::matcher::canonical_issue_number;
use crate::state::AppState;

/// Mylar's snapshot as the viewer may know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MylarSeen {
    pub received_at: DateTime<FixedOffset>,
    pub freshness: Freshness,
}

/// What [`load`] returns.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Every release in the span, ordered by date, name and number.
    pub entries: Vec<Entry>,
    /// `Some` only when the viewer may see Mylar data and there is a
    /// snapshot that has not expired.
    pub mylar: Option<MylarSeen>,
    /// The viewer has an age-rating cap on some library. Such a viewer is
    /// only shown releases of series they can already see.
    pub capped: bool,
}

/// Decide whether this viewer sees Mylar data, and how fresh it is.
async fn mylar_for(
    app: &AppState,
    acl: &VisibleLibraries,
    now: DateTime<FixedOffset>,
) -> Result<Option<MylarSeen>, DbErr> {
    let cfg = app.cfg();
    if !cfg.integrations_mylar_enabled || !access::sees_whole_library(&app.db, acl).await {
        return Ok(None);
    }
    let Some(head) = mylar::snapshot_row(&app.db).await? else {
        return Ok(None);
    };
    let freshness = mylar::freshness(
        head.received_at,
        now,
        cfg.integrations_mylar_stale_after_minutes,
    );
    Ok((freshness != Freshness::Expired).then_some(MylarSeen {
        received_at: head.received_at,
        freshness,
    }))
}

async fn calendar_rows(
    db: &DatabaseConnection,
    filter: Condition,
) -> Result<Vec<CalendarRow>, DbErr> {
    let rows = release_calendar_issue::Entity::find()
        .find_also_related(release_calendar_series::Entity)
        .filter(filter)
        .order_by_asc(release_calendar_issue::Column::StoreDate)
        .order_by_asc(release_calendar_issue::Column::MetronIssueId)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(i, s)| {
            let s = s?;
            Some(CalendarRow {
                metron_issue_id: i.metron_issue_id,
                metron_series_id: i.metron_series_id,
                series_name: s.name,
                volume: s.volume,
                year_began: s.year_began,
                publisher: s.publisher,
                cv_series_id: s.cv_series_id,
                number: i.number,
                store_date: i.store_date,
                cover_date: i.cover_date,
                image_url: i.image_url,
            })
        })
        .collect())
}

fn pull_line(m: mylar_pull::Model) -> PullLine {
    PullLine {
        id: m.id,
        ship_date: m.ship_date,
        publisher: m.publisher,
        series_name: m.series_name,
        issue_number: m.issue_number,
        status: m.status,
        cv_series_id: m.cv_series_id,
        cv_issue_id: m.cv_issue_id,
        series_year: m.series_year,
    }
}

#[derive(Debug, FromQueryResult)]
struct SeriesRow {
    id: Uuid,
    slug: String,
    name: String,
    library_id: Uuid,
    age_rating: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct IssueCount {
    series_id: Uuid,
    n: i64,
}

#[derive(Debug, FromQueryResult)]
struct IssueNumberRow {
    series_id: Uuid,
    number_raw: Option<String>,
    year: Option<i32>,
    month: Option<i32>,
}

fn provider_filter<C: ColumnTrait>(
    source: C,
    id: C,
    cv: &[String],
    metron: &[String],
) -> Option<Condition> {
    let mut any = Condition::any();
    let mut used = false;
    if !cv.is_empty() {
        any = any.add(
            Condition::all()
                .add(source.eq("comicvine"))
                .add(id.is_in(cv.iter().cloned())),
        );
        used = true;
    }
    if !metron.is_empty() {
        any = any.add(
            Condition::all()
                .add(source.eq("metron"))
                .add(id.is_in(metron.iter().cloned())),
        );
        used = true;
    }
    used.then_some(any)
}

/// The viewer's local series for the provider ids on hand: by
/// `external_ids` and by `series_provider_range` (a relaunch one provider
/// files as its own series lives in the range table). When several local
/// series carry one provider id, the one with the most issues is linked.
async fn library_links(
    db: &DatabaseConnection,
    acl: &VisibleLibraries,
    cv_series: &HashSet<i64>,
    metron_series: &HashSet<i64>,
) -> Result<(HashMap<i64, SeriesLink>, HashMap<i64, SeriesLink>), DbErr> {
    let cv: Vec<String> = cv_series.iter().map(i64::to_string).collect();
    let metron: Vec<String> = metron_series.iter().map(i64::to_string).collect();
    // (source, provider id, local series id)
    let mut claims: Vec<(String, i64, Uuid)> = Vec::new();
    if let Some(cond) = provider_filter(
        external_id::Column::Source,
        external_id::Column::ExternalId,
        &cv,
        &metron,
    ) {
        let rows = external_id::Entity::find()
            .filter(external_id::Column::EntityType.eq("series"))
            .filter(cond)
            .all(db)
            .await?;
        claims.extend(rows.into_iter().filter_map(|r| {
            Some((
                r.source,
                r.external_id.parse().ok()?,
                Uuid::parse_str(&r.entity_id).ok()?,
            ))
        }));
    }
    if let Some(cond) = provider_filter(
        series_provider_range::Column::Source,
        series_provider_range::Column::ProviderSeriesId,
        &cv,
        &metron,
    ) {
        let rows = series_provider_range::Entity::find()
            .filter(cond)
            .all(db)
            .await?;
        claims.extend(
            rows.into_iter()
                .filter_map(|r| Some((r.source, r.provider_series_id.parse().ok()?, r.series_id))),
        );
    }
    if claims.is_empty() {
        return Ok((HashMap::new(), HashMap::new()));
    }

    let ids: HashSet<Uuid> = claims.iter().map(|c| c.2).collect();
    let visible: HashMap<Uuid, SeriesLink> = series::Entity::find()
        .select_only()
        .columns([
            series::Column::Id,
            series::Column::Slug,
            series::Column::Name,
            series::Column::LibraryId,
            series::Column::AgeRating,
        ])
        .filter(series::Column::Id.is_in(ids))
        .filter(access::active_series())
        .into_model::<SeriesRow>()
        .all(db)
        .await?
        .into_iter()
        .filter(|s| acl.series_ok(s.library_id, s.age_rating.as_deref()))
        .map(|s| {
            (
                s.id,
                SeriesLink {
                    id: s.id,
                    slug: s.slug,
                    name: s.name,
                },
            )
        })
        .collect();

    // provider id -> visible local series that claim it
    let mut by_cv: HashMap<i64, Vec<Uuid>> = HashMap::new();
    let mut by_metron: HashMap<i64, Vec<Uuid>> = HashMap::new();
    for (source, provider_id, series_id) in claims {
        if !visible.contains_key(&series_id) {
            continue;
        }
        let map = if source == "comicvine" {
            &mut by_cv
        } else {
            &mut by_metron
        };
        let v = map.entry(provider_id).or_default();
        if !v.contains(&series_id) {
            v.push(series_id);
        }
    }
    // Only a provider id claimed by more than one visible series needs
    // the tie-break, so the count query usually does not run.
    let contested: HashSet<Uuid> = by_cv
        .values()
        .chain(by_metron.values())
        .filter(|v| v.len() > 1)
        .flatten()
        .copied()
        .collect();
    let mut sizes: HashMap<Uuid, i64> = HashMap::new();
    if !contested.is_empty() {
        let rows = issue::Entity::find()
            .select_only()
            .column(issue::Column::SeriesId)
            .column_as(issue::Column::Id.count(), "n")
            .filter(issue::Column::SeriesId.is_in(contested))
            .filter(issue::Column::RemovedAt.is_null())
            .group_by(issue::Column::SeriesId)
            .into_model::<IssueCount>()
            .all(db)
            .await?;
        sizes = rows.into_iter().map(|r| (r.series_id, r.n)).collect();
    }
    let pick = |candidates: Vec<Uuid>| -> Option<SeriesLink> {
        candidates
            .into_iter()
            // Most issues first; the id breaks a tie so the answer is stable.
            .max_by_key(|id| (sizes.get(id).copied().unwrap_or(0), std::cmp::Reverse(*id)))
            .and_then(|id| visible.get(&id).cloned())
    };
    let resolve = |m: HashMap<i64, Vec<Uuid>>| -> HashMap<i64, SeriesLink> {
        m.into_iter()
            .filter_map(|(k, v)| Some((k, pick(v)?)))
            .collect()
    };
    Ok((resolve(by_cv), resolve(by_metron)))
}

/// Reconcile `calendar` with what the viewer may see, and return the
/// decorated entries. `pull_span` bounds the pull-list lines considered.
async fn assemble(
    app: &AppState,
    acl: &VisibleLibraries,
    calendar: Vec<CalendarRow>,
    pull_span: (NaiveDate, NaiveDate),
    pull_series: Option<&HashSet<i64>>,
    mylar: Option<MylarSeen>,
) -> Result<Vec<Entry>, DbErr> {
    let db = &app.db;
    let mut pull: Vec<PullLine> = Vec::new();
    if mylar.is_some() {
        let mut q = mylar_pull::Entity::find()
            .filter(mylar_pull::Column::ShipDate.between(pull_span.0, pull_span.1));
        if let Some(only) = pull_series {
            q = q.filter(mylar_pull::Column::CvSeriesId.is_in(only.iter().copied()));
        }
        pull = q
            .order_by_asc(mylar_pull::Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(pull_line)
            .collect();
    }
    let paired = reconcile::pair(&calendar, &pull);

    let cv_series: HashSet<i64> = paired.iter().filter_map(|p| p.cv_series_id).collect();
    let metron_series: HashSet<i64> = paired.iter().filter_map(|p| p.metron_series_id).collect();
    let (by_cv_series, by_metron_series) =
        library_links(db, acl, &cv_series, &metron_series).await?;

    let mut facts = Facts {
        by_cv_series,
        by_metron_series,
        mylar_visible: mylar.is_some(),
        ..Default::default()
    };

    // Owned: only for releases whose series is linked.
    let linked: HashSet<Uuid> = facts
        .by_cv_series
        .values()
        .chain(facts.by_metron_series.values())
        .map(|l| l.id)
        .collect();
    if !linked.is_empty() {
        // By provider issue id first: exact, and independent of dates.
        let metron_issues: Vec<String> = paired
            .iter()
            .filter_map(|p| p.metron_issue_id)
            .map(|i| i.to_string())
            .collect();
        let cv_issues: Vec<String> = paired
            .iter()
            .filter_map(|p| p.cv_issue_id)
            .map(|i| i.to_string())
            .collect();
        if let Some(cond) = provider_filter(
            external_id::Column::Source,
            external_id::Column::ExternalId,
            &cv_issues,
            &metron_issues,
        ) {
            let claims = external_id::Entity::find()
                .filter(external_id::Column::EntityType.eq("issue"))
                .filter(cond)
                .all(db)
                .await?;
            if !claims.is_empty() {
                // Only an issue of a linked (so visible) series counts.
                let present: HashSet<String> = issue::Entity::find()
                    .select_only()
                    .column(issue::Column::Id)
                    .filter(issue::Column::Id.is_in(claims.iter().map(|c| c.entity_id.clone())))
                    .filter(issue::Column::SeriesId.is_in(linked.iter().copied()))
                    .filter(issue::Column::RemovedAt.is_null())
                    .into_tuple::<String>()
                    .all(db)
                    .await?
                    .into_iter()
                    .collect();
                for c in claims {
                    if !present.contains(&c.entity_id) {
                        continue;
                    }
                    let Ok(id) = c.external_id.parse::<i64>() else {
                        continue;
                    };
                    if c.source == "metron" {
                        facts.owned_metron_issues.insert(id);
                    } else {
                        facts.owned_cv_issues.insert(id);
                    }
                }
            }
        }

        // Then by number + cover month. An issue with no cover year can
        // never match that way, so it is not read.
        let earliest = paired
            .iter()
            .map(|p| p.cover_date.unwrap_or(p.date))
            .min()
            .map_or(0, |d| chrono::Datelike::year(&d) - 1);
        let local = issue::Entity::find()
            .select_only()
            .columns([
                issue::Column::SeriesId,
                issue::Column::NumberRaw,
                issue::Column::Year,
                issue::Column::Month,
            ])
            .filter(issue::Column::SeriesId.is_in(linked.iter().copied()))
            .filter(issue::Column::RemovedAt.is_null())
            .filter(issue::Column::Year.gte(earliest))
            .into_model::<IssueNumberRow>()
            .all(db)
            .await?;
        for i in local {
            if let Some(number) = i.number_raw {
                facts
                    .local_issues
                    .entry(i.series_id)
                    .or_default()
                    .push(LocalIssue {
                        number,
                        year: i.year,
                        month: i.month,
                    });
            }
        }
    }

    if mylar.is_some() && !cv_series.is_empty() {
        facts.tracked = mylar_watch::Entity::find()
            .filter(mylar_watch::Column::CvSeriesId.is_in(cv_series.iter().copied()))
            .all(db)
            .await?
            .into_iter()
            .filter(|w| !w.status.eq_ignore_ascii_case("paused"))
            .map(|w| w.cv_series_id)
            .collect();
        let wanted = mylar_wanted::Entity::find()
            .filter(mylar_wanted::Column::CvSeriesId.is_in(cv_series.iter().copied()))
            .all(db)
            .await?;
        for w in wanted {
            let Some(status) = MylarStatus::from_mylar(&w.status) else {
                continue;
            };
            if let Some(issue_id) = w.cv_issue_id {
                facts.wanted_by_issue.insert(issue_id, status);
            }
            if let Some(cv) = w.cv_series_id {
                facts
                    .wanted_by_number
                    .insert((cv, canonical_issue_number(&w.issue_number)), status);
            }
        }
    }

    Ok(paired
        .into_iter()
        .map(|p| reconcile::decorate(p, &facts))
        .collect())
}

/// Every release with a date in `from..=to`, reconciled for `user`.
pub async fn load(
    app: &AppState,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Loaded, DbErr> {
    let acl = access::for_user(app, user).await;
    let now = Utc::now().fixed_offset();
    let mylar = mylar_for(app, &acl, now).await?;
    let calendar = calendar_rows(
        &app.db,
        Condition::all().add(release_calendar_issue::Column::StoreDate.between(from, to)),
    )
    .await?;
    let entries = assemble(app, &acl, calendar, (from, to), None, mylar).await?;
    Ok(Loaded {
        entries,
        mylar,
        capped: acl.has_caps(),
    })
}

/// Publishers with a release in `from..=to`, by name.
pub async fn publishers(
    db: &DatabaseConnection,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<String>, DbErr> {
    let mut names: Vec<String> = release_calendar_series::Entity::find()
        .select_only()
        .column(release_calendar_series::Column::Publisher)
        .distinct()
        .inner_join(release_calendar_issue::Entity)
        .filter(release_calendar_issue::Column::StoreDate.between(from, to))
        .filter(release_calendar_series::Column::Publisher.is_not_null())
        .into_tuple::<Option<String>>()
        .all(db)
        .await?
        .into_iter()
        .flatten()
        .filter(|p| !p.trim().is_empty())
        .collect();
    names.sort_by_key(|p| p.to_lowercase());
    Ok(names)
}

/// The next releases (today or later) of one local series the viewer has
/// already been shown, soonest first.
pub async fn next_for_series(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
    today: NaiveDate,
    limit: usize,
) -> Result<Loaded, DbErr> {
    let db = &app.db;
    let acl = access::for_user(app, user).await;
    let now = Utc::now().fixed_offset();
    let mylar = mylar_for(app, &acl, now).await?;

    // The provider series this local series maps to.
    let ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(external_id::Column::Source.is_in(["comicvine", "metron"]))
        .all(db)
        .await?;
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .filter(series_provider_range::Column::Source.is_in(["comicvine", "metron"]))
        .all(db)
        .await?;
    let mut cv: HashSet<i64> = HashSet::new();
    let mut metron: HashSet<i64> = HashSet::new();
    for (source, id) in ids
        .into_iter()
        .map(|r| (r.source, r.external_id))
        .chain(ranges.into_iter().map(|r| (r.source, r.provider_series_id)))
    {
        let Ok(id) = id.parse::<i64>() else { continue };
        if source == "comicvine" {
            cv.insert(id);
        } else {
            metron.insert(id);
        }
    }
    if cv.is_empty() && metron.is_empty() {
        return Ok(Loaded {
            entries: Vec::new(),
            mylar,
            capped: acl.has_caps(),
        });
    }

    let mut of_series = Condition::any();
    if !cv.is_empty() {
        of_series =
            of_series.add(release_calendar_series::Column::CvSeriesId.is_in(cv.iter().copied()));
    }
    if !metron.is_empty() {
        of_series = of_series
            .add(release_calendar_issue::Column::MetronSeriesId.is_in(metron.iter().copied()));
    }
    let calendar = calendar_rows(
        db,
        Condition::all()
            .add(release_calendar_issue::Column::StoreDate.gte(today))
            .add(of_series),
    )
    .await?;
    let last = calendar.iter().map(|r| r.store_date).max().unwrap_or(today);
    // Pull-list lines for this series only: a line Metron does not list
    // still counts as an upcoming release of it.
    let span = (
        today,
        last.max(today) + Duration::days(PAIR_WINDOW_DAYS * 4),
    );
    let mut entries = assemble(app, &acl, calendar, span, Some(&cv), mylar).await?;
    entries.retain(|e| e.library_series.as_ref().is_some_and(|l| l.id == series_id));
    entries.truncate(limit);
    Ok(Loaded {
        entries,
        mylar,
        capped: acl.has_caps(),
    })
}
