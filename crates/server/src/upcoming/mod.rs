//! Upcoming releases (upcoming-releases 1.0): what is coming out, and
//! what an operator's Mylar is tracking, shown read-only.
//!
//! Two sources feed it and neither is ever written back to:
//!
//! - [`calendar`] — the release calendar. A scheduled job lists Metron's
//!   issues by store-date window into the `release_calendar_*` tables.
//! - [`mylar`] — a snapshot pushed *to* Folio by a script on the Mylar
//!   host. Folio never calls Mylar: there is no Mylar address or key
//!   setting, by design.
//!
//! Architecture, the view-only invariant and the operator steps:
//! `docs/dev/upcoming-releases.md`.

pub mod calendar;
pub mod mylar;
pub mod reconcile;
pub mod view;

use chrono::{Datelike, Duration, NaiveDate};

/// The only host a calendar cover is served from (and the CSP allows).
/// The API sends no other cover URL, and the colour read fetches no other.
pub const COVER_HOST_PREFIX: &str = "https://static.metron.cloud/";

/// Default for `upcoming.weeks_ahead`. Metron is complete about six
/// weeks out and partial for two more (measured 2026-11-14).
pub const DEFAULT_WEEKS_AHEAD: u32 = 8;
/// Upper bound for `upcoming.weeks_ahead`.
pub const MAX_WEEKS_AHEAD: u32 = 12;
/// Default for `integrations.mylar.stale_after_minutes`: three missed
/// hourly heartbeats.
pub const DEFAULT_MYLAR_STALE_MINUTES: u32 = 180;
/// Weeks before the current one the list pass keeps listing, so a
/// release that just came out still has its row.
pub const WEEKS_BACK: u32 = 2;
/// A calendar row is deleted once its store date is this many days past.
pub const PRUNE_AFTER_DAYS: i64 = 28;

/// Monday of the week containing `day`. Weeks are Monday–Sunday
/// everywhere in this feature (the web's `lib/log/overview.ts`
/// convention); a provider's or Mylar's own week numbering is never used.
pub fn week_start(day: NaiveDate) -> NaiveDate {
    day - Duration::days(i64::from(day.weekday().num_days_from_monday()))
}

/// The inclusive span of store dates the calendar covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// A Monday.
    pub start: NaiveDate,
    /// A Sunday.
    pub end: NaiveDate,
}

impl Window {
    /// Mondays of every week in the window, in order.
    pub fn weeks(&self) -> impl Iterator<Item = NaiveDate> + use<> {
        let (start, end) = (self.start, self.end);
        (0..)
            .map(move |i| start + Duration::weeks(i))
            .take_while(move |monday| *monday <= end)
    }
}

/// [`WEEKS_BACK`] weeks before `today`'s week through `weeks_ahead` weeks
/// after it.
pub fn window(today: NaiveDate, weeks_ahead: u32) -> Window {
    let this_week = week_start(today);
    Window {
        start: this_week - Duration::weeks(i64::from(WEEKS_BACK)),
        end: this_week + Duration::weeks(i64::from(weeks_ahead)) + Duration::days(6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn week_start_is_the_monday() {
        // 2026-11-14 is a Saturday; its week began Monday the 9th.
        assert_eq!(week_start(d(2026, 11, 14)), d(2026, 11, 9));
        assert_eq!(week_start(d(2026, 11, 9)), d(2026, 11, 9));
        // A Sunday belongs to the week that began six days earlier.
        assert_eq!(week_start(d(2026, 11, 15)), d(2026, 11, 9));
        assert_eq!(week_start(d(2026, 11, 16)), d(2026, 11, 16));
    }

    #[test]
    fn window_spans_weeks_back_through_weeks_ahead() {
        let w = window(d(2026, 11, 14), 8);
        assert_eq!(w.start, d(2026, 10, 26));
        // Monday 2026-11-09 + 8 weeks = 2027-01-04, plus six days.
        assert_eq!(w.end, d(2027, 1, 10));
        let weeks: Vec<_> = w.weeks().collect();
        // Two back, the current one, eight ahead.
        assert_eq!(weeks.len(), 11);
        assert_eq!(weeks[0], d(2026, 10, 26));
        assert_eq!(weeks[2], d(2026, 11, 9));
        assert_eq!(*weeks.last().unwrap(), d(2027, 1, 4));
    }
}
