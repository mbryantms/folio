//! What a catalog file is, read from its path: which reading-order source
//! published it, whether it is one part of a numbered order, and which
//! files make up that order.
//!
//! The community catalog (`DieselTech/CBL-ReadingLists`) carries several
//! sources, each with its own layout:
//!
//! | Source | Where | Shape |
//! |---|---|---|
//! | CBRO | `<Pub>/Master Reading Order/CBRO/` | plain master parts, `… Part #07 (WEB-CBRO)` |
//! | CBRO with events | `<Pub>/Master Reading Order/CBRO - With Events/` | the same parts with events inline |
//! | CMRO | `Marvel/Master Reading Order/CMRO/<depth>/` | one order at four depths, cut into 1,000-issue files, `… Reading Order-Part 01` |
//! | Marvel Guides | `<Pub>/Master Reading Order/Marvel Guides/<era>/` | named sub-chapters, `… Part 2.1 Age of Ultron (MG)` |
//! | UXRO | `…/Ultimate X-Men Reading Order/` | era files ordered by their leading `[years]` |
//! | LoCG, Official, CBH, CBT | `<Pub>/Events/…` | standalone event lists |
//!
//! Pure functions over paths; this is the one place that parses catalog
//! paths ([`super::derive`] uses it for part numbers). `docs/dev/
//! catalog-sources.md` has the survey behind the table.

use serde::Serialize;

/// Who published a catalog file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSourceKind {
    /// Comic Book Reading Orders: a plain master part or an event list.
    Cbro,
    /// A CBRO master part with every event's issues inline.
    CbroWithEvents,
    /// The Complete Marvel Reading Order.
    Cmro,
    MarvelGuides,
    /// The Ultimate X-Men Reading Order.
    Uxro,
    /// League of Comic Geeks.
    Locg,
    /// A publisher's own checklist.
    Official,
    Other,
}

/// What [`profile`] reads from a path.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub source: CatalogSourceKind,
    /// The same for every part of one order, `None` for a standalone list.
    pub order_key: Option<String>,
    /// The part's number within its order (`7`, `2.1`, a start year for
    /// UXRO): parts sort by it.
    pub part: Option<f64>,
    /// CMRO only: `Essential`, `Core`, `Main` or `Expanded` (how much of
    /// the order the file's set covers).
    pub depth: Option<String>,
    /// For a with-events file: the plain file it is the twin of.
    pub plain_twin: Option<String>,
}

const CBRO_MASTER: &str = "/Master Reading Order/CBRO/";
const CBRO_TWIN: &str = "/Master Reading Order/CBRO - With Events/";
const CMRO: &str = "/Master Reading Order/CMRO/";
const MARVEL_GUIDES: &str = "/Marvel Guides/";
const UXRO: &str = "/Ultimate X-Men Reading Order/";

/// File name without folders or the `.cbl` extension.
pub fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".cbl").unwrap_or(name)
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(f, _)| f)
}

/// "… Part #07 (WEB-CBRO)" → ("…", 7.0); "… Part 2.1 Age of Ultron (MG)"
/// → ("…", 2.1). The prefix is everything before the word, trimmed, so
/// parts of one order compare equal.
///
/// The word must stand alone and be followed by a number: the **first**
/// such "Part" wins, so a title that happens to contain the letters
/// ("Departed", "Part of the Problem") never hides or fakes a part number.
pub fn part_of(stem: &str) -> Option<(String, f64)> {
    let lower = stem.to_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(i) = lower[from..].find("part") {
        let at = from + i;
        from = at + 4;
        let boundary = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        if !boundary || !stem.is_char_boundary(at) || !stem.is_char_boundary(at + 4) {
            continue;
        }
        let rest = stem[at + 4..]
            .trim_start()
            .trim_start_matches('#')
            .trim_start();
        // The number must follow the word directly ("Parts", "Partial"
        // leave letters here and fail to parse).
        if stem[at + 4..].starts_with(|c: char| c.is_alphanumeric()) {
            continue;
        }
        let num: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(n) = num.trim_end_matches('.').parse::<f64>() {
            return Some((stem[..at].trim().to_owned(), n));
        }
    }
    None
}

/// A leading `[1963-1974]` tag as one sortable number: the start year,
/// then the end year (`1963.1974`), so two files starting the same year
/// still order by when they end.
fn leading_years(stem: &str) -> Option<f64> {
    let inner = stem.strip_prefix('[')?.split(']').next()?;
    let (lo, hi) = year_range(inner)?;
    Some(f64::from(lo) + f64::from(hi) / 10_000.0)
}

/// The era folder of a Marvel Guides file: the path up to and including
/// the segment after `Marvel Guides/`. An era is one order; its files sit
/// in a sub-folder per part ("Part 02 Age of Ultron/… Part 2.1 …").
fn marvel_guides_era(path: &str) -> Option<&str> {
    let at = path.find(MARVEL_GUIDES)? + MARVEL_GUIDES.len();
    let era_len = path[at..].find('/')?;
    Some(&path[..at + era_len])
}

fn has_tag(stem: &str, tag: &str) -> bool {
    stem.to_ascii_lowercase()
        .contains(&format!("({})", tag.to_ascii_lowercase()))
}

/// What the file at `path` is.
pub fn profile(path: &str) -> Profile {
    let stem = file_stem(path);
    let dir = folder(path);
    let source = if path.contains(CBRO_TWIN) {
        CatalogSourceKind::CbroWithEvents
    } else if path.contains(CMRO) {
        CatalogSourceKind::Cmro
    } else if path.contains(MARVEL_GUIDES) || has_tag(stem, "MG") {
        CatalogSourceKind::MarvelGuides
    } else if path.contains(UXRO) || has_tag(stem, "UXRO") {
        CatalogSourceKind::Uxro
    } else if path.contains(CBRO_MASTER)
        || path.contains("/CBRO/")
        || has_tag(stem, "WEB-CBRO")
        || has_tag(stem, "CBRO")
    {
        CatalogSourceKind::Cbro
    } else if has_tag(stem, "LoCG") || has_tag(stem, "WEB-LOCG") || path.contains("/LoCG/") {
        CatalogSourceKind::Locg
    } else if has_tag(stem, "Official") || path.contains("/Official/") {
        CatalogSourceKind::Official
    } else {
        CatalogSourceKind::Other
    };
    let (order_key, part) = match source {
        // Ordered by era, not numbered.
        CatalogSourceKind::Uxro => match leading_years(stem) {
            Some(y) => (Some(format!("{dir}|uxro")), Some(y)),
            None => (None, None),
        },
        // The era is the order, whichever part folder a file sits in and
        // however its prefix is written ("2021-2024" beside "2021-2023").
        CatalogSourceKind::MarvelGuides => match (marvel_guides_era(path), part_of(stem)) {
            (Some(era), Some((_, n))) => (Some(format!("{era}|mg")), Some(n)),
            (None, Some((prefix, n))) => (Some(format!("{dir}|{prefix}")), Some(n)),
            _ => (None, None),
        },
        _ => match part_of(stem) {
            Some((prefix, n)) => (Some(format!("{dir}|{prefix}")), Some(n)),
            None => (None, None),
        },
    };
    let depth = (source == CatalogSourceKind::Cmro)
        .then(|| {
            let after = &path[path.find(CMRO)? + CMRO.len()..];
            let d = after.split('/').next()?;
            (!d.ends_with(".cbl") && d != "Misc").then(|| d.to_owned())
        })
        .flatten();
    let plain_twin = (source == CatalogSourceKind::CbroWithEvents)
        .then(|| path.replacen(CBRO_TWIN, CBRO_MASTER, 1));
    Profile {
        source,
        order_key,
        part,
        depth,
        plain_twin,
    }
}

/// Every file of the order `path` belongs to (itself included), as
/// `(part, path)` in reading order; empty when `path` is not part of an
/// order, or is the order's only file.
pub fn order_parts<'a>(path: &str, index_paths: &[&'a str]) -> Vec<(f64, &'a str)> {
    let Some(key) = profile(path).order_key else {
        return Vec::new();
    };
    let mut parts: Vec<(f64, &'a str)> = index_paths
        .iter()
        .filter_map(|p| {
            let pr = profile(p);
            (pr.order_key.as_deref() == Some(key.as_str()))
                .then_some(pr.part)
                .flatten()
                .map(|n| (n, *p))
        })
        .collect();
    parts.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    parts.dedup_by(|a, b| a.1 == b.1);
    if parts.len() < 2 {
        return Vec::new();
    }
    parts
}

/// The first and last year of a leading `1986-1993` (folder names under
/// `<Pub>/Events/CBRO/` start with one).
fn year_range(name: &str) -> Option<(i32, i32)> {
    let (a, rest) = name.split_once('-')?;
    let lo: i32 = a.trim().parse().ok()?;
    let hi: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let hi: i32 = hi.parse().ok()?;
    ((1900..=2200).contains(&lo) && hi >= lo).then_some((lo, hi))
}

/// The event lists worth comparing a list against when looking for the
/// events it reads inline: under `<publisher>/Events/CBRO/`, the files of
/// every era folder whose years overlap `lo..=hi`, plus the loose event
/// files at that root (minus the "… Events- …" roll-ups of every event).
/// `path` is the list's own catalog path (its first segment is the
/// publisher). Sorted, so a cap takes the same files every time.
pub fn event_files_for_years<'a>(
    path: &str,
    lo: i32,
    hi: i32,
    index_paths: &[&'a str],
) -> Vec<&'a str> {
    let Some(publisher) = path.split('/').next().filter(|p| !p.is_empty()) else {
        return Vec::new();
    };
    let root = format!("{publisher}/Events/CBRO/");
    let mut out: Vec<&'a str> = index_paths
        .iter()
        .filter_map(|p| {
            let rest = p.strip_prefix(root.as_str())?;
            match rest.split_once('/') {
                // In an era folder: by its years.
                Some((era, _)) => {
                    let (a, b) = year_range(era)?;
                    (a <= hi && b >= lo).then_some(*p)
                }
                // Loose at the root: an event of no one era.
                None => (!rest.contains(" Events- ")).then_some(*p),
            }
        })
        .collect();
    out.sort_unstable();
    out
}

/// A short name for an order, for a group of its lists: the shared prefix
/// of its file names, without a trailing separator.
pub fn order_name(path: &str) -> Option<String> {
    let pr = profile(path);
    pr.order_key.as_ref()?;
    let stem = file_stem(path);
    let name = match pr.source {
        CatalogSourceKind::Uxro => folder(path).rsplit('/').next().unwrap_or(stem).to_owned(),
        CatalogSourceKind::MarvelGuides => match marvel_guides_era(path) {
            Some(era) => format!("Marvel Guides {}", era.rsplit('/').next().unwrap_or(era)),
            None => part_of(stem)?.0,
        },
        _ => part_of(stem)?.0,
    };
    let name = name
        .trim()
        .trim_end_matches(['-', '–', ':'])
        .trim()
        .to_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMRO_CORE_1: &str =
        "Marvel/Master Reading Order/CMRO/Core/[Marvel] CMRO Core Reading Order-Part 01.cbl";
    const CMRO_CORE_2: &str =
        "Marvel/Master Reading Order/CMRO/Core/[Marvel] CMRO Core Reading Order-Part 02.cbl";
    const CMRO_MAIN_1: &str =
        "Marvel/Master Reading Order/CMRO/Main/[Marvel] CMRO Main Reading Order-Part 01.cbl";
    const CMRO_MISC: &str =
        "Marvel/Master Reading Order/CMRO/Misc/[Marvel] CMRO Ultimate Reading Order.cbl";
    const CBRO_7: &str = "Marvel/Master Reading Order/CBRO/[Marvel] Marvel Master Reading Order Part #07 (WEB-CBRO).cbl";
    const CBRO_8: &str = "Marvel/Master Reading Order/CBRO/[Marvel] Marvel Master Reading Order Part #08 (WEB-CBRO).cbl";
    const CBRO_NOW_1: &str =
        "Marvel/Master Reading Order/CBRO/[Marvel] Marvel NOW!- Marvel NOW! Part #1 (WEB-CBRO).cbl";
    const TWIN_7: &str = "Marvel/Master Reading Order/CBRO - With Events/[Marvel] Marvel Master Reading Order Part #07 (WEB-CBRO).cbl";
    const MG_1_4: &str = "Marvel/Master Reading Order/Marvel Guides/2012-2015 Marvel NOW!/Part 01 Now or Never/[Marvel] 2012-2015 Part 1.4 Superior Power (MG).cbl";
    const MG_2_1: &str = "Marvel/Master Reading Order/Marvel Guides/2012-2015 Marvel NOW!/Part 02 Age of Ultron/[Marvel] 2012-2015 Part 2.1 Age of Ultron (MG).cbl";
    const MG_3_2: &str = "Marvel/Master Reading Order/Marvel Guides/2012-2015 Marvel NOW!/Part 03 Battle of the Atom/[Marvel] 2012-2014 Part 3.2 Aftermath of the Departed (MG).cbl";
    const UXRO_1: &str = "Marvel/Teams/X-Men/Ultimate X-Men Reading Order/[1963-1974] X-Men Era One The Silver Age (UXRO).cbl";
    const UXRO_2: &str = "Marvel/Teams/X-Men/Ultimate X-Men Reading Order/[1974-1983] X-Men Era Two The Uncanny X-Men (UXRO).cbl";
    const EVENT: &str =
        "Marvel/Events/CBRO/2012-2016 - Marvel NOW!/[Marvel] Age of Ultron (WEB-CBRO).cbl";
    const LOCG: &str = "Marvel/Events/LoCG/Civil War (LoCG).cbl";

    #[test]
    fn part_numbers_need_the_word_and_a_number() {
        assert_eq!(
            part_of("[Marvel] Marvel Master Reading Order Part #07 (WEB-CBRO)"),
            Some(("[Marvel] Marvel Master Reading Order".into(), 7.0))
        );
        assert_eq!(
            part_of("[Marvel] CMRO Core Reading Order-Part 01"),
            Some(("[Marvel] CMRO Core Reading Order-".into(), 1.0))
        );
        assert_eq!(
            part_of("Something Part 2.1"),
            Some(("Something".into(), 2.1))
        );
        // A later word containing the letters doesn't hide the number...
        assert_eq!(
            part_of("[Marvel] 2012-2015 Part 3.2 Aftermath of the Departed (MG)"),
            Some(("[Marvel] 2012-2015".into(), 3.2))
        );
        // ...and an earlier one doesn't fake it.
        assert_eq!(
            part_of("Departures Part 4"),
            Some(("Departures".into(), 4.0))
        );
        assert_eq!(part_of("Part of the Problem"), None);
        assert_eq!(part_of("Partial Eclipse 2"), None);
        assert_eq!(part_of("No parts here"), None);
    }

    #[test]
    fn profiles_name_the_source_and_the_order() {
        let c = profile(CMRO_CORE_1);
        assert_eq!(c.source, CatalogSourceKind::Cmro);
        assert_eq!(c.depth.as_deref(), Some("Core"));
        assert_eq!(c.part, Some(1.0));
        // Two depths are two orders; two parts of one depth are one.
        assert_eq!(c.order_key, profile(CMRO_CORE_2).order_key);
        assert_ne!(c.order_key, profile(CMRO_MAIN_1).order_key);
        // A side order isn't a depth and has no parts.
        let misc = profile(CMRO_MISC);
        assert_eq!(misc.source, CatalogSourceKind::Cmro);
        assert_eq!((misc.depth, misc.order_key), (None, None));

        assert_eq!(profile(CBRO_7).source, CatalogSourceKind::Cbro);
        assert_eq!(profile(CBRO_7).order_key, profile(CBRO_8).order_key);
        assert_ne!(profile(CBRO_7).order_key, profile(CBRO_NOW_1).order_key);
        let twin = profile(TWIN_7);
        assert_eq!(twin.source, CatalogSourceKind::CbroWithEvents);
        assert_eq!(twin.plain_twin.as_deref(), Some(CBRO_7));

        let mg = profile(MG_2_1);
        assert_eq!(mg.source, CatalogSourceKind::MarvelGuides);
        assert_eq!(mg.part, Some(2.1));
        assert_eq!(mg.order_key, profile(MG_3_2).order_key);

        assert_eq!(profile(UXRO_1).source, CatalogSourceKind::Uxro);
        assert_eq!(profile(UXRO_1).part, Some(1963.1974));
        assert!(profile(UXRO_1).part < profile(UXRO_2).part);
        assert_eq!(profile(EVENT).source, CatalogSourceKind::Cbro);
        assert_eq!(profile(EVENT).order_key, None);
        assert_eq!(profile(LOCG).source, CatalogSourceKind::Locg);
    }

    #[test]
    fn order_parts_are_the_same_order_in_reading_order() {
        let index = [
            MG_2_1,
            CMRO_CORE_2,
            MG_1_4,
            CMRO_MAIN_1,
            CMRO_CORE_1,
            MG_3_2,
            CBRO_7,
            UXRO_2,
            UXRO_1,
            EVENT,
        ];
        let paths =
            |p: &str| -> Vec<&str> { order_parts(p, &index).into_iter().map(|(_, p)| p).collect() };
        assert_eq!(paths(CMRO_CORE_2), [CMRO_CORE_1, CMRO_CORE_2]);
        // Marvel Guides: 1.4 is followed by 2.1, then 3.2.
        assert_eq!(paths(MG_2_1), [MG_1_4, MG_2_1, MG_3_2]);
        assert_eq!(paths(UXRO_2), [UXRO_1, UXRO_2]);
        // Alone in its order, or not in one: nothing to import together.
        assert!(paths(CMRO_MAIN_1).is_empty());
        assert!(paths(CBRO_7).is_empty());
        assert!(paths(EVENT).is_empty());
    }

    #[test]
    fn order_names_drop_the_part() {
        assert_eq!(
            order_name(CMRO_CORE_1).as_deref(),
            Some("[Marvel] CMRO Core Reading Order")
        );
        assert_eq!(
            order_name(MG_2_1).as_deref(),
            Some("Marvel Guides 2012-2015 Marvel NOW!")
        );
        assert_eq!(
            order_name(UXRO_1).as_deref(),
            Some("Ultimate X-Men Reading Order")
        );
        assert_eq!(order_name(EVENT), None);
    }

    #[test]
    fn event_files_are_picked_by_the_years_a_list_covers() {
        let index = [
            "Marvel/Events/CBRO/1986-1993 - Part 4/[Marvel] Inferno (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2012-2016 - Marvel NOW!/[Marvel] Age of Ultron (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/2015-2018 - All-New All Different Marvel/[Marvel] Civil War II (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/[Marvel] Revolutionary War (WEB-CBRO).cbl",
            "Marvel/Events/CBRO/[Marvel] Marvel Events- Marvel (WEB-CBRO).cbl",
            "Marvel/Events/LoCG/Civil War (LoCG).cbl",
            "DC/Events/CBRO/2011-2015 - New 52/[DC] Forever Evil (WEB-CBRO).cbl",
        ];
        // A Marvel list covering 2013–2015: the two overlapping eras and
        // the loose event, not the roll-up, another source or publisher.
        let got = event_files_for_years(CMRO_CORE_1, 2013, 2015, &index);
        assert_eq!(got, [index[1], index[2], index[3]]);
        assert_eq!(
            event_files_for_years(CMRO_CORE_1, 1990, 1991, &index),
            [index[0], index[3]]
        );
        assert!(event_files_for_years("", 2013, 2015, &index).is_empty());
    }
}
