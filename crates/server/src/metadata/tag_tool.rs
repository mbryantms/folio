//! Which tagging tool wrote an archive's ComicInfo — read off the `Notes`
//! trace every tagger leaves (`Tagged with ComicTagger … using info from
//! Metron …`, `Tagged with Folio on …`, Mylar's `CVDB` genre marker,
//! ComicRack-era `Scraped metadata from …`). The admin Coverage tab groups
//! the library by it, and a series' Metadata tab shows it per issue, so the
//! operator can see where ids came from and which tool's ids the scanner
//! never read.
//!
//! One classification, two forms: [`classify`] for rows already in hand and
//! [`sql_case`] for aggregates over the whole library. The unit tests pin
//! the two to the same answers.

/// The tool keys, in the order the Coverage tab lists them.
pub const TOOLS: [&str; 8] = [
    "folio",
    "comictagger_comicvine",
    "comictagger_metron",
    "comictagger_gcd",
    "comictagger_other",
    "mylar",
    "scraper",
    "untagged",
];

/// Classify one issue's ComicInfo `Notes`, `Genre` and `Web` values.
pub fn classify(notes: &str, genre: &str, web: &str) -> &'static str {
    let n = notes.to_ascii_lowercase();
    let g = genre.to_ascii_lowercase();
    if n.starts_with("tagged with folio") {
        return "folio";
    }
    if n.contains("comictagger") {
        if n.contains("from metron") {
            return "comictagger_metron";
        }
        if n.contains("from comic vine") || n.contains("from comicvine") {
            return "comictagger_comicvine";
        }
        if n.contains("grand comics") {
            return "comictagger_gcd";
        }
        return "comictagger_other";
    }
    if n.contains("mylar") || g.contains("cvdb") {
        return "mylar";
    }
    if n.contains("scraped") || n.contains("[cvdb") || !web.trim().is_empty() {
        return "scraper";
    }
    "untagged"
}

/// The same classification as a SQL `CASE` over the given column
/// expressions (each a text expression, never NULL).
pub fn sql_case(notes: &str, genre: &str, web: &str) -> String {
    format!(
        "CASE \
           WHEN {notes} ILIKE 'Tagged with Folio%' THEN 'folio' \
           WHEN {notes} ILIKE '%ComicTagger%' AND {notes} ILIKE '%from Metron%' THEN 'comictagger_metron' \
           WHEN {notes} ILIKE '%ComicTagger%' AND ({notes} ILIKE '%from Comic Vine%' OR {notes} ILIKE '%from ComicVine%') THEN 'comictagger_comicvine' \
           WHEN {notes} ILIKE '%ComicTagger%' AND {notes} ILIKE '%Grand Comics%' THEN 'comictagger_gcd' \
           WHEN {notes} ILIKE '%ComicTagger%' THEN 'comictagger_other' \
           WHEN {notes} ILIKE '%Mylar%' OR {genre} ILIKE '%CVDB%' THEN 'mylar' \
           WHEN {notes} ILIKE '%Scraped%' OR {notes} ILIKE '%[CVDB%' OR btrim({web}) <> '' THEN 'scraper' \
           ELSE 'untagged' END"
    )
}

/// Human label for a tool key.
pub fn label(tool: &str) -> &'static str {
    match tool {
        "folio" => "Folio",
        "comictagger_comicvine" => "ComicTagger · ComicVine",
        "comictagger_metron" => "ComicTagger · Metron",
        "comictagger_gcd" => "ComicTagger · GCD",
        "comictagger_other" => "ComicTagger",
        "mylar" => "Mylar",
        "scraper" => "Other scraper",
        _ => "No tags",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_known_traces() {
        assert_eq!(
            classify("Tagged with Folio on 2026-10-10 — sources Metron", "", ""),
            "folio"
        );
        assert_eq!(
            classify(
                "Tagged with ComicTagger 1.6.0b5.dev0 using info from Metron on 2025-05-02. [Issue ID 28577]",
                "",
                "https://metron.cloud/issue/detective-comics-1937-514/"
            ),
            "comictagger_metron"
        );
        assert_eq!(
            classify(
                "Tagged with ComicTagger 1.5.5 using info from Comic Vine on 2024-01-01. [Issue ID 1]",
                "Superhero",
                ""
            ),
            "comictagger_comicvine"
        );
        assert_eq!(
            classify(
                "Tagged with ComicTagger 1.6 using info from Grand Comics Database on 2025-01-01.",
                "",
                ""
            ),
            "comictagger_gcd"
        );
        assert_eq!(
            classify("Tagged with ComicTagger 1.3.5 on 2020-01-01.", "", ""),
            "comictagger_other"
        );
        assert_eq!(classify("", "Superhero, CVDB34263", ""), "mylar");
        assert_eq!(
            classify("Scraped metadata from ComicVine [CVDB121185].", "", ""),
            "scraper"
        );
        assert_eq!(
            classify("", "", "https://comicvine.gamespot.com/batman-466/4000-1/"),
            "scraper"
        );
        assert_eq!(classify("", "", ""), "untagged");
        assert_eq!(classify("   ", "Horror", "  "), "untagged");
    }

    #[test]
    fn every_tool_has_a_label() {
        for t in TOOLS {
            assert_ne!(label(t), "", "{t}");
        }
        assert_eq!(label("untagged"), "No tags");
        assert_eq!(label("nonsense"), "No tags");
    }

    #[test]
    fn sql_case_names_every_tool() {
        let sql = sql_case("n", "g", "w");
        for t in TOOLS {
            assert!(sql.contains(&format!("'{t}'")), "{t} missing from {sql}");
        }
    }
}
