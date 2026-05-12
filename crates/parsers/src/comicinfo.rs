//! ComicInfo.xml parser (Anansi Project schema, §4.2).
//!
//! XXE-safe: built on `quick-xml`, which does not resolve external entities by default.
//! Additionally, any `<!DOCTYPE>` declaration causes a parse failure with [`ParseError::DoctypeRejected`].
//!
//! Input size is capped at 1 MiB (§A6 / §A7 in the review).

use crate::ParseError;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// Strongly typed view of the fields we extract for ranking and display.
///
/// The full XML is also re-serialized into a `BTreeMap<String, String>` so
/// new ComicInfo fields appear in `comic_info_raw` JSONB without code change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComicInfo {
    pub title: Option<String>,
    pub series: Option<String>,
    pub number: Option<String>,
    pub count: Option<i32>,
    pub volume: Option<i32>,
    pub alternate_series: Option<String>,
    pub alternate_number: Option<String>,
    pub alternate_count: Option<i32>,
    pub summary: Option<String>,
    pub notes: Option<String>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub writer: Option<String>,
    pub penciller: Option<String>,
    pub inker: Option<String>,
    pub colorist: Option<String>,
    pub letterer: Option<String>,
    pub cover_artist: Option<String>,
    pub editor: Option<String>,
    pub translator: Option<String>,
    pub publisher: Option<String>,
    pub imprint: Option<String>,
    pub genre: Option<String>,
    pub tags: Option<String>,
    pub web: Option<String>,
    pub page_count: Option<i32>,
    pub language_iso: Option<String>,
    pub format: Option<String>,
    pub black_and_white: Option<bool>,
    /// `Yes` | `YesAndRightToLeft` | `No`
    pub manga: Option<String>,
    pub characters: Option<String>,
    pub teams: Option<String>,
    pub locations: Option<String>,
    pub scan_information: Option<String>,
    pub story_arc: Option<String>,
    pub story_arc_number: Option<String>,
    pub series_group: Option<String>,
    pub age_rating: Option<String>,
    pub community_rating: Option<f64>,
    pub main_character_or_team: Option<String>,
    pub review: Option<String>,
    pub gtin: Option<String>,
    /// External database IDs. ComicInfo doesn't standardize these but ComicTagger,
    /// Mylar3, and Metron-Tagger emit non-canonical elements like `<ComicVineID>`
    /// and `<MetronID>`. We parse them from those elements when present, and
    /// fall back to extracting them from `<Web>` URLs (ComicVine encodes the ID
    /// as `4000-N` for issues / `4050-N` for series).
    pub comicvine_id: Option<i64>,
    pub metron_id: Option<i64>,
    /// Series-scope external IDs (from `<ComicVineSeriesID>`, `<MetronSeriesID>`,
    /// or extracted from a series-shaped `<Web>` URL). The scanner copies these
    /// onto the parent series row.
    pub comicvine_series_id: Option<i64>,
    pub metron_series_id: Option<i64>,
    pub pages: Vec<PageInfo>,
    /// Every leaf element by name → text content. Includes everything above plus
    /// any non-canonical fields the file ships with.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    pub image: i32,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_page: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_height: Option<i32>,
    /// `Some(true)` when `double_page` was inferred from the page's pixel
    /// aspect ratio rather than declared by ComicInfo. Lets admin / debug
    /// tooling distinguish guesses from publisher-supplied metadata; the
    /// reader treats both identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_page_inferred: Option<bool>,
}

pub fn parse(bytes: &[u8]) -> Result<ComicInfo, ParseError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ParseError::TooLarge {
            actual: bytes.len(),
            limit: MAX_INPUT_BYTES,
        });
    }

    let mut reader = Reader::from_reader(bytes);
    let cfg = reader.config_mut();
    cfg.trim_text(true);
    cfg.expand_empty_elements = true;
    // quick-xml does not resolve entities other than the five XML predefined ones
    // by default, so XXE is a non-issue. We additionally reject DOCTYPE.

    let mut info = ComicInfo::default();
    let mut buf = Vec::with_capacity(2048);
    let mut path: Vec<String> = Vec::with_capacity(8);
    let mut current_text = String::new();
    // Page-element accumulation
    let mut current_page_attrs: BTreeMap<String, String> = BTreeMap::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::DocType(_)) => return Err(ParseError::DoctypeRejected),
            Ok(Event::Start(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| ParseError::Malformed(e.to_string()))?
                    .to_string();
                if name == "Page" {
                    current_page_attrs.clear();
                    for attr in e.attributes().with_checks(false).flatten() {
                        let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let v = attr
                            .unescape_value()
                            .map(|c| c.into_owned())
                            .unwrap_or_default();
                        current_page_attrs.insert(k, v);
                    }
                }
                path.push(name);
                current_text.clear();
            }
            Ok(Event::Empty(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| ParseError::Malformed(e.to_string()))?
                    .to_string();
                if name == "Page" {
                    let mut attrs = BTreeMap::new();
                    for attr in e.attributes().with_checks(false).flatten() {
                        let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let v = attr
                            .unescape_value()
                            .map(|c| c.into_owned())
                            .unwrap_or_default();
                        attrs.insert(k, v);
                    }
                    if let Some(p) = page_from_attrs(&attrs) {
                        info.pages.push(p);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| ParseError::Malformed(e.to_string()))?
                    .to_string();
                if name == "Page" {
                    if let Some(p) = page_from_attrs(&current_page_attrs) {
                        info.pages.push(p);
                    }
                    current_page_attrs.clear();
                } else if path.last().map(|s| s.as_str()) == Some(name.as_str()) {
                    let depth = path.len();
                    if depth == 2 {
                        // Direct child of <ComicInfo>; assign + populate raw map.
                        let value = std::mem::take(&mut current_text);
                        let trimmed = value.trim().to_string();
                        if !trimmed.is_empty() {
                            assign(&mut info, &name, &trimmed);
                            info.raw.insert(name.clone(), trimmed);
                        }
                    }
                }
                path.pop();
            }
            Ok(Event::Text(t)) => {
                let s = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                current_text.push_str(&s);
            }
            Ok(Event::CData(t)) => {
                current_text.push_str(&String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Malformed(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    // ComicVine URL fallback: if we still don't have explicit IDs, try to
    // recover them from the `<Web>` URL. Multiple URLs can be space- or
    // newline-separated; we scan all of them. Explicit fields always win.
    if let Some(web) = info.web.as_deref()
        && (info.comicvine_id.is_none() || info.comicvine_series_id.is_none())
    {
        for url in web.split_whitespace() {
            let (issue, series) = ids_from_comicvine_url(url);
            if info.comicvine_id.is_none()
                && let Some(n) = issue
            {
                info.comicvine_id = Some(n);
            }
            if info.comicvine_series_id.is_none()
                && let Some(n) = series
            {
                info.comicvine_series_id = Some(n);
            }
        }
    }

    Ok(info)
}

fn page_from_attrs(attrs: &BTreeMap<String, String>) -> Option<PageInfo> {
    let image: i32 = attrs.get("Image")?.parse().ok()?;
    Some(PageInfo {
        image,
        kind: attrs.get("Type").cloned(),
        double_page: attrs.get("DoublePage").and_then(|v| v.parse().ok()),
        image_size: attrs.get("ImageSize").and_then(|v| v.parse().ok()),
        key: attrs.get("Key").cloned(),
        bookmark: attrs.get("Bookmark").cloned(),
        image_width: attrs.get("ImageWidth").and_then(|v| v.parse().ok()),
        image_height: attrs.get("ImageHeight").and_then(|v| v.parse().ok()),
        // ComicInfo never declares this — it's only set by the scanner's
        // dimension-probe fallback.
        double_page_inferred: None,
    })
}

fn assign(info: &mut ComicInfo, name: &str, val: &str) {
    macro_rules! str_field {
        ($f:ident) => {
            info.$f = Some(val.to_string())
        };
    }
    macro_rules! int_field {
        ($f:ident) => {
            if let Ok(n) = val.parse() {
                info.$f = Some(n)
            }
        };
    }
    macro_rules! float_field {
        ($f:ident) => {
            if let Ok(n) = val.parse() {
                info.$f = Some(n)
            }
        };
    }
    match name {
        "Title" => str_field!(title),
        "Series" => str_field!(series),
        "Number" => str_field!(number),
        "Count" => int_field!(count),
        "Volume" => int_field!(volume),
        "AlternateSeries" => str_field!(alternate_series),
        "AlternateNumber" => str_field!(alternate_number),
        "AlternateCount" => int_field!(alternate_count),
        "Summary" => str_field!(summary),
        "Notes" => str_field!(notes),
        "Year" => int_field!(year),
        "Month" => int_field!(month),
        "Day" => int_field!(day),
        "Writer" => str_field!(writer),
        "Penciller" => str_field!(penciller),
        "Inker" => str_field!(inker),
        "Colorist" => str_field!(colorist),
        "Letterer" => str_field!(letterer),
        "CoverArtist" => str_field!(cover_artist),
        "Editor" => str_field!(editor),
        "Translator" => str_field!(translator),
        "Publisher" => str_field!(publisher),
        "Imprint" => str_field!(imprint),
        "Genre" => str_field!(genre),
        "Tags" => str_field!(tags),
        "Web" => str_field!(web),
        "PageCount" => int_field!(page_count),
        "LanguageISO" => str_field!(language_iso),
        "Format" => str_field!(format),
        "BlackAndWhite" => {
            info.black_and_white = match val.to_ascii_lowercase().as_str() {
                "yes" | "true" => Some(true),
                "no" | "false" => Some(false),
                _ => None,
            };
        }
        "Manga" => str_field!(manga),
        "Characters" => str_field!(characters),
        "Teams" => str_field!(teams),
        "Locations" => str_field!(locations),
        "ScanInformation" => str_field!(scan_information),
        "StoryArc" => str_field!(story_arc),
        "StoryArcNumber" => str_field!(story_arc_number),
        "SeriesGroup" => str_field!(series_group),
        "AgeRating" => str_field!(age_rating),
        "CommunityRating" => float_field!(community_rating),
        "MainCharacterOrTeam" => str_field!(main_character_or_team),
        "Review" => str_field!(review),
        "GTIN" => str_field!(gtin),
        // External-database IDs. Tag names vary across taggers; accept a few
        // common spellings. Extract digits from the value so URLs paste-ins
        // (e.g. `4000-12345`) still resolve to the numeric id.
        "ComicVineID" | "ComicVineId" | "ComicvineID" | "Comicvineid" => {
            if let Some(n) = parse_id_from_value(val) {
                info.comicvine_id = Some(n);
            }
        }
        "MetronID" | "MetronId" | "MetronInfoIssueID" => {
            if let Some(n) = parse_id_from_value(val) {
                info.metron_id = Some(n);
            }
        }
        "ComicVineSeriesID" | "ComicVineSeriesId" | "ComicVineVolumeID" | "ComicVineVolumeId" => {
            if let Some(n) = parse_id_from_value(val) {
                info.comicvine_series_id = Some(n);
            }
        }
        "MetronSeriesID" | "MetronSeriesId" | "MetronInfoSeriesID" => {
            if let Some(n) = parse_id_from_value(val) {
                info.metron_series_id = Some(n);
            }
        }
        _ => {} // unknown leaf — kept in `raw`
    }
}

/// Parse a numeric ID from either a plain number ("12345") or the
/// ComicVine-style `prefix-digits` form ("4000-12345"). Returns the trailing
/// digit run on success.
fn parse_id_from_value(val: &str) -> Option<i64> {
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Some(n);
    }
    // ComicVine taggers sometimes write the full `4000-12345` token.
    let last = trimmed
        .rsplit_once('-')
        .map(|(_, tail)| tail)
        .unwrap_or(trimmed);
    last.parse::<i64>().ok()
}

/// Extract a `4000-N` (issue) or `4050-N` (series/volume) ComicVine id from a
/// canonical comicvine.gamespot.com URL. Returns `(issue_id, series_id)`.
fn ids_from_comicvine_url(url: &str) -> (Option<i64>, Option<i64>) {
    let lower = url.to_ascii_lowercase();
    if !lower.contains("comicvine.gamespot.com") {
        return (None, None);
    }
    let mut issue = None;
    let mut series = None;
    // Walk path segments looking for `<prefix>-<digits>` tokens.
    for seg in url.split(['/', '?', '#', '&']) {
        if let Some((prefix, tail)) = seg.split_once('-')
            && let Ok(n) = tail.parse::<i64>()
        {
            match prefix {
                "4000" => issue = Some(n),
                "4050" | "4060" => series = Some(n),
                _ => {}
            }
        }
    }
    (issue, series)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ComicInfo>
  <Title>The Boy from Mars</Title>
  <Series>Saga</Series>
  <Number>1</Number>
  <Count>54</Count>
  <Volume>1</Volume>
  <Summary>An interplanetary love story.</Summary>
  <Year>2012</Year>
  <Month>3</Month>
  <Writer>Brian K. Vaughan</Writer>
  <Penciller>Fiona Staples</Penciller>
  <Publisher>Image Comics</Publisher>
  <PageCount>44</PageCount>
  <LanguageISO>en</LanguageISO>
  <BlackAndWhite>No</BlackAndWhite>
  <Manga>No</Manga>
  <AgeRating>Mature 17+</AgeRating>
  <Pages>
    <Page Image="0" Type="FrontCover" ImageSize="123456" ImageWidth="1024" ImageHeight="1536"/>
    <Page Image="1" ImageSize="234567"/>
    <Page Image="2" Type="Story" DoublePage="true"/>
  </Pages>
  <Web>https://example.com/saga/1</Web>
  <CustomField>not part of schema</CustomField>
</ComicInfo>"#;

    #[test]
    fn parses_known_fields() {
        let info = parse(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(info.title.as_deref(), Some("The Boy from Mars"));
        assert_eq!(info.series.as_deref(), Some("Saga"));
        assert_eq!(info.number.as_deref(), Some("1"));
        assert_eq!(info.count, Some(54));
        assert_eq!(info.year, Some(2012));
        assert_eq!(info.writer.as_deref(), Some("Brian K. Vaughan"));
        assert_eq!(info.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(info.page_count, Some(44));
        assert_eq!(info.black_and_white, Some(false));
        assert_eq!(info.manga.as_deref(), Some("No"));
        assert_eq!(info.age_rating.as_deref(), Some("Mature 17+"));
        assert_eq!(info.pages.len(), 3);
        assert_eq!(info.pages[2].double_page, Some(true));
        assert_eq!(info.pages[0].kind.as_deref(), Some("FrontCover"));
        assert_eq!(info.pages[0].image_width, Some(1024));
    }

    #[test]
    fn unknown_fields_kept_in_raw() {
        let info = parse(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(
            info.raw.get("CustomField"),
            Some(&"not part of schema".to_string())
        );
        // Known fields land in raw too.
        assert_eq!(info.raw.get("Series"), Some(&"Saga".to_string()));
    }

    #[test]
    fn doctype_is_rejected_xxe_safe() {
        let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>
<ComicInfo><Title>&xxe;</Title></ComicInfo>"#;
        let err = parse(xxe.as_bytes()).expect_err("must reject");
        assert!(matches!(err, ParseError::DoctypeRejected));
    }

    #[test]
    fn oversize_is_rejected() {
        let huge = vec![b'x'; MAX_INPUT_BYTES + 1];
        let err = parse(&huge).expect_err("must reject");
        assert!(matches!(err, ParseError::TooLarge { .. }));
    }

    #[test]
    fn mismatched_close_tag_yields_error() {
        let err = parse(b"<ComicInfo><Series>oops</WrongClose></ComicInfo>")
            .expect_err("mismatched close must reject");
        assert!(matches!(err, ParseError::Malformed(_)));
    }

    #[test]
    fn empty_xml_returns_default() {
        let info = parse(b"<ComicInfo></ComicInfo>").expect("parse");
        assert!(info.title.is_none());
        assert!(info.pages.is_empty());
    }

    #[test]
    fn parses_external_database_ids_from_explicit_tags() {
        let xml = r#"<ComicInfo>
  <ComicVineID>12345</ComicVineID>
  <MetronID>987</MetronID>
  <ComicVineSeriesID>4050-67890</ComicVineSeriesID>
</ComicInfo>"#;
        let info = parse(xml.as_bytes()).unwrap();
        assert_eq!(info.comicvine_id, Some(12345));
        assert_eq!(info.metron_id, Some(987));
        // Prefix-stripped tail is stored.
        assert_eq!(info.comicvine_series_id, Some(67890));
    }

    #[test]
    fn extracts_comicvine_ids_from_web_url_when_tags_absent() {
        let xml = r#"<ComicInfo>
  <Web>https://comicvine.gamespot.com/saga-1/4000-381432/ https://comicvine.gamespot.com/saga/4050-49901/</Web>
</ComicInfo>"#;
        let info = parse(xml.as_bytes()).unwrap();
        assert_eq!(info.comicvine_id, Some(381432));
        assert_eq!(info.comicvine_series_id, Some(49901));
    }

    #[test]
    fn explicit_tags_win_over_web_url_extraction() {
        // Explicit tag overrides the URL-extracted value.
        let xml = r#"<ComicInfo>
  <ComicVineID>1</ComicVineID>
  <Web>https://comicvine.gamespot.com/x/4000-99/</Web>
</ComicInfo>"#;
        let info = parse(xml.as_bytes()).unwrap();
        assert_eq!(info.comicvine_id, Some(1));
    }

    #[test]
    fn manga_right_to_left_preserved() {
        let xml = r#"<ComicInfo><Manga>YesAndRightToLeft</Manga></ComicInfo>"#;
        let info = parse(xml.as_bytes()).unwrap();
        assert_eq!(info.manga.as_deref(), Some("YesAndRightToLeft"));
    }

    #[test]
    fn double_page_inferred_round_trips_as_optional() {
        // The ComicInfo XML never declares this field — it's only set by
        // the scanner's dimension-probe fallback. Parsing always produces
        // None; explicit construction round-trips through JSON.
        let info = parse(SAMPLE.as_bytes()).expect("parse");
        assert!(info.pages.iter().all(|p| p.double_page_inferred.is_none()));

        // JSON round-trip with the field set
        let mut p = PageInfo {
            image: 5,
            kind: None,
            double_page: Some(true),
            image_size: None,
            key: None,
            bookmark: None,
            image_width: Some(3976),
            image_height: Some(3056),
            double_page_inferred: Some(true),
        };
        let j = serde_json::to_value(&p).unwrap();
        let back: PageInfo = serde_json::from_value(j).unwrap();
        assert_eq!(back.double_page_inferred, Some(true));

        // None must serialize away entirely (skip_serializing_if).
        p.double_page_inferred = None;
        let j = serde_json::to_value(&p).unwrap();
        assert!(j.get("double_page_inferred").is_none());
    }
}
