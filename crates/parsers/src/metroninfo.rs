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
                            current_creator_role =
                                attr.unescape_value().ok().map(|c| c.into_owned());
                        }
                    }
                } else if name == "ID" {
                    current_id_source = None;
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.as_ref() == b"source" {
                            current_id_source = attr.unescape_value().ok().map(|c| c.into_owned());
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
                text.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
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
