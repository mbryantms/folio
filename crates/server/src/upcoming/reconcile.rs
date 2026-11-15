//! Reconciling the two sources into one list of upcoming releases.
//!
//! Pure functions over rows already read from Postgres: nothing here
//! touches the database or the network, and nothing is materialised, so
//! there is no second copy to drift. [`crate::upcoming::view`] loads the
//! inputs; this module decides what they mean.
//!
//! **Pairing** ([`pair`]) joins Mylar's pull-list lines to Metron's
//! calendar rows, one to one:
//!
//! 1. same ComicVine volume id, same issue number after
//!    [`canonical_issue_number`], dates within [`PAIR_WINDOW_DAYS`];
//! 2. same ComicVine volume id, same Monday–Sunday week, and exactly one
//!    unpaired row on each side (a provider that lumps and one that
//!    splits a relaunch number the same book differently);
//! 3. when either side has no ComicVine id: equal sanitised title, equal
//!    number, dates within the window, and the same start year or the
//!    same publisher.
//!
//! A pull line with no partner still becomes an entry: Metron lists
//! fewer small-publisher books than a pull list does.
//!
//! **Facts** ([`decorate`]) add what Folio and Mylar know: whether the
//! series is in the library, whether the issue is already owned, whether
//! Mylar tracks the series and what it did with the issue. Mylar's status
//! is only ever what Mylar reported. Nothing is inferred.

use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::week_start;
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::title_norm::sanitize_title;

/// Two dates this close can be the same release (a Tuesday on one
/// source, the Wednesday on the other; or a week's slip not yet copied).
pub const PAIR_WINDOW_DAYS: i64 = 7;
/// Owned-by-number needs the local issue's cover month this close to the
/// calendar's. Never number alone: a long series reuses numbers.
pub const OWNED_COVER_MONTHS: i32 = 2;

/// One Metron calendar row with its series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRow {
    pub metron_issue_id: i64,
    pub metron_series_id: i64,
    pub series_name: String,
    pub volume: Option<i32>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    pub cv_series_id: Option<i64>,
    pub number: String,
    pub store_date: NaiveDate,
    pub cover_date: Option<NaiveDate>,
    pub image_url: Option<String>,
}

/// One line of Mylar's pull list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullLine {
    pub id: i64,
    pub ship_date: NaiveDate,
    pub publisher: Option<String>,
    pub series_name: String,
    pub issue_number: String,
    pub status: String,
    pub cv_series_id: Option<i64>,
    pub cv_issue_id: Option<i64>,
    pub series_year: Option<i32>,
}

/// Where an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UpcomingEntrySource)]
pub enum EntrySource {
    /// Metron's calendar only.
    Metron,
    /// Mylar's pull list only (Metron does not list it).
    PullList,
    /// Both, paired.
    Both,
}

/// What Mylar reported for an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UpcomingMylarStatus)]
pub enum MylarStatus {
    Wanted,
    Snatched,
    Downloaded,
    Failed,
}

impl MylarStatus {
    /// Mylar's own word → ours. `Skipped` (and anything unknown) is not
    /// a status worth showing: it means "on the pull list, not tracked".
    pub fn from_mylar(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "wanted" => Some(Self::Wanted),
            "snatched" => Some(Self::Snatched),
            "downloaded" | "archived" | "post-processed" => Some(Self::Downloaded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// The single thing a card says about a release, by precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UpcomingStage)]
pub enum Stage {
    /// Folio already has the issue.
    InLibrary,
    /// Mylar has the file; Folio has not scanned it yet.
    Downloaded,
    /// Mylar sent it to a download client.
    Snatched,
    /// Mylar wants it (or tried and failed).
    Wanted,
    /// The series is on Mylar's watchlist.
    Tracked,
    /// Announced, and nothing more is known.
    Announced,
}

/// A release after pairing, before the library and Mylar facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub source: EntrySource,
    pub series_name: String,
    pub volume: Option<i32>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    pub number: String,
    /// Metron's store date when it has the release, else Mylar's ship date.
    pub date: NaiveDate,
    pub cover_date: Option<NaiveDate>,
    pub image_url: Option<String>,
    pub metron_issue_id: Option<i64>,
    pub metron_series_id: Option<i64>,
    pub cv_series_id: Option<i64>,
    pub cv_issue_id: Option<i64>,
    /// The pull line's id, when there is one.
    pub pull_id: Option<i64>,
    /// Mylar's own status word from the pull line.
    pub pull_status: Option<String>,
}

impl Paired {
    /// Stable for one snapshot + calendar state: the Metron issue id when
    /// there is one, else the pull line's id.
    pub fn key(&self) -> String {
        match (self.metron_issue_id, self.pull_id) {
            (Some(m), _) => format!("m{m}"),
            (None, Some(p)) => format!("p{p}"),
            (None, None) => String::new(),
        }
    }
}

fn close(a: NaiveDate, b: NaiveDate) -> bool {
    (a - b).num_days().abs() <= PAIR_WINDOW_DAYS
}

fn same_text(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (a, b) = (sanitize_title(a), sanitize_title(b));
            !a.is_empty() && a == b
        }
        _ => false,
    }
}

fn merge(row: &CalendarRow, line: &PullLine) -> Paired {
    Paired {
        source: EntrySource::Both,
        series_name: row.series_name.clone(),
        volume: row.volume,
        year_began: row.year_began.or(line.series_year),
        publisher: row.publisher.clone().or_else(|| line.publisher.clone()),
        number: row.number.clone(),
        date: row.store_date,
        cover_date: row.cover_date,
        image_url: row.image_url.clone(),
        metron_issue_id: Some(row.metron_issue_id),
        metron_series_id: Some(row.metron_series_id),
        cv_series_id: row.cv_series_id.or(line.cv_series_id),
        cv_issue_id: line.cv_issue_id,
        pull_id: Some(line.id),
        pull_status: Some(line.status.clone()),
    }
}

fn from_calendar(row: &CalendarRow) -> Paired {
    Paired {
        source: EntrySource::Metron,
        series_name: row.series_name.clone(),
        volume: row.volume,
        year_began: row.year_began,
        publisher: row.publisher.clone(),
        number: row.number.clone(),
        date: row.store_date,
        cover_date: row.cover_date,
        image_url: row.image_url.clone(),
        metron_issue_id: Some(row.metron_issue_id),
        metron_series_id: Some(row.metron_series_id),
        cv_series_id: row.cv_series_id,
        cv_issue_id: None,
        pull_id: None,
        pull_status: None,
    }
}

fn from_pull(line: &PullLine) -> Paired {
    Paired {
        source: EntrySource::PullList,
        series_name: line.series_name.clone(),
        volume: None,
        year_began: line.series_year,
        publisher: line.publisher.clone(),
        number: line.issue_number.clone(),
        date: line.ship_date,
        cover_date: None,
        image_url: None,
        metron_issue_id: None,
        metron_series_id: None,
        cv_series_id: line.cv_series_id,
        cv_issue_id: line.cv_issue_id,
        pull_id: Some(line.id),
        pull_status: Some(line.status.clone()),
    }
}

/// Pair pull-list lines with calendar rows (module docs), then return
/// every release once, ordered by date, series name and number.
pub fn pair(calendar: &[CalendarRow], pull: &[PullLine]) -> Vec<Paired> {
    let numbers: Vec<String> = calendar
        .iter()
        .map(|r| canonical_issue_number(&r.number))
        .collect();
    let line_numbers: Vec<String> = pull
        .iter()
        .map(|l| canonical_issue_number(&l.issue_number))
        .collect();
    // calendar index -> pull index
    let mut partner: Vec<Option<usize>> = vec![None; calendar.len()];
    let mut taken: Vec<bool> = vec![false; pull.len()];

    // Pass 1: ComicVine id + number + dates.
    for (li, line) in pull.iter().enumerate() {
        let Some(cv) = line.cv_series_id else {
            continue;
        };
        let best = calendar
            .iter()
            .enumerate()
            .filter(|(ci, row)| {
                partner[*ci].is_none()
                    && row.cv_series_id == Some(cv)
                    && numbers[*ci] == line_numbers[li]
                    && close(row.store_date, line.ship_date)
            })
            .min_by_key(|(_, row)| {
                (
                    (row.store_date - line.ship_date).num_days().abs(),
                    row.metron_issue_id,
                )
            })
            .map(|(ci, _)| ci);
        if let Some(ci) = best {
            partner[ci] = Some(li);
            taken[li] = true;
        }
    }

    // Pass 2: same ComicVine id and week, one unpaired row on each side.
    let mut open_rows: HashMap<(i64, NaiveDate), Vec<usize>> = HashMap::new();
    for (ci, row) in calendar.iter().enumerate() {
        if partner[ci].is_none()
            && let Some(cv) = row.cv_series_id
        {
            open_rows
                .entry((cv, week_start(row.store_date)))
                .or_default()
                .push(ci);
        }
    }
    let mut open_lines: HashMap<(i64, NaiveDate), Vec<usize>> = HashMap::new();
    for (li, line) in pull.iter().enumerate() {
        if !taken[li]
            && let Some(cv) = line.cv_series_id
        {
            open_lines
                .entry((cv, week_start(line.ship_date)))
                .or_default()
                .push(li);
        }
    }
    for (key, rows) in &open_rows {
        if let ([ci], Some([li])) = (rows.as_slice(), open_lines.get(key).map(Vec::as_slice)) {
            partner[*ci] = Some(*li);
            taken[*li] = true;
        }
    }

    // Pass 3: no ComicVine id on one side; title + number + dates + a guard.
    for (li, line) in pull.iter().enumerate() {
        if taken[li] {
            continue;
        }
        let best = calendar
            .iter()
            .enumerate()
            .filter(|(ci, row)| {
                partner[*ci].is_none()
                    && (line.cv_series_id.is_none() || row.cv_series_id.is_none())
                    && numbers[*ci] == line_numbers[li]
                    && close(row.store_date, line.ship_date)
                    && same_text(Some(&row.series_name), Some(&line.series_name))
                    && ((row.year_began.is_some() && row.year_began == line.series_year)
                        || same_text(row.publisher.as_deref(), line.publisher.as_deref()))
            })
            .min_by_key(|(_, row)| {
                (
                    (row.store_date - line.ship_date).num_days().abs(),
                    row.metron_issue_id,
                )
            })
            .map(|(ci, _)| ci);
        if let Some(ci) = best {
            partner[ci] = Some(li);
            taken[li] = true;
        }
    }

    let mut out: Vec<Paired> = calendar
        .iter()
        .enumerate()
        .map(|(ci, row)| match partner[ci] {
            Some(li) => merge(row, &pull[li]),
            None => from_calendar(row),
        })
        .chain(
            pull.iter()
                .enumerate()
                .filter(|(li, _)| !taken[*li])
                .map(|(_, line)| from_pull(line)),
        )
        .collect();
    out.sort_by(|a, b| {
        (a.date, a.series_name.to_lowercase(), &a.number, a.key()).cmp(&(
            b.date,
            b.series_name.to_lowercase(),
            &b.number,
            b.key(),
        ))
    });
    out
}

/// A Folio series a release belongs to, already filtered to what the
/// viewer may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesLink {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// A local issue's number and cover month, for the owned-by-number check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIssue {
    pub number: String,
    pub year: Option<i32>,
    pub month: Option<i32>,
}

/// Everything known about the viewer's library and Mylar that bears on
/// the releases being decorated.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Visible local series by ComicVine volume id / Metron series id.
    pub by_cv_series: HashMap<i64, SeriesLink>,
    pub by_metron_series: HashMap<i64, SeriesLink>,
    /// Provider issue ids of local issues in those series.
    pub owned_metron_issues: HashSet<i64>,
    pub owned_cv_issues: HashSet<i64>,
    /// Local issues of those series, by series id.
    pub local_issues: HashMap<Uuid, Vec<LocalIssue>>,
    /// Whether the viewer may see anything from Mylar. When false every
    /// field below is ignored, whatever it holds.
    pub mylar_visible: bool,
    /// ComicVine volume ids on Mylar's watchlist that are not paused.
    pub tracked: HashSet<i64>,
    /// Mylar's wanted table by ComicVine issue id, and by
    /// `(ComicVine volume id, canonical number)`.
    pub wanted_by_issue: HashMap<i64, MylarStatus>,
    pub wanted_by_number: HashMap<(i64, String), MylarStatus>,
}

/// A release with everything a card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub paired: Paired,
    pub library_series: Option<SeriesLink>,
    pub owned: bool,
    pub tracked: bool,
    pub mylar_status: Option<MylarStatus>,
    pub stage: Stage,
    pub first_issue: bool,
}

fn months(year: i32, month: i32) -> i32 {
    year * 12 + month
}

fn owned(p: &Paired, link: &SeriesLink, facts: &Facts) -> bool {
    if p.metron_issue_id
        .is_some_and(|id| facts.owned_metron_issues.contains(&id))
        || p.cv_issue_id
            .is_some_and(|id| facts.owned_cv_issues.contains(&id))
    {
        return true;
    }
    // Number plus cover month. The calendar's cover date when it has one,
    // else the store date (a cover date runs about two months ahead).
    let reference = p.cover_date.unwrap_or(p.date);
    let want = months(reference.year(), reference.month() as i32);
    let slack = if p.cover_date.is_some() {
        OWNED_COVER_MONTHS
    } else {
        OWNED_COVER_MONTHS + 2
    };
    let number = canonical_issue_number(&p.number);
    facts.local_issues.get(&link.id).is_some_and(|issues| {
        issues.iter().any(|i| {
            canonical_issue_number(&i.number) == number
                && matches!((i.year, i.month), (Some(y), Some(m)) if (months(y, m) - want).abs() <= slack)
        })
    })
}

/// Add the library and Mylar facts to one paired release.
pub fn decorate(p: Paired, facts: &Facts) -> Entry {
    let library_series = p
        .cv_series_id
        .and_then(|id| facts.by_cv_series.get(&id))
        .or_else(|| {
            p.metron_series_id
                .and_then(|id| facts.by_metron_series.get(&id))
        })
        .cloned();
    let is_owned = library_series
        .as_ref()
        .is_some_and(|link| owned(&p, link, facts));

    let (tracked, mylar_status) = if facts.mylar_visible {
        let tracked = p.cv_series_id.is_some_and(|id| facts.tracked.contains(&id));
        let status = p
            .pull_status
            .as_deref()
            .and_then(MylarStatus::from_mylar)
            .or_else(|| {
                p.cv_issue_id
                    .and_then(|id| facts.wanted_by_issue.get(&id).copied())
            })
            .or_else(|| {
                p.cv_series_id.and_then(|cv| {
                    facts
                        .wanted_by_number
                        .get(&(cv, canonical_issue_number(&p.number)))
                        .copied()
                })
            });
        (tracked, status)
    } else {
        (false, None)
    };

    let stage = if is_owned {
        Stage::InLibrary
    } else {
        match mylar_status {
            Some(MylarStatus::Downloaded) => Stage::Downloaded,
            Some(MylarStatus::Snatched) => Stage::Snatched,
            Some(MylarStatus::Wanted | MylarStatus::Failed) => Stage::Wanted,
            None if tracked => Stage::Tracked,
            None => Stage::Announced,
        }
    };
    let first_issue = canonical_issue_number(&p.number) == "1";
    Entry {
        paired: p,
        library_series,
        owned: is_owned,
        tracked,
        mylar_status,
        stage,
        first_issue,
    }
}

impl Entry {
    /// "My series": in a library the viewer can see, or on Mylar's
    /// watchlist (which only a viewer allowed to see Mylar data has).
    pub fn is_mine(&self) -> bool {
        self.library_series.is_some() || self.tracked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn row(id: i64, series: (i64, &str), cv: Option<i64>, number: &str, date: &str) -> CalendarRow {
        CalendarRow {
            metron_issue_id: id,
            metron_series_id: series.0,
            series_name: series.1.to_owned(),
            volume: Some(1),
            year_began: Some(2024),
            publisher: Some("DC Comics".to_owned()),
            cv_series_id: cv,
            number: number.to_owned(),
            store_date: d(date),
            cover_date: None,
            image_url: None,
        }
    }

    fn line(
        id: i64,
        name: &str,
        cv: Option<i64>,
        number: &str,
        date: &str,
        status: &str,
    ) -> PullLine {
        PullLine {
            id,
            ship_date: d(date),
            publisher: Some("DC Comics".to_owned()),
            series_name: name.to_owned(),
            issue_number: number.to_owned(),
            status: status.to_owned(),
            cv_series_id: cv,
            cv_issue_id: cv.map(|c| c * 10 + id),
            series_year: Some(2024),
        }
    }

    #[test]
    fn pairs_on_comicvine_id_number_and_a_close_date() {
        let cal = [row(
            1,
            (50, "Action Comics"),
            Some(91078),
            "1104",
            "2026-11-11",
        )];
        // Mylar says Tuesday, zero-pads the number.
        let pull = [line(
            7,
            "Action Comics",
            Some(91078),
            "01104",
            "2026-11-10",
            "Downloaded",
        )];
        let out = pair(&cal, &pull);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, EntrySource::Both);
        assert_eq!(out[0].date, d("2026-11-11"), "Metron's date wins");
        assert_eq!(out[0].pull_status.as_deref(), Some("Downloaded"));
        assert_eq!(out[0].cv_issue_id, Some(910_787));
        assert_eq!(out[0].key(), "m1");
    }

    #[test]
    fn a_far_date_or_another_number_does_not_pair() {
        let cal = [row(
            1,
            (50, "Action Comics"),
            Some(91078),
            "1104",
            "2026-11-11",
        )];
        let far = [line(
            7,
            "Action Comics",
            Some(91078),
            "1104",
            "2026-12-09",
            "Wanted",
        )];
        assert_eq!(pair(&cal, &far).len(), 2);
        // Another number in another week: nothing ties them.
        let other = [line(
            7,
            "Action Comics",
            Some(91078),
            "1105",
            "2026-11-18",
            "Wanted",
        )];
        let out = pair(&cal, &other);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|p| p.source != EntrySource::Both));
    }

    #[test]
    fn pairing_is_one_to_one_and_prefers_the_closest_date() {
        // A second printing: the same series and number twice on the list.
        let cal = [
            row(1, (50, "Absolute Batman"), Some(7), "1", "2026-11-11"),
            row(2, (50, "Absolute Batman"), Some(7), "1", "2026-11-18"),
        ];
        let pull = [
            line(8, "Absolute Batman", Some(7), "1", "2026-11-18", "Wanted"),
            line(
                9,
                "Absolute Batman",
                Some(7),
                "1",
                "2026-11-11",
                "Downloaded",
            ),
        ];
        let out = pair(&cal, &pull);
        assert_eq!(out.len(), 2);
        let by_metron: HashMap<i64, &Paired> = out
            .iter()
            .map(|p| (p.metron_issue_id.unwrap(), p))
            .collect();
        assert_eq!(by_metron[&1].pull_status.as_deref(), Some("Downloaded"));
        assert_eq!(by_metron[&2].pull_status.as_deref(), Some("Wanted"));
    }

    #[test]
    fn a_lumped_and_a_split_number_pair_when_they_are_the_only_two_that_week() {
        // Metron splits the relaunch (#1); ComicVine lumps it (#601).
        let cal = [row(
            1,
            (60, "Fantastic Four"),
            Some(2045),
            "1",
            "2026-11-11",
        )];
        let pull = [line(
            7,
            "Fantastic Four",
            Some(2045),
            "601",
            "2026-11-11",
            "Wanted",
        )];
        let out = pair(&cal, &pull);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, EntrySource::Both);
        assert_eq!(out[0].number, "1", "the calendar's number is shown");

        // Two candidates on one side: too ambiguous to guess.
        let cal2 = [
            row(1, (60, "Fantastic Four"), Some(2045), "1", "2026-11-11"),
            row(2, (60, "Fantastic Four"), Some(2045), "2", "2026-11-12"),
        ];
        assert_eq!(pair(&cal2, &pull).len(), 3);
    }

    #[test]
    fn without_a_comicvine_id_the_title_needs_a_year_or_publisher_guard() {
        let mut cal = [row(
            1,
            (70, "The Unmatched Indie!"),
            None,
            "3",
            "2026-11-11",
        )];
        let mut pull = [line(
            7,
            "Unmatched Indie",
            None,
            "3",
            "2026-11-11",
            "Skipped",
        )];
        assert_eq!(pair(&cal, &pull).len(), 1, "same year and publisher");

        // Another year AND another publisher: a different book of that name.
        pull[0].series_year = Some(1994);
        pull[0].publisher = Some("Somebody Else".to_owned());
        assert_eq!(pair(&cal, &pull).len(), 2);

        // Publisher alone is enough.
        pull[0].publisher = Some("dc comics".to_owned());
        assert_eq!(pair(&cal, &pull).len(), 1);

        // Both sides have ids and they differ: the title is not consulted.
        cal[0].cv_series_id = Some(1);
        pull[0].cv_series_id = Some(2);
        assert_eq!(pair(&cal, &pull).len(), 2);
    }

    #[test]
    fn an_unpaired_pull_line_is_kept_as_its_own_release() {
        let cal = [row(
            1,
            (50, "Action Comics"),
            Some(91078),
            "1104",
            "2026-11-11",
        )];
        let pull = [line(
            7,
            "Small Press Thing",
            None,
            "2",
            "2026-11-12",
            "Skipped",
        )];
        let out = pair(&cal, &pull);
        assert_eq!(out.len(), 2);
        let only = out
            .iter()
            .find(|p| p.source == EntrySource::PullList)
            .unwrap();
        assert_eq!(only.series_name, "Small Press Thing");
        assert_eq!(only.key(), "p7");
        assert_eq!(only.metron_issue_id, None);
    }

    #[test]
    fn output_is_ordered_by_date_then_name() {
        let cal = [
            row(1, (50, "Zatanna"), Some(1), "1", "2026-11-11"),
            row(2, (51, "Action Comics"), Some(2), "9", "2026-11-18"),
            row(3, (52, "batman"), Some(3), "4", "2026-11-11"),
        ];
        let names: Vec<String> = pair(&cal, &[]).into_iter().map(|p| p.series_name).collect();
        assert_eq!(names, ["batman", "Zatanna", "Action Comics"]);
    }

    fn link() -> SeriesLink {
        SeriesLink {
            id: Uuid::from_u128(1),
            slug: "action-comics-2016".to_owned(),
            name: "Action Comics".to_owned(),
        }
    }

    fn paired() -> Paired {
        let cal = [row(
            1,
            (50, "Action Comics"),
            Some(91078),
            "1104",
            "2026-11-11",
        )];
        pair(&cal, &[]).remove(0)
    }

    #[test]
    fn stage_follows_the_precedence() {
        let mut facts = Facts {
            mylar_visible: true,
            ..Default::default()
        };
        assert_eq!(decorate(paired(), &facts).stage, Stage::Announced);

        facts.tracked.insert(91078);
        let e = decorate(paired(), &facts);
        assert_eq!(e.stage, Stage::Tracked);
        assert!(e.tracked && e.is_mine());

        for (word, stage) in [
            ("Wanted", Stage::Wanted),
            ("Failed", Stage::Wanted),
            ("Snatched", Stage::Snatched),
            ("Downloaded", Stage::Downloaded),
            ("Archived", Stage::Downloaded),
            ("Skipped", Stage::Tracked),
        ] {
            let mut p = paired();
            p.pull_status = Some(word.to_owned());
            assert_eq!(decorate(p, &facts).stage, stage, "{word}");
        }

        // Owned beats whatever Mylar says.
        facts.by_cv_series.insert(91078, link());
        facts.owned_metron_issues.insert(1);
        let mut p = paired();
        p.pull_status = Some("Wanted".to_owned());
        let e = decorate(p, &facts);
        assert_eq!(e.stage, Stage::InLibrary);
        assert!(e.owned);
        assert_eq!(e.library_series, Some(link()));
    }

    #[test]
    fn a_viewer_who_may_not_see_mylar_gets_none_of_it() {
        let mut facts = Facts::default();
        facts.tracked.insert(91078);
        facts
            .wanted_by_number
            .insert((91078, "1104".to_owned()), MylarStatus::Wanted);
        let mut p = paired();
        p.pull_status = Some("Downloaded".to_owned());
        let e = decorate(p, &facts);
        assert!(!e.tracked);
        assert_eq!(e.mylar_status, None);
        assert_eq!(e.stage, Stage::Announced);
        assert!(!e.is_mine());
    }

    #[test]
    fn mylars_wanted_table_fills_in_when_there_is_no_pull_line() {
        let mut facts = Facts {
            mylar_visible: true,
            ..Default::default()
        };
        facts
            .wanted_by_number
            .insert((91078, "1104".to_owned()), MylarStatus::Snatched);
        assert_eq!(
            decorate(paired(), &facts).mylar_status,
            Some(MylarStatus::Snatched)
        );
    }

    #[test]
    fn owned_by_number_needs_the_cover_month_too() {
        let mut facts = Facts::default();
        facts.by_cv_series.insert(91078, link());
        let local = |year, month| LocalIssue {
            number: "1104".to_owned(),
            year,
            month,
        };
        let mut p = paired();
        p.cover_date = Some(d("2027-01-01"));

        // Same number forty years ago is another issue.
        facts
            .local_issues
            .insert(link().id, vec![local(Some(1987), Some(1))]);
        assert!(!decorate(p.clone(), &facts).owned);
        // No cover date on the local issue: never number alone.
        facts
            .local_issues
            .insert(link().id, vec![local(None, None)]);
        assert!(!decorate(p.clone(), &facts).owned);
        // Within two months of the calendar's cover date.
        facts
            .local_issues
            .insert(link().id, vec![local(Some(2026), Some(12))]);
        let e = decorate(p, &facts);
        assert!(e.owned);
        assert_eq!(e.stage, Stage::InLibrary);
    }

    #[test]
    fn the_metron_series_id_links_when_there_is_no_comicvine_id() {
        let mut facts = Facts::default();
        facts.by_metron_series.insert(50, link());
        let mut p = paired();
        p.cv_series_id = None;
        let e = decorate(p, &facts);
        assert_eq!(e.library_series, Some(link()));
        assert!(e.is_mine());
        assert!(!e.first_issue);

        let mut first = paired();
        first.number = "001".to_owned();
        assert!(decorate(first, &facts).first_issue);
    }
}
