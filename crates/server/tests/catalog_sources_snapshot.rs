//! Catalog audit: `cbl::sources` over the real catalog's file list.
//!
//! `tests/fixtures/catalog-paths.txt` is a snapshot of every `.cbl` path in
//! `DieselTech/CBL-ReadingLists` (refresh it with `just catalog-snapshot`).
//! The rules in `sources.rs` were written against a sample; this runs
//! them over all 1,713 files and pins what they say, so a rule that
//! misreads a real layout (the first version split each Marvel Guides era
//! into dozens of three-file "orders") fails here, and a refreshed
//! snapshot shows what changed upstream.
//!
//! No database: these are pure functions over paths.

use std::collections::{BTreeMap, HashSet};

use server::cbl::sources::{self, CatalogSourceKind};

const PATHS: &str = include_str!("fixtures/catalog-paths.txt");

fn paths() -> Vec<&'static str> {
    PATHS.lines().filter(|l| !l.is_empty()).collect()
}

/// Every order with two or more files: key → `(part, path)` in order.
fn orders(paths: &[&'static str]) -> BTreeMap<String, Vec<(f64, &'static str)>> {
    let mut out: BTreeMap<String, Vec<(f64, &'static str)>> = BTreeMap::new();
    for p in paths {
        let pr = sources::profile(p);
        if let (Some(k), Some(n)) = (pr.order_key, pr.part) {
            out.entry(k).or_default().push((n, p));
        }
    }
    out.retain(|_, v| v.len() > 1);
    for v in out.values_mut() {
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    }
    out
}

#[test]
fn every_file_has_a_source() {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for p in paths() {
        *by.entry(format!("{:?}", sources::profile(p).source))
            .or_default() += 1;
    }
    let expect: BTreeMap<String, usize> = [
        ("Cbro", 442),
        ("CbroWithEvents", 37),
        ("Cmro", 119),
        ("Locg", 189),
        ("MarvelGuides", 343),
        ("Official", 28),
        ("Other", 524),
        ("Uxro", 31),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_owned(), n))
    .collect();
    assert_eq!(by, expect);
    assert_eq!(by.values().sum::<usize>(), 1713);
}

#[test]
fn cmro_is_four_depths_numbered_without_gaps() {
    let all = paths();
    let orders = orders(&all);
    for (depth, n) in [
        ("Essential", 7),
        ("Core", 25),
        ("Main", 31),
        ("Expanded", 42),
    ] {
        let key = orders
            .keys()
            .find(|k| k.contains(&format!("/CMRO/{depth}|")))
            .unwrap_or_else(|| panic!("no CMRO {depth} order"));
        let parts: Vec<f64> = orders[key].iter().map(|(p, _)| *p).collect();
        let want: Vec<f64> = (1..=n).map(f64::from).collect();
        assert_eq!(parts, want, "CMRO {depth}");
        for (_, path) in &orders[key] {
            assert_eq!(sources::profile(path).depth.as_deref(), Some(depth));
        }
    }
    // The side orders are not a depth; all but the two-part one stand alone.
    let misc: Vec<&&str> = all.iter().filter(|p| p.contains("/CMRO/Misc/")).collect();
    assert_eq!(misc.len(), 14);
    assert!(misc.iter().all(|p| sources::profile(p).depth.is_none()));
}

#[test]
fn every_with_events_master_has_its_plain_twin() {
    let all = paths();
    let set: HashSet<&str> = all.iter().copied().collect();
    let twins: Vec<String> = all
        .iter()
        .filter_map(|p| sources::profile(p).plain_twin)
        .collect();
    assert_eq!(twins.len(), 37);
    for t in &twins {
        assert!(set.contains(t.as_str()), "no plain file for {t}");
        assert_eq!(sources::profile(t).source, CatalogSourceKind::Cbro);
    }
    // And the masters number the same on both sides.
    let orders = orders(&all);
    let count = |needle: &str| {
        orders
            .iter()
            .find(|(k, _)| k.contains(needle))
            .map(|(_, v)| v.len())
    };
    assert_eq!(
        count("Marvel/Master Reading Order/CBRO|[Marvel] Marvel Master Reading Order"),
        Some(14)
    );
    assert_eq!(
        count(
            "Marvel/Master Reading Order/CBRO - With Events|[Marvel] Marvel Master Reading Order"
        ),
        Some(14)
    );
    assert_eq!(
        count("DC/Master Reading Order/CBRO|[DC Comics] DC Master Reading Order"),
        Some(8)
    );
}

#[test]
fn a_marvel_guides_era_is_one_order() {
    let all = paths();
    let orders = orders(&all);
    let era = |name: &str| {
        orders
            .iter()
            .find(|(k, _)| k.ends_with(&format!("/Marvel Guides/{name}|mg")))
            .map(|(_, v)| v.len())
    };
    // Files sit in a sub-folder per part; the era is still one order.
    assert_eq!(era("2012-2015 Marvel NOW!"), Some(43));
    assert_eq!(era("2004-2012 Marvel Disassembled"), Some(59));
    assert_eq!(era("2018-2021 Fresh Start"), Some(63));
    assert_eq!(era("1961-1970 Silver Age"), Some(18));
    assert_eq!(era("2011-2016 New 52"), Some(7));
    // Every Marvel Guides master file is part of an era order, with a
    // part number.
    let masters: Vec<&&str> = all
        .iter()
        .filter(|p| p.contains("/Master Reading Order/Marvel Guides/"))
        .collect();
    assert_eq!(masters.len(), 342);
    for p in masters {
        let pr = sources::profile(p);
        assert!(pr.part.is_some() && pr.order_key.is_some(), "{p}");
    }
    // 1.4 is followed by 2.1: an era reads straight through its parts.
    let key = orders
        .keys()
        .find(|k| k.ends_with("/Marvel Guides/2012-2015 Marvel NOW!|mg"))
        .unwrap();
    let parts: Vec<f64> = orders[key].iter().map(|(p, _)| *p).take(6).collect();
    assert_eq!(parts, [1.1, 1.2, 1.3, 1.4, 2.1, 2.2]);
}

#[test]
fn uxro_reads_in_era_order() {
    let all = paths();
    let orders = orders(&all);
    let (_, files) = orders
        .iter()
        .find(|(k, _)| k.ends_with("|uxro"))
        .expect("the UXRO order");
    assert_eq!(files.len(), 31);
    let name = |i: usize| sources::file_stem(files[i].1);
    assert_eq!(name(0), "[1963-1974] X-Men Era One The Silver Age (UXRO)");
    // Two files starting the same year order by their end year, then name.
    assert_eq!(name(18), "[2012-2013] X-Men Era Eighteen Regenesis (UXRO)");
    assert_eq!(
        name(19),
        "[2012-2013] X-Men Era Eighteen.One The Return of The Phoenix (UXRO)"
    );
    assert_eq!(
        name(30),
        "[2023-2024] X-Men Era Twenty-Eight Fall of X (UXRO)"
    );
}

#[test]
fn order_parts_agrees_from_every_member_and_ties_are_known() {
    let all = paths();
    let orders = orders(&all);
    assert_eq!(orders.len(), 33);
    // Upstream numbers two files the same in these orders (a tie reads in
    // file-name order). A new tie after a snapshot refresh shows up here.
    let mut tied: Vec<String> = Vec::new();
    for (key, files) in &orders {
        let want: Vec<&str> = files.iter().map(|(_, p)| *p).collect();
        for member in [want[0], want[want.len() - 1]] {
            let got: Vec<&str> = sources::order_parts(member, &all)
                .into_iter()
                .map(|(_, p)| p)
                .collect();
            assert_eq!(got, want, "{key}");
        }
        if files.windows(2).any(|w| w[0].0 == w[1].0) {
            tied.push(key.clone());
        }
        assert!(sources::order_name(want[0]).is_some(), "{key}");
    }
    assert_eq!(
        tied,
        [
            "DC/Characters/Batman/Batman - Modern Age|[DC] Batman Modern Age -",
            "Marvel/Master Reading Order/Marvel Guides/1997-2004 Heroes Relaunched|mg",
            // "[2012-2013] … Eighteen" / "Eighteen.One" and the two
            // "[2014-2015]" eras: same years, read in name order.
            "Marvel/Teams/X-Men/Ultimate X-Men Reading Order|uxro",
        ]
    );
}

#[test]
fn event_candidates_follow_the_years_a_list_covers() {
    let all = paths();
    let core_1 = all
        .iter()
        .find(|p| p.ends_with("CMRO Core Reading Order-Part 01.cbl"))
        .unwrap();
    // 2013–2015: Marvel NOW! and All-New All-Different overlap, plus the
    // loose events; a detection downloads at most 60.
    let got = sources::event_files_for_years(core_1, 2013, 2015, &all);
    assert!(got.iter().any(|p| p.contains("/2012-2016 - Marvel NOW!/")));
    assert!(
        got.iter()
            .any(|p| p.contains("/2015-2018 - All-New All Different Marvel/"))
    );
    assert!(got.iter().all(|p| p.starts_with("Marvel/Events/CBRO/")));
    assert!(got.iter().all(|p| !p.contains(" Events- ")));
    assert!(!got.is_empty() && got.len() <= 60, "{}", got.len());
    // The 1960s: only the first era and the loose events.
    let early = sources::event_files_for_years(core_1, 1961, 1964, &all);
    assert!(early.iter().all(|p| !p.contains("/2012-2016")));
    assert!(early.len() < got.len());
}
