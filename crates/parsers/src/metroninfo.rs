//! MetronInfo.xml parser (§4.4).
//!
//! Same defensive posture as `comicinfo`: XXE-safe (DOCTYPE rejected), 1 MiB cap.
//!
//! MetronInfo is structurally similar to ComicInfo but with richer creator
//! credits (`<Credit><Creator>Name</Creator><Roles><Role>…` — one element
//! per creator listing every role it holds; the legacy Folio
//! `<Credit role="…"><Creator><Name>` shape is still read) and proper IDs
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

    // See `xml_input::to_utf8` — quick-xml 0.42 rejects the whole
    // document on any non-UTF-8 byte; normalize first so one bad byte
    // can't drop an entire sidecar.
    let text = crate::xml_input::to_utf8(bytes);
    let mut reader = Reader::from_reader(text.as_bytes());
    let cfg = reader.config_mut();
    // See `comicinfo::parse` for the rationale — quick-xml 0.40 emits
    // entity refs as separate events, so `trim_text(true)` would strip
    // the whitespace adjacent to `&amp;` etc. Per-field trim still
    // happens at assignment time on `text`.
    cfg.trim_text(false);
    cfg.expand_empty_elements = true;

    let mut info = MetronInfo::default();
    let mut buf = Vec::with_capacity(2048);
    let mut path: Vec<String> = Vec::with_capacity(16);
    let mut text = String::new();
    let mut credit = CreditState::default();
    let mut current_id_source: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::DocType(_)) => return Err(ParseError::DoctypeRejected),
            Ok(Event::Start(e)) => {
                let name = e.name().into_inner().to_string();
                if name == "Credit" {
                    credit = CreditState::default();
                    // Legacy Folio shape (pre-WP-8.1): `<Credit role="…">`.
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.as_ref() == "role"
                            && let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        {
                            let v = v.trim();
                            if !v.is_empty() {
                                credit.roles.push(role_key_from_metron(v));
                            }
                        }
                    }
                } else if name == "ID" {
                    current_id_source = None;
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.as_ref() == "source" {
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
                let name = e.name().into_inner().to_string();
                if path.last().map(|s| s.as_str()) == Some(name.as_str()) {
                    let value = std::mem::take(&mut text);
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        assign(
                            &mut info,
                            &path,
                            &name,
                            &value,
                            &mut credit,
                            &mut current_id_source,
                        );
                        // Only direct children of <MetronInfo> go into the
                        // passthrough map — mirrors `comicinfo::parse`. A
                        // nested leaf (`<Credit><Creator><Name>`, `<URLs>
                        // <URL>`) has no valid top-level home, so
                        // re-emitting it from `raw` would corrupt the
                        // document; the typed lists carry those.
                        if path.len() == 2 {
                            info.raw.insert(name.clone(), value);
                        }
                    }
                }
                path.pop();
                if name == "Credit" {
                    std::mem::take(&mut credit).flush_into(&mut info.credits);
                }
                if name == "ID" {
                    current_id_source = None;
                }
            }
            Ok(Event::Text(t)) => {
                // quick-xml 0.42 event payloads are already `&str`, so only
                // `escape::unescape()` is left of the old `decode()` chain.
                // Errors fall back to empty (matches `.unwrap_or_default`).
                let s = quick_xml::escape::unescape(&t)
                    .map(|u| u.into_owned())
                    .unwrap_or_default();
                text.push_str(&s);
            }
            Ok(Event::GeneralRef(r)) => {
                // quick-xml 0.40 surfaces `&lt;` / `&gt;` / `&amp;` (and
                // other entity refs) as standalone `GeneralRef` events
                // instead of inlining them in the Text bytes. Without
                // this branch the angle brackets in HTML-bearing
                // `<Summary>`/`<Description>` round-trips disappeared.
                // See `comicinfo.rs` for the full incident note.
                let content: &str = &r;
                if let Some(num) = content.strip_prefix('#') {
                    if let Some(ch) = decode_numeric_char_ref(num) {
                        text.push(ch);
                    }
                } else if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(content)
                {
                    text.push_str(resolved);
                }
            }
            Ok(Event::CData(t)) => {
                text.push_str(&t);
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
///   - `<Credits>` is emitted from the `credits` BTreeMap in the
///     MetronInfo schema shape: one `<Credit>` per creator
///     (`<Creator>name</Creator><Roles><Role>…</Role></Roles>`), creators
///     in first-seen order over the role-sorted map, role values mapped
///     onto the schema enumeration by [`metron_role_value`].
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

    // Credits — last block, in the MetronInfo schema shape (v1.0/v1.1
    // `creditType`): one `<Credit>` per creator, its name as the
    // `<Creator>` text, every role it holds under `<Roles>` (WP-8.1;
    // before, Folio wrote a non-schema `<Credit role="…"><Creator><Name>`
    // per (role, creator) pair — the parser still reads that). Creators
    // in first-seen order over the role-sorted map; roles mapped onto the
    // schema enumeration ([`metron_role_value`]) and deduped per creator.
    let mut by_creator: Vec<(&str, Vec<&'static str>)> = Vec::new();
    for (role, creators) in &info.credits {
        let value = metron_role_value(role);
        for creator in creators {
            if creator.trim().is_empty() {
                continue;
            }
            match by_creator.iter_mut().find(|(c, _)| *c == creator.as_str()) {
                Some((_, roles)) => {
                    if !roles.contains(&value) {
                        roles.push(value);
                    }
                }
                None => by_creator.push((creator.as_str(), vec![value])),
            }
        }
    }
    if !by_creator.is_empty() {
        out.push_str("  <Credits>\n");
        for (creator, roles) in by_creator {
            out.push_str("    <Credit>\n");
            out.push_str("      <Creator>");
            escape_xml_text(&mut out, creator);
            out.push_str("</Creator>\n");
            out.push_str("      <Roles>\n");
            for role in roles {
                out.push_str("        <Role>");
                escape_xml_text(&mut out, role);
                out.push_str("</Role>\n");
            }
            out.push_str("      </Roles>\n");
            out.push_str("    </Credit>\n");
        }
        out.push_str("  </Credits>\n");
    }

    out.push_str("</MetronInfo>\n");
    out
}

/// Mirror of `comicinfo::decode_numeric_char_ref`. Used by the
/// `GeneralRef` branch above to resolve `&#NNN;` / `&#xHEX;` references.
fn decode_numeric_char_ref(num: &str) -> Option<char> {
    let codepoint = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        num.parse::<u32>().ok()?
    };
    char::from_u32(codepoint)
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
            | "Roles"
            | "Role"
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

/// One `<Credit>` being parsed: its creator name(s) and role(s), flushed
/// into [`MetronInfo::credits`] at `</Credit>` (the schema puts
/// `<Roles>` after `<Creator>`, the legacy shape the role up front).
#[derive(Default)]
struct CreditState {
    creators: Vec<String>,
    roles: Vec<String>,
}

impl CreditState {
    fn flush_into(self, credits: &mut BTreeMap<String, Vec<String>>) {
        for role in &self.roles {
            for creator in &self.creators {
                let names = credits.entry(role.clone()).or_default();
                if !names.contains(creator) {
                    names.push(creator.clone());
                }
            }
        }
    }
}

/// The MetronInfo `roleValues` enumeration (schema v1.0, unchanged in
/// v1.1), in schema order.
pub const METRON_ROLES: &[&str] = &[
    "Writer",
    "Script",
    "Story",
    "Plot",
    "Interviewer",
    "Artist",
    "Penciller",
    "Breakdowns",
    "Illustrator",
    "Layouts",
    "Inker",
    "Embellisher",
    "Finishes",
    "Ink Assists",
    "Colorist",
    "Color Separations",
    "Color Assists",
    "Color Flats",
    "Digital Art Technician",
    "Gray Tone",
    "Letterer",
    "Cover",
    "Editor",
    "Consulting Editor",
    "Assistant Editor",
    "Associate Editor",
    "Group Editor",
    "Senior Editor",
    "Managing Editor",
    "Collection Editor",
    "Production",
    "Designer",
    "Logo Design",
    "Translator",
    "Supervising Editor",
    "Executive Editor",
    "Editor In Chief",
    "President",
    "Publisher",
    "Chief Creative Officer",
    "Executive Producer",
    "Other",
];

/// Folio's internal credit key (the ComicInfo names the accessors read:
/// `Writer`, `CoverArtist`, …) for a MetronInfo `<Role>` value. Only
/// `Cover` differs; every other value is kept as written.
fn role_key_from_metron(role: &str) -> String {
    let role = role.trim();
    if role.eq_ignore_ascii_case("cover") {
        "CoverArtist".to_owned()
    } else {
        role.to_owned()
    }
}

/// The MetronInfo `<Role>` value for one of Folio's credit keys: the
/// schema enumeration entry it names (case / `_` / `-` / spacing
/// insensitive; `CoverArtist` → `Cover`), else `Other` — the schema's
/// catch-all, so the document stays valid for roles outside the
/// enumeration (`journalist`, `unknown`).
pub fn metron_role_value(key: &str) -> &'static str {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let k = norm(key);
    if k == "coverartist" || k == "covers" {
        return "Cover";
    }
    METRON_ROLES
        .iter()
        .find(|r| norm(r) == k)
        .copied()
        .unwrap_or("Other")
}

fn assign(
    info: &mut MetronInfo,
    path: &[String],
    name: &str,
    val: &str,
    credit: &mut CreditState,
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

    // Credits — both shapes. Schema (MetronInfo v1.0/v1.1):
    // `<Credit><Creator>Name</Creator><Roles><Role>Writer</Role></Roles>`.
    // Legacy Folio: `<Credit role="Writer"><Creator><Name>…</Name>`.
    match (parent, name) {
        (Some("Credit"), "Creator") | (Some("Creator"), "Name") => {
            credit.creators.push(val.to_string());
            return;
        }
        (Some("Roles"), "Role") => {
            credit.roles.push(role_key_from_metron(val));
            return;
        }
        _ => {}
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

    /// The schema's credit shape (MetronInfo v1.0/v1.1 `creditType`, as
    /// in the upstream `schema/v1.0/Sample.xml`): name as `<Creator>`
    /// text, roles under `<Roles>`, several roles per creator.
    #[test]
    fn parses_schema_credit_shape() {
        let xml = r#"<?xml version="1.0"?>
<MetronInfo>
  <Credits>
    <Credit>
      <Creator id="32165">Geoff Johns</Creator>
      <Roles>
        <Role id="32165">Writer</Role>
        <Role>Cover</Role>
      </Roles>
    </Credit>
    <Credit>
      <Creator>David Finch</Creator>
      <Roles><Role>Cover</Role></Roles>
    </Credit>
    <Credit>
      <Creator>Jane Doe</Creator>
      <Roles><Role>Penciller</Role><Role>Ink Assists</Role></Roles>
    </Credit>
  </Credits>
</MetronInfo>"#;
        let info = parse(xml.as_bytes()).expect("parse");
        assert_eq!(info.writer().as_deref(), Some("Geoff Johns"));
        // `Cover` is the schema spelling of Folio's `CoverArtist`.
        assert_eq!(
            info.cover_artist().as_deref(),
            Some("Geoff Johns, David Finch")
        );
        assert_eq!(info.penciller().as_deref(), Some("Jane Doe"));
        assert_eq!(
            info.credits.get("Ink Assists").map(Vec::as_slice),
            Some(["Jane Doe".to_string()].as_slice())
        );
        assert!(info.raw.keys().all(|k| k != "Creator" && k != "Role"));
    }

    /// WP-8.1: the serializer emits the schema shape — one `<Credit>` per
    /// creator, `<Creator>` text + `<Roles><Role>`, role values from the
    /// schema enumeration — and no `role=` attribute / `<Name>` child.
    #[test]
    fn serialize_writes_schema_credit_shape() {
        let mut info = MetronInfo::default();
        info.credits
            .insert("Writer".into(), vec!["Ann".into(), "Bob".into()]);
        info.credits
            .insert("CoverArtist".into(), vec!["Ann".into()]);
        info.credits.insert("journalist".into(), vec!["Cy".into()]);
        info.credits
            .insert("ink assists".into(), vec!["Bob".into()]);
        let xml = serialize(&info);
        assert!(!xml.contains("role="), "{xml}");
        assert!(!xml.contains("<Name>"), "{xml}");
        // Ann: Cover (from CoverArtist, sorted first) + Writer, in one Credit.
        assert!(
            xml.contains(
                "    <Credit>\n      <Creator>Ann</Creator>\n      <Roles>\n        \
                 <Role>Cover</Role>\n        <Role>Writer</Role>\n      </Roles>\n    </Credit>\n"
            ),
            "{xml}"
        );
        assert!(xml.contains("<Role>Ink Assists</Role>"), "{xml}");
        // Outside the enumeration → the schema's catch-all.
        assert!(
            xml.contains("<Creator>Cy</Creator>\n      <Roles>\n        <Role>Other</Role>"),
            "{xml}"
        );
        assert_eq!(xml.matches("<Credit>").count(), 3, "{xml}");
        let back = parse(xml.as_bytes()).unwrap();
        assert_eq!(back.writer().as_deref(), Some("Ann, Bob"));
        assert_eq!(back.cover_artist().as_deref(), Some("Ann"));
    }

    #[test]
    fn metron_role_value_maps_onto_the_schema_enumeration() {
        for (key, want) in [
            ("Writer", "Writer"),
            ("writer", "Writer"),
            ("CoverArtist", "Cover"),
            ("cover_artist", "Cover"),
            ("Cover", "Cover"),
            ("Penciller", "Penciller"),
            ("ink assists", "Ink Assists"),
            ("editor in chief", "Editor In Chief"),
            ("Translator", "Translator"),
            ("journalist", "Other"),
            ("unknown", "Other"),
        ] {
            assert_eq!(metron_role_value(key), want, "{key}");
        }
    }

    #[test]
    fn serialize_round_trip_preserves_external_ids() {
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        // Each `<ID source="…">value</ID>` round-trips.
        assert!(xml.contains(r#"<ID source="metron">12345</ID>"#), "{xml}");
        assert!(
            xml.contains(r#"<ID source="comicvine">67890</ID>"#),
            "{xml}"
        );

        let reparsed = parse(xml.as_bytes()).expect("reparse");
        assert_eq!(
            reparsed.ids.get("metron").map(String::as_str),
            Some("12345")
        );
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

    /// WP-2.6 (c): `raw` holds top-level elements only. A nested leaf
    /// (`<Credits><Credit><Creator><Name>`, `<URLs><URL>`) never lands in
    /// the passthrough map, so serializing can't hoist it to the root and
    /// corrupt the document; the typed lists / credits carry those.
    #[test]
    fn raw_map_holds_top_level_elements_only() {
        let xml = r#"<?xml version="1.0"?>
<MetronInfo>
  <Title>X</Title>
  <MangaVolume>3</MangaVolume>
  <URLs><URL primary="true">https://example.com/1</URL></URLs>
  <Credits>
    <Credit role="Writer"><Creator><Name>Someone</Name></Creator></Credit>
  </Credits>
</MetronInfo>"#;
        let parsed = parse(xml.as_bytes()).expect("parse");
        assert_eq!(parsed.raw.get("MangaVolume").map(String::as_str), Some("3"));
        assert_eq!(parsed.raw.get("Title").map(String::as_str), Some("X"));
        assert!(!parsed.raw.contains_key("URL"), "{:?}", parsed.raw);
        assert!(!parsed.raw.contains_key("Name"), "{:?}", parsed.raw);
        assert_eq!(parsed.writer().as_deref(), Some("Someone"));

        let out = serialize(&parsed);
        assert!(out.contains("<MangaVolume>3</MangaVolume>"), "{out}");
        assert!(!out.contains("<URL>"), "nested leaf hoisted to root: {out}");
        assert!(
            !out.contains("  <Name>"),
            "nested leaf hoisted to root: {out}"
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
        assert!(
            xml.contains("<Title>Tom &amp; Jerry: &lt;ep1&gt;</Title>"),
            "{xml}"
        );
    }

    #[test]
    fn serialize_id_source_attr_is_escaped() {
        let mut info = MetronInfo::default();
        // Hostile source key (the source registry caps names but we
        // defend regardless — XML attribute escape must run).
        info.ids.insert("evil\"src".into(), "999".into());
        let xml = serialize(&info);
        assert!(
            xml.contains(r#"<ID source="evil&quot;src">999</ID>"#),
            "{xml}"
        );
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
