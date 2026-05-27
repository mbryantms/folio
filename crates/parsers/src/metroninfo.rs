//! MetronInfo.xml parser (§4.4).
//!
//! Same defensive posture as `comicinfo`: XXE-safe (DOCTYPE rejected), 1 MiB cap.
//!
//! MetronInfo is structurally similar to ComicInfo but with richer creator
//! credits (one element per creator with a role attribute) and proper IDs
//! (`<ID source="metron">123</ID>`). For Phase 1b we extract a curated subset
//! that overlaps with our `comic_info_raw` storage; everything else lands in
//! `raw` for forward-compat.
//!
//! When both ComicInfo and MetronInfo are present in the same archive, the
//! caller merges with precedence:
//! per-issue ComicInfo > MetronInfo > series.json > filename inference (§4.3).
//! MetronInfo's role-tagged creators are flattened into the
//! `Writer/Penciller/...` strings expected by downstream code.

use crate::ParseError;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetronInfo {
    pub title: Option<String>,
    pub series: Option<String>,
    pub publisher: Option<String>,
    pub imprint: Option<String>,
    pub number: Option<String>,
    pub volume: Option<i32>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub summary: Option<String>,
    pub notes: Option<String>,
    pub age_rating: Option<String>,
    pub language: Option<String>,
    pub manga: Option<String>,
    pub gtin: Option<String>,
    pub story_arcs: Vec<String>,
    pub characters: Vec<String>,
    pub teams: Vec<String>,
    pub locations: Vec<String>,
    pub tags: Vec<String>,
    pub genres: Vec<String>,
    /// External IDs by source: `{"metron": 123, "comicvine": 456}`.
    pub ids: BTreeMap<String, String>,
    /// Creators grouped by role. Multiple credits with the same role are joined with `, `.
    pub credits: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, String>,
}

impl MetronInfo {
    /// Convenience: comma-joined writer credits, if any.
    pub fn writer(&self) -> Option<String> {
        self.credit_string("Writer")
    }
    pub fn penciller(&self) -> Option<String> {
        self.credit_string("Penciller")
    }
    pub fn inker(&self) -> Option<String> {
        self.credit_string("Inker")
    }
    pub fn colorist(&self) -> Option<String> {
        self.credit_string("Colorist")
    }
    pub fn letterer(&self) -> Option<String> {
        self.credit_string("Letterer")
    }
    pub fn cover_artist(&self) -> Option<String> {
        self.credit_string("CoverArtist")
    }
    pub fn editor(&self) -> Option<String> {
        self.credit_string("Editor")
    }
    pub fn translator(&self) -> Option<String> {
        self.credit_string("Translator")
    }

    fn credit_string(&self, role: &str) -> Option<String> {
        self.credits
            .get(role)
            .filter(|v| !v.is_empty())
            .map(|v| v.join(", "))
    }
}

pub fn parse(bytes: &[u8]) -> Result<MetronInfo, ParseError> {
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

    let mut info = MetronInfo::default();
    let mut buf = Vec::with_capacity(2048);
    let mut path: Vec<String> = Vec::with_capacity(16);
    let mut text = String::new();
    let mut current_creator_role: Option<String> = None;
    let mut current_id_source: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::DocType(_)) => return Err(ParseError::DoctypeRejected),
            Ok(Event::Start(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|err| ParseError::Malformed(err.to_string()))?
                    .to_string();
                if name == "Credit" {
                    current_creator_role = None;
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.as_ref() == b"role" {
                            current_creator_role = attr
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()
                                .map(|c| c.into_owned());
                        }
                    }
                } else if name == "ID" {
                    current_id_source = None;
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.as_ref() == b"source" {
                            current_id_source = attr
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()
                                .map(|c| c.into_owned());
                        }
                    }
                }
                path.push(name);
                text.clear();
            }
            Ok(Event::End(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|err| ParseError::Malformed(err.to_string()))?
                    .to_string();
                if path.last().map(|s| s.as_str()) == Some(name.as_str()) {
                    let value = std::mem::take(&mut text);
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        assign(
                            &mut info,
                            &path,
                            &name,
                            &value,
                            &mut current_creator_role,
                            &mut current_id_source,
                        );
                        info.raw.insert(name.clone(), value);
                    }
                }
                path.pop();
                if name == "Credit" {
                    current_creator_role = None;
                }
                if name == "ID" {
                    current_id_source = None;
                }
            }
            Ok(Event::Text(t)) => {
                // quick-xml 0.40: `BytesText::unescape()` removed;
                // chain `decode()` + `escape::unescape()`. Errors fall
                // back to empty (matches the old `.unwrap_or_default`).
                let s = t
                    .decode()
                    .ok()
                    .and_then(|d| quick_xml::escape::unescape(&d).ok().map(|u| u.into_owned()))
                    .unwrap_or_default();
                text.push_str(&s);
            }
            Ok(Event::CData(t)) => {
                text.push_str(&String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Malformed(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(info)
}

/// Emit a MetronInfo.xml document from `info`. UTF-8, 2-space indent.
///
/// Element order matches the de-facto MetronInfo schema (Metron-Tagger
/// output) so a parse → serialize round-trip produces a stable diff.
///
/// Rules:
///
///   - Scalar fields are emitted first in schema order, omitting empty
///     / `None` values.
///   - `<ID source="…">…</ID>` elements come next, sorted by source key
///     for deterministic output.
///   - List elements (`StoryArcs`, `Characters`, `Teams`, `Locations`,
///     `Tags`, `Genres`) are emitted only when the corresponding `Vec`
///     is non-empty, in canonical container/leaf form
///     (`<StoryArcs><StoryArc>…</StoryArc></StoryArcs>`).
///   - `<Credits>` is emitted from the `credits` BTreeMap: one `<Credit>`
///     per (role, creator) pair, preserving multiplicity (Vec order).
///     Roles are iterated in BTreeMap key order; creators within a role
///     in Vec order.
///   - Unknown scalar leafs in [`MetronInfo::raw`] are passed through
///     after the typed scalars but before the list elements. Entries
///     matching a typed field name are not re-emitted (the typed value
///     wins, even if the caller mutated the struct without updating
///     `raw`).
///   - Text values are XML-escaped via [`escape_xml_text`].
///
/// Module M1 of [`metadata-sidecar-writeback-1.0`](../../../../../.claude/plans/metadata-sidecar-writeback-1.0.md).
pub fn serialize(info: &MetronInfo) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<MetronInfo xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">\n");

    write_opt_str(&mut out, "Title", &info.title);
    write_opt_str(&mut out, "Series", &info.series);
    write_opt_str(&mut out, "Publisher", &info.publisher);
    write_opt_str(&mut out, "Imprint", &info.imprint);
    write_opt_str(&mut out, "Number", &info.number);
    write_opt_int(&mut out, "Volume", info.volume);
    write_opt_int(&mut out, "Year", info.year);
    write_opt_int(&mut out, "Month", info.month);
    write_opt_int(&mut out, "Day", info.day);
    write_opt_str(&mut out, "Summary", &info.summary);
    write_opt_str(&mut out, "Notes", &info.notes);
    write_opt_str(&mut out, "AgeRating", &info.age_rating);
    write_opt_str(&mut out, "Language", &info.language);
    write_opt_str(&mut out, "Manga", &info.manga);
    write_opt_str(&mut out, "GTIN", &info.gtin);

    // Raw passthrough for unknown scalar elements. Done after typed
    // scalars; before lists. Filters typed names so duplicates don't
    // appear when the parser populated raw alongside the typed field.
    for (k, v) in &info.raw {
        if is_typed_metron_info_leaf(k) {
            continue;
        }
        write_text(&mut out, k, v);
    }

    // External IDs — `<ID source="…">value</ID>`. BTreeMap iterates in
    // key order, so output is deterministic.
    for (source, value) in &info.ids {
        out.push_str("  <ID source=\"");
        escape_xml_attr(&mut out, source);
        out.push_str("\">");
        escape_xml_text(&mut out, value);
        out.push_str("</ID>\n");
    }

    // Lists.
    write_list(&mut out, "StoryArcs", "StoryArc", &info.story_arcs);
    write_list(&mut out, "Characters", "Character", &info.characters);
    write_list(&mut out, "Teams", "Team", &info.teams);
    write_list(&mut out, "Locations", "Location", &info.locations);
    write_list(&mut out, "Tags", "Tag", &info.tags);
    write_list(&mut out, "Genres", "Genre", &info.genres);

    // Credits — last block.
    if !info.credits.is_empty() && info.credits.values().any(|v| !v.is_empty()) {
        out.push_str("  <Credits>\n");
        for (role, creators) in &info.credits {
            for creator in creators {
                out.push_str("    <Credit role=\"");
                escape_xml_attr(&mut out, role);
                out.push_str("\">\n");
                out.push_str("      <Creator><Name>");
                escape_xml_text(&mut out, creator);
                out.push_str("</Name></Creator>\n");
                out.push_str("    </Credit>\n");
            }
        }
        out.push_str("  </Credits>\n");
    }

    out.push_str("</MetronInfo>\n");
    out
}

fn is_typed_metron_info_leaf(name: &str) -> bool {
    matches!(
        name,
        "Title"
            | "Series"
            | "Publisher"
            | "Imprint"
            | "Number"
            | "Volume"
            | "Year"
            | "Month"
            | "Day"
            | "Summary"
            | "Notes"
            | "AgeRating"
            | "Language"
            | "Manga"
            | "GTIN"
            // Containers + their leaf names — never re-emit raw form
            // (the lists themselves were structured under the right
            // parent, and the parser stores each terminal leaf into
            // `raw` under its own name).
            | "StoryArcs"
            | "StoryArc"
            | "Characters"
            | "Character"
            | "Teams"
            | "Team"
            | "Locations"
            | "Location"
            | "Tags"
            | "Tag"
            | "Genres"
            | "Genre"
            | "Credits"
            | "Credit"
            | "Creator"
            | "Name"
            | "ID"
    )
}

fn write_list(out: &mut String, container: &str, leaf: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str("  <");
    out.push_str(container);
    out.push_str(">\n");
    for it in items {
        out.push_str("    <");
        out.push_str(leaf);
        out.push('>');
        escape_xml_text(out, it);
        out.push_str("</");
        out.push_str(leaf);
        out.push_str(">\n");
    }
    out.push_str("  </");
    out.push_str(container);
    out.push_str(">\n");
}

fn write_opt_str(out: &mut String, name: &str, v: &Option<String>) {
    if let Some(s) = v.as_deref().filter(|s| !s.trim().is_empty()) {
        write_text(out, name, s);
    }
}

fn write_opt_int(out: &mut String, name: &str, v: Option<i32>) {
    if let Some(n) = v {
        write_text(out, name, &n.to_string());
    }
}

fn write_text(out: &mut String, name: &str, value: &str) {
    out.push_str("  <");
    out.push_str(name);
    out.push('>');
    escape_xml_text(out, value);
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}

fn escape_xml_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
}

fn escape_xml_attr(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

fn assign(
    info: &mut MetronInfo,
    path: &[String],
    name: &str,
    val: &str,
    current_role: &mut Option<String>,
    current_id_source: &mut Option<String>,
) {
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

    // List-style elements: collect from <Tags><Tag>x</Tag></Tags> shape.
    let parent = path.iter().rev().nth(1).map(String::as_str);
    let leaf_into_list = match (parent, name) {
        (Some("StoryArcs"), "StoryArc") => Some(&mut info.story_arcs),
        (Some("Characters"), "Character") => Some(&mut info.characters),
        (Some("Teams"), "Team") => Some(&mut info.teams),
        (Some("Locations"), "Location") => Some(&mut info.locations),
        (Some("Tags"), "Tag") => Some(&mut info.tags),
        (Some("Genres"), "Genre") => Some(&mut info.genres),
        _ => None,
    };
    if let Some(list) = leaf_into_list {
        list.push(val.to_string());
        return;
    }

    if name == "Name" && parent == Some("Creator") {
        if let Some(role) = current_role.as_ref() {
            info.credits
                .entry(role.clone())
                .or_default()
                .push(val.to_string());
        }
        return;
    }

    if name == "ID" {
        let key = current_id_source
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        info.ids.insert(key, val.to_string());
        return;
    }

    // Scalar fields are direct children of <MetronInfo>.
    if path.len() != 2 {
        return;
    }
    match name {
        "Title" => str_field!(title),
        "Series" => str_field!(series),
        "Publisher" => str_field!(publisher),
        "Imprint" => str_field!(imprint),
        "Number" => str_field!(number),
        "Volume" => int_field!(volume),
        "Year" => int_field!(year),
        "Month" => int_field!(month),
        "Day" => int_field!(day),
        "Summary" => str_field!(summary),
        "Notes" => str_field!(notes),
        "AgeRating" => str_field!(age_rating),
        "Language" => str_field!(language),
        "Manga" => str_field!(manga),
        "GTIN" => str_field!(gtin),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetronInfo>
  <Title>The Boy from Mars</Title>
  <Series>Saga</Series>
  <Publisher>Image Comics</Publisher>
  <Number>1</Number>
  <Volume>1</Volume>
  <Year>2012</Year>
  <Month>3</Month>
  <Summary>An interplanetary love story.</Summary>
  <AgeRating>Mature 17+</AgeRating>
  <Manga>No</Manga>
  <ID source="metron">12345</ID>
  <ID source="comicvine">67890</ID>
  <StoryArcs>
    <StoryArc>The Will</StoryArc>
    <StoryArc>Volume 1</StoryArc>
  </StoryArcs>
  <Characters>
    <Character>Alana</Character>
    <Character>Marko</Character>
  </Characters>
  <Credits>
    <Credit role="Writer">
      <Creator><Name>Brian K. Vaughan</Name></Creator>
    </Credit>
    <Credit role="Penciller">
      <Creator><Name>Fiona Staples</Name></Creator>
    </Credit>
    <Credit role="Penciller">
      <Creator><Name>(Co-artist)</Name></Creator>
    </Credit>
  </Credits>
</MetronInfo>"#;

    #[test]
    fn parses_known_fields_and_credits() {
        let info = parse(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(info.title.as_deref(), Some("The Boy from Mars"));
        assert_eq!(info.series.as_deref(), Some("Saga"));
        assert_eq!(info.year, Some(2012));
        assert_eq!(info.story_arcs, vec!["The Will", "Volume 1"]);
        assert_eq!(info.characters, vec!["Alana", "Marko"]);
        assert_eq!(
            info.credits.get("Penciller").map(|v| v.join(", ")),
            Some("Fiona Staples, (Co-artist)".to_string())
        );
        assert_eq!(info.writer().as_deref(), Some("Brian K. Vaughan"));
        assert_eq!(
            info.penciller().as_deref(),
            Some("Fiona Staples, (Co-artist)")
        );
        assert_eq!(info.ids.get("metron").map(String::as_str), Some("12345"));
        assert_eq!(info.ids.get("comicvine").map(String::as_str), Some("67890"));
    }

    #[test]
    fn serialize_round_trip_preserves_scalars() {
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        let reparsed = parse(xml.as_bytes()).expect("reparse");

        assert_eq!(reparsed.title, parsed.title);
        assert_eq!(reparsed.series, parsed.series);
        assert_eq!(reparsed.publisher, parsed.publisher);
        assert_eq!(reparsed.number, parsed.number);
        assert_eq!(reparsed.volume, parsed.volume);
        assert_eq!(reparsed.year, parsed.year);
        assert_eq!(reparsed.month, parsed.month);
        assert_eq!(reparsed.summary, parsed.summary);
        assert_eq!(reparsed.age_rating, parsed.age_rating);
        assert_eq!(reparsed.manga, parsed.manga);
    }

    #[test]
    fn serialize_round_trip_preserves_lists() {
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        let reparsed = parse(xml.as_bytes()).expect("reparse");

        assert_eq!(reparsed.story_arcs, parsed.story_arcs);
        assert_eq!(reparsed.characters, parsed.characters);
    }

    #[test]
    fn serialize_round_trip_preserves_credits_with_same_role() {
        // SAMPLE has two `Penciller` credits — Fiona Staples + (Co-artist).
        // The Vec must round-trip with multiplicity AND order preserved.
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        let reparsed = parse(xml.as_bytes()).expect("reparse");

        assert_eq!(
            reparsed.credits.get("Penciller").map(Vec::as_slice),
            Some(["Fiona Staples".to_string(), "(Co-artist)".to_string()].as_slice()),
        );
        assert_eq!(
            reparsed.credits.get("Writer").map(Vec::as_slice),
            Some(["Brian K. Vaughan".to_string()].as_slice()),
        );
    }

    #[test]
    fn serialize_round_trip_preserves_external_ids() {
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        // Each `<ID source="…">value</ID>` round-trips.
        assert!(xml.contains(r#"<ID source="metron">12345</ID>"#), "{xml}");
        assert!(xml.contains(r#"<ID source="comicvine">67890</ID>"#), "{xml}");

        let reparsed = parse(xml.as_bytes()).expect("reparse");
        assert_eq!(reparsed.ids.get("metron").map(String::as_str), Some("12345"));
        assert_eq!(
            reparsed.ids.get("comicvine").map(String::as_str),
            Some("67890"),
        );
    }

    #[test]
    fn serialize_passes_through_unknown_raw_scalars() {
        let xml = r#"<?xml version="1.0"?>
<MetronInfo>
  <Title>X</Title>
  <X-Custom-Vendor>vendor-specific-payload</X-Custom-Vendor>
</MetronInfo>"#;
        let parsed = parse(xml.as_bytes()).expect("parse");
        assert_eq!(
            parsed.raw.get("X-Custom-Vendor").map(String::as_str),
            Some("vendor-specific-payload"),
        );

        let out = serialize(&parsed);
        assert!(
            out.contains("<X-Custom-Vendor>vendor-specific-payload</X-Custom-Vendor>"),
            "raw passthrough dropped: {out}",
        );

        let reparsed = parse(out.as_bytes()).expect("reparse");
        assert_eq!(
            reparsed.raw.get("X-Custom-Vendor").map(String::as_str),
            Some("vendor-specific-payload"),
        );
    }

    #[test]
    fn serialize_omits_empty_fields() {
        let info = MetronInfo {
            title: Some("Only Title".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(xml.contains("<Title>Only Title</Title>"));
        assert!(!xml.contains("<Series"));
        assert!(!xml.contains("<StoryArcs"));
        assert!(!xml.contains("<Credits"));
        assert!(!xml.contains("<ID "));
    }

    #[test]
    fn serialize_escapes_xml_special_chars() {
        let info = MetronInfo {
            title: Some("Tom & Jerry: <ep1>".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(xml.contains("<Title>Tom &amp; Jerry: &lt;ep1&gt;</Title>"), "{xml}");
    }

    #[test]
    fn serialize_id_source_attr_is_escaped() {
        let mut info = MetronInfo::default();
        // Hostile source key (the source registry caps names but we
        // defend regardless — XML attribute escape must run).
        info.ids.insert("evil\"src".into(), "999".into());
        let xml = serialize(&info);
        assert!(xml.contains(r#"<ID source="evil&quot;src">999</ID>"#), "{xml}");
    }

    #[test]
    fn doctype_is_rejected() {
        let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>
<MetronInfo><Title>&xxe;</Title></MetronInfo>"#;
        let err = parse(xxe.as_bytes()).unwrap_err();
        assert!(matches!(err, ParseError::DoctypeRejected));
    }

    #[test]
    fn oversize_rejected() {
        let huge = vec![b'x'; MAX_INPUT_BYTES + 1];
        let err = parse(&huge).unwrap_err();
        assert!(matches!(err, ParseError::TooLarge { .. }));
    }
}
