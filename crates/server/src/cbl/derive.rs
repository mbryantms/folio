//! Linked reading lists, PR 3b: place a master list's events and chain its
//! parts from the catalog, without anyone placing them by hand.
//!
//! Pure functions; the I/O (index, blob downloads, imports) lives in
//! `api::cbl_derive`.
//!
//! **Events (CBRO twin diff).** CBRO publishes every master part twice: a
//! plain list and a "With Events" twin with each event's issues inserted
//! where they are read. [`twin_diff`] diffs the two ([`diff_pairs`], Myers
//! O(ND) — a part has thousands of issues but only a few hundred inserts),
//! splits each inserted run by which candidate event list holds each issue
//! (the **smallest** candidate holding it wins, so a "Marvel Events"
//! roll-up never claims a run a specific event covers), and anchors each
//! event after the plain entry before its run. A run nobody covers is
//! reported, never guessed. Event issues already in the plain list are not
//! inserted, so they never move an event (House of M's two in Part #07).
//!
//! **Where the candidates come from.** [`twin_path`] maps
//! `<Pub>/Master Reading Order/CBRO/<file>` to
//! `<Pub>/Master Reading Order/CBRO - With Events/<file>`;
//! [`event_candidate_paths`] picks the era folder under
//! `<Pub>/Events/CBRO/` that matches the master (a numbered part →
//! `… - Part N`; a named era → the folder whose name it carries, e.g.
//! "Marvel NOW!- Marvel NOW! Part #1" → "2012-2016 - Marvel NOW!") plus
//! the loose event files at that root (Revolutionary War lives there).
//!
//! **Parts.** [`part_ordinal`] reads "Part #07" / "Part 07" / "Part 2.1"
//! from a file name; two lists in the same folder with the same prefix and
//! consecutive ordinals chain (`continues`).

use std::collections::HashMap;

/// The identity two lists' books are compared by: the ComicVine issue id
/// when there is one, else series + number (case-folded).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BookKey {
    Cv(i32),
    Name(String, String),
}

impl BookKey {
    pub fn new(cv: Option<i32>, series: &str, number: &str) -> Self {
        match cv {
            Some(id) => Self::Cv(id),
            None => Self::Name(
                series.trim().to_lowercase(),
                number.trim().trim_start_matches('#').to_lowercase(),
            ),
        }
    }
}

/// Matched `(a index, b index)` pairs of a longest common subsequence of
/// `a` and `b`, in order (Myers' O(ND) diff; memory O(D²)).
pub fn diff_pairs<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    if max == 0 {
        return Vec::new();
    }
    let off = max + 1;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    // trace[d] = v over k in [-d, d] as it stood before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut last_d = 0;
    'outer: for d in 0..=max {
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = |k: isize| (off + k) as usize;
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                last_d = d;
                break 'outer;
            }
            k += 2;
        }
    }
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=last_d).rev() {
        if d == 0 {
            while x > 0 && y > 0 {
                pairs.push(((x - 1) as usize, (y - 1) as usize));
                x -= 1;
                y -= 1;
            }
            break;
        }
        let snap = &trace[d as usize];
        let val = |k: isize| snap[(k + d) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && val(k - 1) < val(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = val(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            pairs.push(((x - 1) as usize, (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        x = prev_x;
        y = prev_y;
    }
    pairs.reverse();
    pairs
}

/// A candidate event list: its books' keys, in its order.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub keys: Vec<BookKey>,
}

/// An event placed in the plain list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Index into the candidates.
    pub candidate: usize,
    /// Index into the plain list of the entry the event is read after.
    pub after: usize,
    /// The candidate's issues inserted at this spot.
    pub issues: usize,
    /// Its order among events at the same spot (twin order).
    pub order: usize,
}

/// Inserted issues no candidate covers (or an event the twin puts before
/// the first plain entry), shown to the person instead of guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unplaced {
    /// Plain index the run follows; `None` = before the first entry.
    pub after: Option<usize>,
    /// Twin indices of the run's first and last issue.
    pub first: usize,
    pub last: usize,
    pub issues: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derivation {
    pub placements: Vec<Placement>,
    pub unplaced: Vec<Unplaced>,
}

/// Share of an event's own issues (those not already in the plain list)
/// the twin must insert at one spot for the event to be placed there.
pub const MIN_EVENT_COVERAGE: f64 = 0.6;
/// A stretch this short between two stretches of the same event is taken
/// as part of it (a stray tie-in inside an event's run).
const ABSORB_GAP: usize = 2;

/// An event found inline in a list: the stretch of the list that is the
/// event's issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRun {
    /// Index into the candidates.
    pub candidate: usize,
    /// First and last index of the stretch in the list (inclusive).
    pub start: usize,
    pub end: usize,
    /// Issues of the event in the stretch.
    pub issues: usize,
}

/// A stretch shorter than this is a tie-in, not an event read here.
pub const MIN_RUN_ISSUES: usize = 3;
/// Share of an event's issues one stretch must hold to count as the
/// place the event is read (a list that interleaves an event across the
/// whole part has no such place, and gets no chapter for it).
pub const MIN_RUN_COVERAGE: f64 = 0.5;

/// Share of a stretch that must be the event's own issues.
pub const MIN_RUN_DENSITY: f64 = 0.6;

/// Find the events a list reads **inline** (a with-events master, a CMRO
/// part): for each candidate event list, the one stretch of `list` that
/// holds most of its issues. For chapters, not links: nothing is moved,
/// the stretch just gets a name.
///
/// Same rules as [`twin_diff`]: an issue belongs to the **smallest**
/// candidate holding it, a stray issue or two inside a stretch doesn't
/// split it, and an event with no stretch holding enough of it is left
/// out rather than guessed. Runs come back in list order and never
/// overlap (a later, smaller claim on an already-taken stretch is
/// dropped).
pub fn detect_event_runs(list: &[BookKey], candidates: &[Candidate]) -> Vec<EventRun> {
    let mut owner: HashMap<&BookKey, usize> = HashMap::new();
    for (ci, c) in candidates.iter().enumerate() {
        for k in &c.keys {
            owner
                .entry(k)
                .and_modify(|cur| {
                    if candidates[*cur].keys.len() > c.keys.len() {
                        *cur = ci;
                    }
                })
                .or_insert(ci);
        }
    }
    // Stretches of one owner, absorbing short gaps inside it.
    struct Seg {
        owner: Option<usize>,
        start: usize,
        end: usize, // inclusive
        issues: usize,
    }
    let mut segs: Vec<Seg> = Vec::new();
    for (i, key) in list.iter().enumerate() {
        let o = owner.get(key).copied();
        match segs.last_mut() {
            Some(s) if s.owner == o => {
                s.end = i;
                s.issues += 1;
            }
            _ => segs.push(Seg {
                owner: o,
                start: i,
                end: i,
                issues: 1,
            }),
        }
    }
    let mut i = 1;
    while i + 1 < segs.len() {
        if segs[i - 1].owner.is_some()
            && segs[i - 1].owner == segs[i + 1].owner
            && segs[i].end - segs[i].start < ABSORB_GAP
        {
            segs[i - 1].end = segs[i + 1].end;
            segs[i - 1].issues += segs[i + 1].issues;
            segs.drain(i..=i + 1);
        } else {
            i += 1;
        }
    }
    // Each event's largest stretch, if it is enough of the event.
    let mut best: HashMap<usize, usize> = HashMap::new();
    for (si, s) in segs.iter().enumerate() {
        if let Some(o) = s.owner
            && best.get(&o).is_none_or(|&b| s.issues > segs[b].issues)
        {
            best.insert(o, si);
        }
    }
    let mut runs: Vec<EventRun> = best
        .into_iter()
        .filter_map(|(cand, si)| {
            let s = &segs[si];
            let size = candidates[cand].keys.len().max(1);
            let span = s.end - s.start + 1;
            (s.issues >= MIN_RUN_ISSUES
                && (s.issues as f64) / (size as f64) >= MIN_RUN_COVERAGE
                // Absorbing gaps can chain an event that only ever appears
                // an issue at a time across a whole part; a stretch must
                // be mostly the event.
                && (s.issues as f64) / (span as f64) >= MIN_RUN_DENSITY)
                .then_some(EventRun {
                    candidate: cand,
                    start: s.start,
                    end: s.end,
                    issues: s.issues,
                })
        })
        .collect();
    runs.sort_by_key(|r| (r.start, r.candidate));
    runs
}

/// The share of `plain`'s books the `twin` also holds (0.0–1.0; 1.0 for
/// an empty plain list). A real twin is the plain list plus insertions,
/// so nearly all of it. When the two barely overlap (the stored list has
/// no ComicVine ids and the twin does, or it is a different edition) a
/// diff would propose every event after the last issue and cost memory
/// quadratic in the mismatch, so callers treat it as "no usable twin".
pub fn twin_overlap(plain: &[BookKey], twin: &[BookKey]) -> f64 {
    if plain.is_empty() {
        return 1.0;
    }
    let twin_keys: std::collections::HashSet<&BookKey> = twin.iter().collect();
    let shared = plain.iter().filter(|k| twin_keys.contains(k)).count();
    shared as f64 / plain.len() as f64
}

/// Below this [`twin_overlap`] the twin is not diffed.
pub const MIN_TWIN_OVERLAP: f64 = 0.8;

/// Diff `plain` against its with-events `twin` and place `candidates`.
pub fn twin_diff(plain: &[BookKey], twin: &[BookKey], candidates: &[Candidate]) -> Derivation {
    // Which candidate owns each key: the smallest list holding it.
    let mut owner: HashMap<&BookKey, usize> = HashMap::new();
    for (ci, c) in candidates.iter().enumerate() {
        for k in &c.keys {
            owner
                .entry(k)
                .and_modify(|cur| {
                    if candidates[*cur].keys.len() > c.keys.len() {
                        *cur = ci;
                    }
                })
                .or_insert(ci);
        }
    }

    // Inserted runs: twin stretches between matched pairs.
    let pairs = diff_pairs(plain, twin);
    struct Run {
        after: Option<usize>,
        start: usize,
        end: usize, // exclusive
    }
    let mut runs = Vec::new();
    // A run before matched pair (i, j) is read after plain entry i - 1
    // (the sentinel pair closes a run at the end of the twin).
    let mut tb = 0usize;
    for &(i, j) in pairs
        .iter()
        .chain(std::iter::once(&(plain.len(), twin.len())))
    {
        if j > tb {
            runs.push(Run {
                after: i.checked_sub(1),
                start: tb,
                end: j,
            });
        }
        tb = j + 1;
    }

    // Segment each run by owner; absorb short gaps inside one owner.
    struct Seg {
        owner: Option<usize>,
        after: Option<usize>,
        start: usize,
        end: usize,
    }
    let mut segs: Vec<Seg> = Vec::new();
    for run in &runs {
        let mut local: Vec<Seg> = Vec::new();
        for (t, key) in twin.iter().enumerate().take(run.end).skip(run.start) {
            let o = owner.get(key).copied();
            match local.last_mut() {
                Some(s) if s.owner == o => s.end = t + 1,
                _ => local.push(Seg {
                    owner: o,
                    after: run.after,
                    start: t,
                    end: t + 1,
                }),
            }
        }
        let mut i = 1;
        while i + 1 < local.len() {
            if local[i - 1].owner.is_some()
                && local[i - 1].owner == local[i + 1].owner
                && local[i].end - local[i].start <= ABSORB_GAP
            {
                local[i - 1].end = local[i + 1].end;
                local.drain(i..=i + 1);
            } else {
                i += 1;
            }
        }
        segs.extend(local);
    }

    // Each event goes where most of its issues are inserted, if that is
    // enough of it.
    let plain_keys: std::collections::HashSet<&BookKey> = plain.iter().collect();
    let mut best: HashMap<usize, usize> = HashMap::new(); // candidate → seg idx
    for (si, s) in segs.iter().enumerate() {
        if let Some(o) = s.owner {
            let len = s.end - s.start;
            let better = best
                .get(&o)
                .is_none_or(|&b| len > segs[b].end - segs[b].start);
            if better {
                best.insert(o, si);
            }
        }
    }
    let mut out = Derivation::default();
    let mut placed_segs = std::collections::HashSet::new();
    for (&cand, &si) in &best {
        let s = &segs[si];
        let own = candidates[cand]
            .keys
            .iter()
            .filter(|k| !plain_keys.contains(k))
            .count()
            .max(1);
        let issues = s.end - s.start;
        if (issues as f64) / (own as f64) < MIN_EVENT_COVERAGE {
            continue;
        }
        let Some(after) = s.after else {
            continue;
        };
        placed_segs.insert(si);
        out.placements.push(Placement {
            candidate: cand,
            after,
            issues,
            order: s.start,
        });
    }
    for (si, s) in segs.iter().enumerate() {
        // A placed event's other, smaller stretches are part of it.
        if placed_segs.contains(&si)
            || s.owner
                .is_some_and(|o| best.get(&o).is_some_and(|b| placed_segs.contains(b)))
        {
            continue;
        }
        match out.unplaced.last_mut() {
            Some(u) if u.after == s.after && u.last + 1 == s.start => {
                u.last = s.end - 1;
                u.issues += s.end - s.start;
            }
            _ => out.unplaced.push(Unplaced {
                after: s.after,
                first: s.start,
                last: s.end - 1,
                issues: s.end - s.start,
            }),
        }
    }
    // Reading order; `order` becomes the rank among events at one spot.
    out.placements.sort_by_key(|p| (p.after, p.order));
    let mut rank = 0;
    for i in 0..out.placements.len() {
        if i > 0 && out.placements[i - 1].after == out.placements[i].after {
            rank += 1;
        } else {
            rank = 0;
        }
        out.placements[i].order = rank;
    }
    out
}

// ───── catalog paths ─────

const MASTER_SEGMENT: &str = "/Master Reading Order/CBRO/";
const TWIN_SEGMENT: &str = "/Master Reading Order/CBRO - With Events/";

/// The with-events twin of a CBRO master path, if it is one.
pub fn twin_path(master: &str) -> Option<String> {
    master
        .contains(MASTER_SEGMENT)
        .then(|| master.replacen(MASTER_SEGMENT, TWIN_SEGMENT, 1))
}

/// File name without folders or the `.cbl` extension.
pub fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".cbl").unwrap_or(name)
}

/// "… Part #07 (WEB-CBRO)" → ("…", 7.0); "Part 2.1" → 2.1. One
/// implementation for every catalog source: [`super::sources::part_of`].
pub fn part_ordinal(stem: &str) -> Option<(String, f64)> {
    super::sources::part_of(stem)
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Words that say nothing about which era a list is.
const NOISE: &[&str] = &[
    "master", "reading", "order", "web", "cbro", "part", "comics",
];

/// The candidate event paths for `master` from the catalog's `paths`: the
/// era folder that matches it plus the loose files at the events root
/// (minus the "… Events- …" roll-ups of every event). Empty when the
/// master isn't a CBRO master or no folder matches.
pub fn event_candidate_paths<'a>(master: &str, paths: &[&'a str]) -> Vec<&'a str> {
    let Some(at) = master.find(MASTER_SEGMENT) else {
        return Vec::new();
    };
    let publisher = &master[..at];
    let root = format!("{publisher}/Events/CBRO/");
    let stem = file_stem(master);
    let pub_tokens: Vec<String> = tokens(publisher)
        .into_iter()
        .chain(
            stem.strip_prefix('[')
                .and_then(|s| s.split(']').next())
                .map(tokens)
                .unwrap_or_default(),
        )
        .collect();
    let meaningful = |s: &str| -> Vec<String> {
        tokens(s)
            .into_iter()
            .filter(|t| !NOISE.contains(&t.as_str()) && !pub_tokens.contains(t))
            .filter(|t| !t.chars().all(|c| c.is_ascii_digit()) || t.len() < 4)
            .collect()
    };

    let mut folders: Vec<&str> = Vec::new();
    let mut loose: Vec<&'a str> = Vec::new();
    for p in paths {
        let Some(rest) = p.strip_prefix(root.as_str()) else {
            continue;
        };
        match rest.split_once('/') {
            Some((folder, _)) => {
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
            }
            None if !rest.contains("Events-") => loose.push(p),
            None => {}
        }
    }

    // The era part of a folder: "2012-2016 - Marvel NOW!" → "Marvel NOW!".
    let era = |f: &'a str| f.split_once(" - ").map_or(f, |(_, e)| e);
    let numbered_master = stem.to_lowercase().contains("master reading order");
    let ordinal = part_ordinal(stem).map(|(_, n)| n);
    let chosen = if numbered_master && let Some(n) = ordinal {
        folders
            .iter()
            .find(|f| part_ordinal(era(f)).is_some_and(|(p, m)| p.is_empty() && m == n))
            .copied()
    } else {
        // Strip "Part #N" before matching a named era.
        let base = match stem.to_lowercase().rfind("part") {
            Some(i) => &stem[..i],
            None => stem,
        };
        let want = meaningful(base);
        folders
            .iter()
            .filter_map(|f| {
                let have = meaningful(era(f));
                if have.is_empty() || part_ordinal(era(f)).is_some_and(|(p, _)| p.is_empty()) {
                    return None;
                }
                let hit = have.iter().filter(|t| want.contains(t)).count();
                (hit == have.len()).then_some((hit, *f))
            })
            .max_by_key(|(hit, _)| *hit)
            .map(|(_, f)| f)
    };
    let Some(folder) = chosen else {
        return Vec::new();
    };
    let prefix = format!("{root}{folder}/");
    paths
        .iter()
        .copied()
        .filter(|p| p.starts_with(&prefix))
        .chain(loose)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: i32) -> BookKey {
        BookKey::Cv(n)
    }
    fn keys(ns: &[i32]) -> Vec<BookKey> {
        ns.iter().copied().map(k).collect()
    }

    #[test]
    fn diff_pairs_finds_a_longest_common_subsequence() {
        let a = keys(&[1, 2, 3, 4, 5]);
        let b = keys(&[9, 1, 2, 7, 3, 5, 8]);
        assert_eq!(diff_pairs(&a, &b), vec![(0, 1), (1, 2), (2, 4), (4, 5)]);
        assert_eq!(diff_pairs::<BookKey>(&[], &keys(&[1])), vec![]);
        assert_eq!(diff_pairs(&a, &a).len(), 5);
    }

    #[test]
    fn places_events_back_to_back_and_reports_strays() {
        // plain 1..=6; twin inserts A(100..103) after 2, B(200,201) right
        // after A, a stray 999 after 4, and C(300..302) after 6.
        let plain = keys(&[1, 2, 3, 4, 5, 6]);
        let twin = keys(&[
            1, 2, 100, 101, 102, 103, 200, 201, 3, 4, 999, 5, 6, 300, 301, 302,
        ]);
        let cands = vec![
            Candidate {
                keys: keys(&[100, 101, 102, 103]),
            },
            Candidate {
                keys: keys(&[200, 201]),
            },
            Candidate {
                keys: keys(&[300, 301, 302]),
            },
            // A roll-up holding everything never wins over the specific lists.
            Candidate {
                keys: keys(&[100, 101, 102, 103, 200, 201, 300, 301, 302]),
            },
        ];
        let d = twin_diff(&plain, &twin, &cands);
        let got: Vec<_> = d
            .placements
            .iter()
            .map(|p| (p.candidate, p.after, p.issues, p.order))
            .collect();
        assert_eq!(got, vec![(0, 1, 4, 0), (1, 1, 2, 1), (2, 5, 3, 0)]);
        assert_eq!(
            d.unplaced,
            vec![Unplaced {
                after: Some(3),
                first: 10,
                last: 10,
                issues: 1
            }]
        );
    }

    #[test]
    fn event_issues_already_in_the_plain_list_do_not_move_it() {
        // House of M: issues 51 and 52 are in the plain list too.
        let plain = keys(&[1, 51, 2, 52, 3]);
        let twin = keys(&[1, 51, 2, 52, 3, 50, 53, 54]);
        let cands = vec![Candidate {
            keys: keys(&[50, 51, 52, 53, 54]),
        }];
        let d = twin_diff(&plain, &twin, &cands);
        assert_eq!(d.placements.len(), 1);
        assert_eq!((d.placements[0].after, d.placements[0].issues), (4, 3));
    }

    #[test]
    fn a_sliver_of_an_event_is_not_enough_to_place_it() {
        let plain = keys(&[1, 2]);
        let twin = keys(&[1, 10, 2]);
        let cands = vec![Candidate {
            keys: keys(&[10, 11, 12, 13]),
        }];
        let d = twin_diff(&plain, &twin, &cands);
        assert!(d.placements.is_empty());
        assert_eq!(d.unplaced.len(), 1);
    }

    #[test]
    fn a_stray_inside_an_event_is_absorbed_and_before_start_is_unplaced() {
        let plain = keys(&[1, 2]);
        let twin = keys(&[7, 1, 10, 11, 999, 12, 2]);
        let cands = vec![
            Candidate {
                keys: keys(&[10, 11, 12]),
            },
            Candidate { keys: keys(&[7]) },
        ];
        let d = twin_diff(&plain, &twin, &cands);
        assert_eq!(d.placements.len(), 1);
        assert_eq!((d.placements[0].candidate, d.placements[0].issues), (0, 4));
        assert_eq!(d.unplaced[0].after, None, "before the first entry");
    }

    #[test]
    fn part_ordinals_and_twins() {
        assert_eq!(
            part_ordinal("[Marvel] Marvel Master Reading Order Part #07 (WEB-CBRO)"),
            Some(("[Marvel] Marvel Master Reading Order".into(), 7.0))
        );
        assert_eq!(
            part_ordinal("Something Part 2.1"),
            Some(("Something".into(), 2.1))
        );
        assert_eq!(part_ordinal("No parts here"), None);
        assert_eq!(
            twin_path("Marvel/Master Reading Order/CBRO/x.cbl").as_deref(),
            Some("Marvel/Master Reading Order/CBRO - With Events/x.cbl")
        );
        assert_eq!(twin_path("Other/Star Wars/CBRO/x.cbl"), None);
    }

    #[test]
    fn picks_the_matching_event_folder() {
        let paths = [
            "Marvel/Events/CBRO/2002-2007 - Part 7/[Marvel] House of M (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2005-2009 - Part 8/[Marvel] Civil War (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2012-2016 - Marvel NOW!/[Marvel] Infinity (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2015-2018 - All-New All Different Marvel/[Marvel] Civil War II (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2018-2019 - A Fresh Start/[Marvel] War of the Realms (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/[Marvel] Revolutionary War (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/[Marvel] Marvel Events- Marvel (WEB-CBRO).cbl",
            "DC/Events/CBRO/2011-2015 - New 52/[DC Comics] Night of the Owls (WEB-CBRO).cbl",
            "DC/Events/CBRO/2015-2016 - DC You/[DC Comics] Darkseid War (WEB-CBRO).cbl",
        ];
        let pick = |m: &str| -> Vec<String> {
            event_candidate_paths(m, &paths)
                .into_iter()
                .map(|p| file_stem(p).to_owned())
                .collect()
        };
        let mro = "Marvel/Master Reading Order/CBRO/";
        assert_eq!(
            pick(&format!(
                "{mro}[Marvel] Marvel Master Reading Order Part #07 (WEB-CBRO).cbl"
            )),
            vec![
                "[Marvel] House of M (WEB-CBRO)",
                "[Marvel] Revolutionary War (WEB-CBRO)"
            ]
        );
        assert_eq!(
            pick(&format!(
                "{mro}[Marvel] Marvel NOW!- Marvel NOW! Part #1 (WEB-CBRO).cbl"
            ))[0],
            "[Marvel] Infinity (WEB-CBRO)"
        );
        assert_eq!(
            pick(&format!(
                "{mro}[Marvel] All-New, All Different Marvel- All-New, All-Different Marvel Part #2 (WEB-CBRO).cbl"
            ))[0],
            "[Marvel] Civil War II (WEB-CBRO)"
        );
        assert_eq!(
            pick(&format!(
                "{mro}[Marvel] Marvel Master Reading Order- A Fresh Start (WEB-CBRO).cbl"
            ))[0],
            "[Marvel] War of the Realms (WEB-CBRO)"
        );
        let dc = "DC/Master Reading Order/CBRO/";
        assert_eq!(
            pick(&format!(
                "{dc}[DC Comics] New 52 Reading Order- New Part #1 (WEB-CBRO).cbl"
            )),
            vec!["[DC Comics] Night of the Owls (WEB-CBRO)"]
        );
        assert_eq!(
            pick(&format!("{dc}[DC Comics] DC You (WEB-CBRO).cbl")),
            vec!["[DC Comics] Darkseid War (WEB-CBRO)"]
        );
        assert!(pick("Other/Star Wars/CBRO/x.cbl").is_empty());
    }

    #[test]
    fn twin_overlap_tells_a_real_twin_from_a_mismatch() {
        let k = |n: i32| BookKey::Cv(n);
        let plain: Vec<BookKey> = (0..10).map(k).collect();
        // The plain list plus insertions: everything overlaps.
        let twin: Vec<BookKey> = (0..10).flat_map(|n| [k(n), k(100 + n)]).collect();
        assert!(twin_overlap(&plain, &twin) >= MIN_TWIN_OVERLAP);
        // Same books keyed by name on one side, by id on the other.
        let named: Vec<BookKey> = (0..10)
            .map(|n| BookKey::new(None, "Series", &n.to_string()))
            .collect();
        assert!(twin_overlap(&named, &twin) < MIN_TWIN_OVERLAP);
        assert_eq!(twin_overlap(&[], &twin), 1.0);
    }

    #[test]
    fn detects_events_read_inline() {
        let k = |n: i32| BookKey::Cv(n);
        // 0..5 main, 100..106 event A (6 issues) with one stray main issue
        // inside, 6..8 main, 200..203 event B, then a lone A tie-in.
        let list: Vec<BookKey> = [0, 1, 2, 3, 4, 100, 101, 102, 5, 103, 104, 105]
            .into_iter()
            .chain([6, 7, 200, 201, 202, 8, 106])
            .map(k)
            .collect();
        let a = Candidate {
            keys: (100..107).map(k).collect(),
        };
        let b = Candidate {
            keys: (200..203).map(k).collect(),
        };
        // A roll-up holding both events never claims a specific event's run.
        let rollup = Candidate {
            keys: (100..107).chain(200..203).map(k).collect(),
        };
        let runs = detect_event_runs(&list, &[a, b, rollup]);
        assert_eq!(
            runs,
            vec![
                EventRun {
                    candidate: 0,
                    start: 5,
                    end: 11,
                    issues: 6
                },
                EventRun {
                    candidate: 1,
                    start: 14,
                    end: 16,
                    issues: 3
                },
            ]
        );
    }

    #[test]
    fn an_event_spread_thin_gets_no_chapter() {
        let k = |n: i32| BookKey::Cv(n);
        // Ten event issues, never more than two together.
        let list: Vec<BookKey> = (0..10)
            .flat_map(|i| [k(i), k(i + 50), k(100 + i)])
            .collect();
        let ev = Candidate {
            keys: (100..110).map(k).collect(),
        };
        assert!(detect_event_runs(&list, &[ev]).is_empty());
        // Too few issues to be an event read here.
        let tiny = Candidate {
            keys: vec![k(100), k(101)],
        };
        let list: Vec<BookKey> = [0, 100, 101, 1].into_iter().map(k).collect();
        assert!(detect_event_runs(&list, &[tiny]).is_empty());
    }
}
