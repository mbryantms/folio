//! MetronInfo.xml parser + serializer (§4.4).
//!
//! Same defensive posture as `comicinfo`: XXE-safe (DOCTYPE rejected), 1 MiB cap.
//!
//! The serializer writes the **MetronInfo v1.2 schema**
//! ([Metron-Project/metroninfo](https://github.com/Metron-Project/metroninfo),
//! `schema/v1.2/MetronInfo.xsd`, vendored at
//! `fixtures/metroninfo/MetronInfo-v1.2.xsd`): `<IDS><ID source primary>`,
//! nested `<Publisher><Name/><Imprint/>` and `<Series lang><Name/>…`,
//! `<Stories><Story>` for the issue title, `<CoverDate>` / `<StoreDate>`
//! as `xs:date`, `<Arcs><Arc><Name/><Number/>`, `<GTIN><ISBN/><UPC/>`,
//! the `ageRatingType` / `formatType` / `informationSource` enumerations,
//! `<URLs>`, `<Credits>` (WP-8.1) and `<LastModified>`. Every other tool
//! that reads MetronInfo (Codex, Metron-Tagger, ComicTagger 1.6) reads
//! this shape; writeback puts the file into every archive.
//!
//! The parser reads **three shapes onto the same struct**: the schema
//! shape above; the pre-schema shape Folio wrote until audit 2026-11
//! WP A4 (`<Title>`, `<Year>/<Month>/<Day>`, flat `<Series>` /
//! `<Publisher>` / `<Imprint>` / `<Language>` / `<Manga>` text, a bare
//! `<GTIN>`, top-level `<ID source="metron">` without `<IDS>`,
//! `<StoryArcs><StoryArc>`, `<Credit role="…"><Creator><Name>`); and the
//! plain top-level `<ID>` some other tools wrote. Every production archive
//! carries the old shape until it is rewritten, so a rescan must lose
//! nothing. Top-level elements Folio has no slot for land in `raw` and are
//! re-emitted verbatim (`<MangaVolume>`, vendor `<X-…>`).
//!
//! When both ComicInfo and MetronInfo are present in the same archive, the
//! scanner merges them (`merge_metron_into_comicinfo`): MetronInfo wins
//! where both populate the same field, then series.json and filename
//! inference fill what is left (§4.3 / §6.8). MetronInfo's role-tagged
//! creators are flattened into the `Writer/Penciller/...` strings expected
//! by downstream code.

use crate::ParseError;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// The schema version the serializer targets; the `xsi:noNamespaceSchemaLocation`
/// hint on the root points at this file in the upstream repository.
pub const SCHEMA_VERSION: &str = "1.2";
pub const SCHEMA_LOCATION: &str =
    "https://raw.githubusercontent.com/Metron-Project/metroninfo/master/schema/v1.2/MetronInfo.xsd";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetronInfo {
    /// The issue's story title. Written as the single `<Stories><Story>`;
    /// several `<Story>` elements are joined with `; ` on read.
    pub title: Option<String>,
    /// `<Series><Name>` (or the pre-schema flat `<Series>` text).
    pub series: Option<String>,
    /// `<Publisher><Name>` (or the pre-schema flat `<Publisher>` text).
    pub publisher: Option<String>,
    /// `<Publisher><Imprint>` (or the pre-schema top-level `<Imprint>`).
    pub imprint: Option<String>,
    pub number: Option<String>,
    /// `<Series><Volume>` — a volume *number*, never a start year.
    pub volume: Option<i32>,
    /// Cover date, split as the ComicInfo side stores it. Written as
    /// `<CoverDate>YYYY-MM-DD</CoverDate>` with a missing month / day
    /// defaulting to `01` (the Metron convention: cover dates are
    /// month-granular); read from `<CoverDate>` or the pre-schema
    /// `<Year>/<Month>/<Day>`.
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub summary: Option<String>,
    pub notes: Option<String>,
    /// Stored verbatim. Written only when [`metron_age_rating`] maps it
    /// onto the schema's `ageRatingType` enumeration.
    pub age_rating: Option<String>,
    /// BCP 47 tag; written as the `<Series lang>` attribute, read from it
    /// or from the pre-schema top-level `<Language>`.
    pub language: Option<String>,
    /// `Yes` / `YesAndRightToLeft` / `No` — pre-schema only. The schema has
    /// no slot for it (`<MangaVolume>` means something else), so it is
    /// read but never written; ComicInfo keeps it.
    pub manga: Option<String>,
    /// Pre-schema bare `<GTIN>text</GTIN>` only. The schema's
    /// `<GTIN><ISBN/><UPC/></GTIN>` lands in `ids` under `isbn` / `upc`.
    pub gtin: Option<String>,
    /// Story-arc names in document order (`<Arcs><Arc><Name>` or the
    /// pre-schema `<StoryArcs><StoryArc>`); numbers in [`Self::arc_numbers`].
    pub story_arcs: Vec<String>,
    pub characters: Vec<String>,
    pub teams: Vec<String>,
    pub locations: Vec<String>,
    pub tags: Vec<String>,
    pub genres: Vec<String>,
    /// External IDs by Folio source key: `{"metron": "123", "comicvine": "456"}`.
    /// Schema `source` names are mapped on read ([`folio_id_source`]) and on
    /// write ([`metron_id_source`]); a source outside the schema's
    /// `informationSource` enumeration is kept here but not written.
    pub ids: BTreeMap<String, String>,
    /// Creators grouped by role. Multiple credits with the same role are joined with `, `.
    pub credits: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, String>,

    // ───── v1.2 schema fields (audit 2026-11 WP A4) ─────
    /// Folio source key of the `<ID primary="true">` entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_id_source: Option<String>,
    /// `<Series id>` — the primary source's id for the series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_sort_name: Option<String>,
    /// `<Series><Format>`, a `formatType` value (see [`metron_series_format`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_start_year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_end_year: Option<i32>,
    /// `<Series><Status>`, a `seriesStatusType` value (see [`metron_series_status`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_issue_count: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_summary: Option<String>,
    /// `<StoreDate>` as `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<i32>,
    /// `<Arc><Number>` by arc name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arc_numbers: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub universes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reprints: Vec<String>,
    /// `<URLs><URL primary="true">`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_url: Option<String>,
    /// The other `<URL>`s, in document order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// `<LastModified>` as written (`xs:dateTime`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
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

    /// The issue's web link: the primary `<URL>`, else the first one.
    pub fn web_url(&self) -> Option<&str> {
        self.primary_url
            .as_deref()
            .or_else(|| self.urls.first().map(String::as_str))
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
    let mut st = ParseState::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::DocType(_)) => return Err(ParseError::DoctypeRejected),
            Ok(Event::Start(e)) => {
                let name = e.name().into_inner().to_string();
                let attr = |key: &str| -> Option<String> {
                    e.attributes()
                        .with_checks(false)
                        .flatten()
                        .find(|a| a.key.as_ref() == key)
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()
                                .map(|c| c.into_owned())
                        })
                };
                match name.as_str() {
                    "Credit" => {
                        st.credit = CreditState::default();
                        // Legacy Folio shape (pre-WP-8.1): `<Credit role="…">`.
                        if let Some(v) = attr("role") {
                            let v = v.trim();
                            if !v.is_empty() {
                                st.credit.roles.push(role_key_from_metron(v));
                            }
                        }
                    }
                    "ID" => {
                        st.id_source = attr("source");
                        st.id_primary = attr("primary").is_some_and(|v| is_xml_true(&v));
                    }
                    "URL" => st.url_primary = attr("primary").is_some_and(|v| is_xml_true(&v)),
                    "Series" if path.len() == 1 => {
                        if let Some(lang) = attr("lang").map(|l| l.trim().to_owned())
                            && !lang.is_empty()
                        {
                            info.language = Some(lang);
                        }
                        if let Some(id) = attr("id").map(|l| l.trim().to_owned())
                            && !id.is_empty()
                        {
                            info.series_id = Some(id);
                        }
                    }
                    "Arc" => st.arc = ArcState::default(),
                    _ => {}
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
                        assign(&mut info, &path, &name, &value, &mut st);
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
                match name.as_str() {
                    "Credit" => std::mem::take(&mut st.credit).flush_into(&mut info.credits),
                    "ID" => {
                        st.id_source = None;
                        st.id_primary = false;
                    }
                    "URL" => st.url_primary = false,
                    "Arc" => {
                        let arc = std::mem::take(&mut st.arc);
                        if let Some(n) = arc.name {
                            if let Some(num) = arc.number {
                                info.arc_numbers.insert(n.clone(), num);
                            }
                            info.story_arcs.push(n);
                        }
                    }
                    _ => {}
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

fn is_xml_true(v: &str) -> bool {
    let v = v.trim();
    v.eq_ignore_ascii_case("true") || v == "1"
}

/// Per-element parse state: the `<Credit>` / `<Arc>` being assembled and
/// the attributes of the `<ID>` / `<URL>` whose text is pending.
#[derive(Default)]
struct ParseState {
    credit: CreditState,
    arc: ArcState,
    id_source: Option<String>,
    id_primary: bool,
    url_primary: bool,
}

#[derive(Default)]
struct ArcState {
    name: Option<String>,
    number: Option<u32>,
}

/// Emit a MetronInfo.xml document from `info` in the v1.2 schema shape.
/// UTF-8, 2-space indent.
///
/// Rules:
///
///   - Elements follow the XSD's listing order (`metroninfoType` is an
///     `xs:all`, so order is not validated, but a stable order keeps the
///     parse → serialize round-trip a clean diff) and are omitted when
///     empty / `None`.
///   - `<IDS>` carries one `<ID source="…">` per id whose source maps
///     onto the schema's `informationSource` enumeration
///     ([`metron_id_source`]), sorted by Folio key; the
///     [`MetronInfo::primary_id_source`] entry gets `primary="true"`.
///     Barcodes (`isbn` / `upc` / the pre-schema `gtin`) go to
///     `<GTIN><ISBN/><UPC/>` instead; `asin` / `doi` / unknown sources
///     have no slot and are not written.
///   - `<Publisher>` is written only with a name (the schema requires
///     `<Name>`); `<Series>` likewise, carrying `lang` / `id` attributes
///     and the volume / format / years / status / count / summary.
///   - Enumerated values (`<AgeRating>`, `<Series><Format>` / `<Status>`)
///     are written only when they map onto the schema's enumeration.
///   - `<CoverDate>` needs a year; a missing month / day is written as
///     `01`. `<StoreDate>` / `<LastModified>` are written as stored when
///     they look like a date / date-time.
///   - `<Credits>` is emitted from the `credits` BTreeMap in the schema
///     shape: one `<Credit>` per creator
///     (`<Creator>name</Creator><Roles><Role>…</Role></Roles>`), creators
///     in first-seen order over the role-sorted map, role values mapped
///     onto the schema enumeration by [`metron_role_value`].
///   - Unknown top-level leafs in [`MetronInfo::raw`] are passed through
///     after `<LastModified>`. Entries matching a typed element name
///     (either shape) are not re-emitted — the typed value wins, and a
///     pre-schema `<Title>` / `<Year>` must not reappear at the root.
///   - Text values are XML-escaped via [`escape_xml_text`].
pub fn serialize(info: &MetronInfo) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(
        "<MetronInfo xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:noNamespaceSchemaLocation=\"",
    );
    out.push_str(SCHEMA_LOCATION);
    out.push_str("\">\n");

    write_ids(&mut out, info);
    write_publisher(&mut out, info);
    write_series(&mut out, info);
    write_opt_str(&mut out, "Number", &info.number);
    if let Some(title) = info.title.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str("  <Stories>\n    <Story>");
        escape_xml_text(&mut out, title);
        out.push_str("</Story>\n  </Stories>\n");
    }
    write_opt_str(&mut out, "Summary", &info.summary);
    if let Some(date) = cover_date_string(info) {
        write_text(&mut out, "CoverDate", &date);
    }
    if let Some(d) = info.store_date.as_deref().filter(|d| looks_like_date(d)) {
        write_text(&mut out, "StoreDate", d.trim());
    }
    if let Some(n) = info.page_count.filter(|n| *n >= 0) {
        write_text(&mut out, "PageCount", &n.to_string());
    }
    write_opt_str(&mut out, "Notes", &info.notes);
    write_list(&mut out, "Genres", "Genre", &info.genres);
    write_list(&mut out, "Tags", "Tag", &info.tags);
    write_arcs(&mut out, info);
    write_list(&mut out, "Characters", "Character", &info.characters);
    write_list(&mut out, "Teams", "Team", &info.teams);
    write_universes(&mut out, &info.universes);
    write_list(&mut out, "Locations", "Location", &info.locations);
    write_list(&mut out, "Reprints", "Reprint", &info.reprints);
    write_gtin(&mut out, info);
    if let Some(r) = info.age_rating.as_deref().and_then(metron_age_rating) {
        write_text(&mut out, "AgeRating", r);
    }
    write_urls(&mut out, info);
    write_credits(&mut out, info);
    if let Some(ts) = info.last_modified.as_deref().filter(|t| looks_like_date(t)) {
        write_text(&mut out, "LastModified", ts.trim());
    }

    // Raw passthrough for unknown top-level elements (vendor `<X-…>`,
    // `<MangaVolume>`, …). Filters typed names so a pre-schema copy of a
    // typed value never reappears at the root.
    for (k, v) in &info.raw {
        if is_typed_metron_info_leaf(k) {
            continue;
        }
        write_text(&mut out, k, v);
    }

    out.push_str("</MetronInfo>\n");
    out
}

fn write_ids(out: &mut String, info: &MetronInfo) {
    let entries: Vec<(&str, &'static str, &str)> = info
        .ids
        .iter()
        .filter_map(|(k, v)| {
            let v = v.trim();
            if v.is_empty() {
                return None;
            }
            metron_id_source(k).map(|schema| (k.as_str(), schema, v))
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    out.push_str("  <IDS>\n");
    for (key, schema, value) in entries {
        out.push_str("    <ID source=\"");
        out.push_str(schema);
        out.push('"');
        if info.primary_id_source.as_deref() == Some(key) {
            out.push_str(" primary=\"true\"");
        }
        out.push('>');
        escape_xml_text(out, value);
        out.push_str("</ID>\n");
    }
    out.push_str("  </IDS>\n");
}

fn write_publisher(out: &mut String, info: &MetronInfo) {
    let Some(name) = info.publisher.as_deref().filter(|s| !s.trim().is_empty()) else {
        return;
    };
    out.push_str("  <Publisher>\n");
    write_nested_text(out, 4, "Name", name.trim());
    if let Some(imprint) = info.imprint.as_deref().filter(|s| !s.trim().is_empty()) {
        write_nested_text(out, 4, "Imprint", imprint.trim());
    }
    out.push_str("  </Publisher>\n");
}

fn write_series(out: &mut String, info: &MetronInfo) {
    let Some(name) = info.series.as_deref().filter(|s| !s.trim().is_empty()) else {
        return;
    };
    out.push_str("  <Series");
    if let Some(id) = info.series_id.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str(" id=\"");
        escape_xml_attr(out, id.trim());
        out.push('"');
    }
    if let Some(lang) = info.language.as_deref().filter(|l| is_language_code(l)) {
        out.push_str(" lang=\"");
        escape_xml_attr(out, lang.trim());
        out.push('"');
    }
    out.push_str(">\n");
    write_nested_text(out, 4, "Name", name.trim());
    if let Some(s) = info
        .series_sort_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        write_nested_text(out, 4, "SortName", s.trim());
    }
    if let Some(v) = info.volume.filter(|v| *v >= 0) {
        write_nested_text(out, 4, "Volume", &v.to_string());
    }
    if let Some(f) = info.series_format.as_deref().and_then(metron_series_format) {
        write_nested_text(out, 4, "Format", f);
    }
    let start = info.series_start_year.filter(|y| *y > 0);
    let end = info
        .series_end_year
        .filter(|y| *y > 0 && start.is_none_or(|s| *y >= s));
    if let Some(y) = start {
        write_nested_text(out, 4, "StartYear", &format!("{y:04}"));
    }
    if let Some(y) = end {
        write_nested_text(out, 4, "EndYear", &format!("{y:04}"));
    }
    // The schema asserts an ongoing series has no EndYear; the end year is
    // the data, so the status gives way.
    if let Some(s) = info
        .series_status
        .as_deref()
        .and_then(metron_series_status)
        .filter(|s| !(*s == "Ongoing" && end.is_some()))
    {
        write_nested_text(out, 4, "Status", s);
    }
    if let Some(n) = info.series_issue_count.filter(|n| *n > 0) {
        write_nested_text(out, 4, "IssueCount", &n.to_string());
    }
    if let Some(s) = info
        .series_summary
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        write_nested_text(out, 4, "Summary", s.trim());
    }
    out.push_str("  </Series>\n");
}

fn write_arcs(out: &mut String, info: &MetronInfo) {
    let arcs: Vec<&str> = info
        .story_arcs
        .iter()
        .map(String::as_str)
        .filter(|a| !a.trim().is_empty())
        .collect();
    if arcs.is_empty() {
        return;
    }
    out.push_str("  <Arcs>\n");
    for arc in arcs {
        out.push_str("    <Arc>\n");
        write_nested_text(out, 6, "Name", arc.trim());
        if let Some(n) = info.arc_numbers.get(arc).filter(|n| **n > 0) {
            write_nested_text(out, 6, "Number", &n.to_string());
        }
        out.push_str("    </Arc>\n");
    }
    out.push_str("  </Arcs>\n");
}

fn write_universes(out: &mut String, universes: &[String]) {
    let names: Vec<&str> = universes
        .iter()
        .map(String::as_str)
        .filter(|u| !u.trim().is_empty())
        .collect();
    if names.is_empty() {
        return;
    }
    out.push_str("  <Universes>\n");
    for u in names {
        out.push_str("    <Universe>\n");
        write_nested_text(out, 6, "Name", u.trim());
        out.push_str("    </Universe>\n");
    }
    out.push_str("  </Universes>\n");
}

/// `<GTIN><ISBN/><UPC/></GTIN>` from the `isbn` / `upc` ids, falling back
/// to the pre-schema `gtin` (field or id source) classified by shape
/// ([`is_isbn_shaped`]). Values that are not a barcode are not written.
fn write_gtin(out: &mut String, info: &MetronInfo) {
    fn barcode(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| is_barcode_shaped(v))
    }
    let generic: Vec<&str> = [
        info.gtin.as_deref(),
        info.ids.get("gtin").map(String::as_str),
    ]
    .into_iter()
    .filter_map(barcode)
    .collect();
    let isbn = barcode(info.ids.get("isbn").map(String::as_str))
        .or_else(|| generic.iter().copied().find(|v| is_isbn_shaped(v)));
    let upc = barcode(info.ids.get("upc").map(String::as_str))
        .or_else(|| generic.iter().copied().find(|v| !is_isbn_shaped(v)));
    if isbn.is_none() && upc.is_none() {
        return;
    }
    out.push_str("  <GTIN>\n");
    if let Some(v) = isbn {
        write_nested_text(out, 4, "ISBN", v);
    }
    if let Some(v) = upc {
        write_nested_text(out, 4, "UPC", v);
    }
    out.push_str("  </GTIN>\n");
}

fn write_urls(out: &mut String, info: &MetronInfo) {
    let primary = info
        .primary_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    let others: Vec<&str> = info
        .urls
        .iter()
        .map(|u| u.trim())
        .filter(|u| !u.is_empty() && Some(*u) != primary)
        .collect();
    if primary.is_none() && others.is_empty() {
        return;
    }
    out.push_str("  <URLs>\n");
    if let Some(u) = primary {
        out.push_str("    <URL primary=\"true\">");
        escape_xml_text(out, u);
        out.push_str("</URL>\n");
    }
    let mut seen: Vec<&str> = Vec::new();
    for u in others {
        if seen.contains(&u) {
            continue;
        }
        seen.push(u);
        write_nested_text(out, 4, "URL", u);
    }
    out.push_str("  </URLs>\n");
}

fn write_credits(out: &mut String, info: &MetronInfo) {
    // Credits in the MetronInfo schema shape (v1.0+ `creditType`): one
    // `<Credit>` per creator, its name as the `<Creator>` text, every role
    // it holds under `<Roles>` (WP-8.1; before, Folio wrote a non-schema
    // `<Credit role="…"><Creator><Name>` per (role, creator) pair — the
    // parser still reads that). Creators in first-seen order over the
    // role-sorted map; roles mapped onto the schema enumeration
    // ([`metron_role_value`]) and deduped per creator.
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
    if by_creator.is_empty() {
        return;
    }
    out.push_str("  <Credits>\n");
    for (creator, roles) in by_creator {
        out.push_str("    <Credit>\n");
        out.push_str("      <Creator>");
        escape_xml_text(out, creator);
        out.push_str("</Creator>\n");
        out.push_str("      <Roles>\n");
        for role in roles {
            out.push_str("        <Role>");
            escape_xml_text(out, role);
            out.push_str("</Role>\n");
        }
        out.push_str("      </Roles>\n");
        out.push_str("    </Credit>\n");
    }
    out.push_str("  </Credits>\n");
}

/// `YYYY-MM-DD` for `<CoverDate>`: the year is required; a missing month
/// or day is `01` (cover dates are month-granular at Metron and ComicVine,
/// which date them on the 1st). An out-of-range month yields `None`; a
/// day the month does not have (`2021-02-31`) falls back to `01`, since
/// `xs:date` rejects it and the month is still true.
fn cover_date_string(info: &MetronInfo) -> Option<String> {
    let y = info.year.filter(|y| (1..=9999).contains(y))?;
    let m = info.month.unwrap_or(1);
    if !(1..=12).contains(&m) {
        return None;
    }
    let d = info
        .day
        .filter(|d| (1..=days_in_month(y, m)).contains(d))
        .unwrap_or(1);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        _ => 28,
    }
}

/// Split an `xs:date` / `xs:dateTime` / `xs:gYear` lexical value into
/// `(year, month, day)`; `YYYY-MM` and `YYYY` are accepted as partial
/// dates. A trailing time or zone (`T09:00:46`, `Z`, `-04:00`) is ignored.
pub fn parse_xml_date(s: &str) -> Option<(i32, Option<i32>, Option<i32>)> {
    let s = s.trim();
    let date = s.split('T').next().unwrap_or(s);
    // Strip a zone suffix on a bare date (`2021-07-14Z`, `2021-07-14+02:00`).
    let date = date.find(['Z', '+']).map_or(date, |i| &date[..i]);
    let date = match date.rsplit_once('-') {
        // `2021-07-14-04:00` — a zone with a leading minus after a full date.
        Some((head, tail)) if tail.contains(':') => head,
        _ => date,
    };
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    if year <= 0 {
        return None;
    }
    let month = match parts.next() {
        Some(m) => Some(m.parse::<i32>().ok().filter(|m| (1..=12).contains(m))?),
        None => None,
    };
    let day = match parts.next() {
        Some(d) => Some(d.parse::<i32>().ok().filter(|d| (1..=31).contains(d))?),
        None => None,
    };
    Some((year, month, day))
}

fn looks_like_date(s: &str) -> bool {
    parse_xml_date(s).is_some()
}

/// BCP 47 shape the schema's `languageCode` pattern accepts:
/// `[a-zA-Z]{2,3}(-[a-zA-Z0-9]{1,8})*`.
pub fn is_language_code(s: &str) -> bool {
    let s = s.trim();
    let mut parts = s.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&primary.len()) || !primary.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn barcode_digits(v: &str) -> String {
    v.chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Digits (hyphens / spaces allowed; an ISBN-10 check digit may be `X`),
/// 8 to 18 characters — EAN-8 through a UPC with its 5-digit add-on.
fn is_barcode_shaped(v: &str) -> bool {
    let d = barcode_digits(v);
    (8..=18).contains(&d.len())
        && d.chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_digit() || (c == 'X' && i == d.len() - 1))
}

/// ISBN-13 (`978` / `979` prefix) or ISBN-10.
pub fn is_isbn_shaped(v: &str) -> bool {
    let d = barcode_digits(v);
    (d.len() == 13 && (d.starts_with("978") || d.starts_with("979"))) || d.len() == 10
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

/// Top-level element names with a typed slot in either shape (schema v1.2
/// or the pre-schema Folio document). Never re-emitted from `raw`: the
/// typed field carries the value, and a pre-schema `<Title>` / `<Year>` at
/// the root would make the document invalid. Only top-level leaves ever
/// reach `raw` (the depth check in [`parse`]), so nested names are not
/// listed.
fn is_typed_metron_info_leaf(name: &str) -> bool {
    matches!(
        name,
        // Pre-schema Folio scalars.
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
            | "ID"
            | "StoryArcs"
            // Schema v1.2 containers and scalars.
            | "IDS"
            | "Stories"
            | "CoverDate"
            | "StoreDate"
            | "PageCount"
            | "Genres"
            | "Tags"
            | "Arcs"
            | "Characters"
            | "Teams"
            | "Universes"
            | "Locations"
            | "Reprints"
            | "URLs"
            | "Credits"
            | "LastModified"
    )
}

fn write_list(out: &mut String, container: &str, leaf: &str, items: &[String]) {
    let items: Vec<&str> = items
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    if items.is_empty() {
        return;
    }
    out.push_str("  <");
    out.push_str(container);
    out.push_str(">\n");
    for it in items {
        write_nested_text(out, 4, leaf, it.trim());
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

fn write_text(out: &mut String, name: &str, value: &str) {
    write_nested_text(out, 2, name, value);
}

fn write_nested_text(out: &mut String, indent: usize, name: &str, value: &str) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
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

// ───────── schema enumerations ─────────

/// The MetronInfo `roleValues` enumeration (schema v1.0, unchanged
/// through v1.2), in schema order.
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

/// The `informationSource` enumeration (v1.2) as `(Folio source key,
/// schema value)`. Folio keys are [`Source::as_str`] spellings
/// (`crates/server/src/metadata/identifier.rs`); `kitsu` / `mangadex`
/// have no Folio source yet and only round-trip through `ids`.
pub const METRON_ID_SOURCES: &[(&str, &str)] = &[
    ("anilist", "AniList"),
    ("comicvine", "Comic Vine"),
    ("gcd", "Grand Comics Database"),
    ("kitsu", "Kitsu"),
    ("mangadex", "MangaDex"),
    ("mangaupdates", "MangaUpdates"),
    ("marvel", "Marvel"),
    ("metron", "Metron"),
    ("mal", "MyAnimeList"),
    ("locg", "League of Comic Geeks"),
];

/// The `ageRatingType` enumeration (v1.0+).
pub const METRON_AGE_RATINGS: &[&str] = &[
    "Unknown",
    "Everyone",
    "Teen",
    "Teen Plus",
    "Mature",
    "Explicit",
    "Adult",
];

/// The `formatType` enumeration (v1.0+).
pub const METRON_FORMATS: &[&str] = &[
    "Annual",
    "Digital Chapter",
    "Graphic Novel",
    "Hardcover",
    "Limited Series",
    "Omnibus",
    "One-Shot",
    "Single Issue",
    "Trade Paperback",
];

/// The `seriesStatusType` enumeration (v1.2).
pub const METRON_SERIES_STATUSES: &[&str] = &["Cancelled", "Completed", "Hiatus", "Ongoing"];

/// Lower-case, `_` / `-` / runs of whitespace collapsed to one space.
fn norm_words(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if matches!(c, '_' | '-') {
                ' '
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The schema `informationSource` value for a Folio source key, or `None`
/// for a source the enumeration lacks (`isbn` / `upc` / `gtin` go to
/// `<GTIN>`; `asin` / `doi` have no slot).
pub fn metron_id_source(folio_key: &str) -> Option<&'static str> {
    let k = norm_words(folio_key).replace(' ', "");
    METRON_ID_SOURCES
        .iter()
        .find(|(key, _)| *key == k)
        .map(|(_, schema)| *schema)
}

/// The Folio source key for an `<ID source="…">` value: the schema name
/// (`Comic Vine`, case-insensitive), a Folio key (`comicvine`) or a
/// known alias (`cv`); anything else is kept lower-cased so the scanner's
/// `Source::from_str` decides (and skips an unknown source).
pub fn folio_id_source(schema_name: &str) -> String {
    let n = norm_words(schema_name);
    let compact = n.replace(' ', "");
    if let Some((key, _)) = METRON_ID_SOURCES
        .iter()
        .find(|(key, schema)| norm_words(schema) == n || *key == compact)
    {
        return (*key).to_owned();
    }
    match compact.as_str() {
        "cv" => "comicvine".to_owned(),
        "grandcomicsdatabase" => "gcd".to_owned(),
        "myanimelist" => "mal".to_owned(),
        "mu" => "mangaupdates".to_owned(),
        "leagueofcomicgeeks" => "locg".to_owned(),
        _ => compact,
    }
}

/// The schema `ageRatingType` value for a stored rating: a schema value
/// passes through (case-insensitive); ComicInfo's `AgeRating` vocabulary
/// maps as Metron-Tagger (darkseid) maps it; anything else is `None`
/// (the element is omitted — the schema default is `Unknown`).
pub fn metron_age_rating(value: &str) -> Option<&'static str> {
    let v = norm_words(value);
    if let Some(r) = METRON_AGE_RATINGS.iter().find(|r| norm_words(r) == v) {
        return Some(r);
    }
    Some(match v.as_str() {
        "rating pending" => "Unknown",
        "everyone 10+" | "g" | "kids to adults" | "early childhood" => "Everyone",
        "pg" => "Teen",
        "ma15+" => "Teen Plus",
        "adults only 18+" | "mature 17+" | "r18+" | "m" => "Mature",
        "x18+" => "Explicit",
        _ => return None,
    })
}

/// The schema `formatType` value for a series type / format label: a
/// schema value passes through; Metron's series-type names (`Ongoing
/// Series`, `Annual Series`, `Hard Cover`, …) and ComicInfo `<Format>`
/// spellings (`TPB`, `One Shot`, `Giant-Size`, …) map as Metron-Tagger
/// maps them. `None` for an unknown label.
pub fn metron_series_format(value: &str) -> Option<&'static str> {
    let v = norm_words(value);
    if let Some(f) = METRON_FORMATS.iter().find(|f| norm_words(f) == v) {
        return Some(f);
    }
    Some(match v.as_str() {
        "annual series" => "Annual",
        "digital" | "digital chapters" => "Digital Chapter",
        "hard cover" => "Hardcover",
        "limited" | "mini series" | "miniseries" | "maxi series" | "maxiseries" => "Limited Series",
        "1 shot" | "one shot" | "fcbd" | "preview" => "One-Shot",
        "ongoing" | "ongoing series" | "cancelled series" | "series" | "magazine" | "giant"
        | "giant size" => "Single Issue",
        "tpb" | "trade paper back" => "Trade Paperback",
        _ => return None,
    })
}

/// The schema `seriesStatusType` value for Folio's `series.status`
/// (`continuing` / `ended` / `cancelled` / `hiatus`) or a schema value.
pub fn metron_series_status(value: &str) -> Option<&'static str> {
    let v = norm_words(value);
    if let Some(s) = METRON_SERIES_STATUSES.iter().find(|s| norm_words(s) == v) {
        return Some(s);
    }
    Some(match v.as_str() {
        "continuing" => "Ongoing",
        "ended" | "complete" | "finished" => "Completed",
        "canceled" => "Cancelled",
        _ => return None,
    })
}

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

fn assign(info: &mut MetronInfo, path: &[String], name: &str, val: &str, st: &mut ParseState) {
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

    let parent = path.iter().rev().nth(1).map(String::as_str);
    let top_level = path.len() == 2;

    // List-style elements, both shapes: `<Arcs><Arc><Name>` is assembled
    // in `st.arc` (the number may precede the name under `xs:all`).
    let leaf_into_list = match (parent, name) {
        (Some("StoryArcs"), "StoryArc") => Some(&mut info.story_arcs),
        (Some("Characters"), "Character") => Some(&mut info.characters),
        (Some("Teams"), "Team") => Some(&mut info.teams),
        (Some("Locations"), "Location") => Some(&mut info.locations),
        (Some("Tags"), "Tag") => Some(&mut info.tags),
        (Some("Genres"), "Genre") => Some(&mut info.genres),
        (Some("Reprints"), "Reprint") => Some(&mut info.reprints),
        _ => None,
    };
    if let Some(list) = leaf_into_list {
        list.push(val.to_string());
        return;
    }

    match (parent, name) {
        // Credits — both shapes. Schema: `<Credit><Creator>Name</Creator>
        // <Roles><Role>Writer</Role></Roles>`. Legacy Folio:
        // `<Credit role="Writer"><Creator><Name>…</Name>`.
        (Some("Credit"), "Creator") | (Some("Creator"), "Name") => {
            st.credit.creators.push(val.to_string());
        }
        (Some("Roles"), "Role") => st.credit.roles.push(role_key_from_metron(val)),

        // `<IDS><ID source primary>` or a top-level `<ID source>`.
        (Some("IDS") | Some("MetronInfo"), "ID") => {
            let key = st
                .id_source
                .as_deref()
                .map(folio_id_source)
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            if st.id_primary {
                info.primary_id_source = Some(key.clone());
            }
            info.ids.insert(key, val.to_string());
        }

        // Nested publisher / series / arc / universe / GTIN / URL leaves.
        (Some("Publisher"), "Name") => str_field!(publisher),
        (Some("Publisher"), "Imprint") => str_field!(imprint),
        (Some("Series"), "Name") => str_field!(series),
        (Some("Series"), "SortName") => str_field!(series_sort_name),
        (Some("Series"), "Volume") => int_field!(volume),
        (Some("Series"), "Format") => str_field!(series_format),
        (Some("Series"), "StartYear") => {
            info.series_start_year = parse_xml_date(val).map(|(y, _, _)| y);
        }
        (Some("Series"), "EndYear") => {
            info.series_end_year = parse_xml_date(val).map(|(y, _, _)| y);
        }
        (Some("Series"), "Status") => str_field!(series_status),
        (Some("Series"), "IssueCount") => int_field!(series_issue_count),
        (Some("Series"), "Summary") => str_field!(series_summary),
        (Some("Arc"), "Name") => st.arc.name = Some(val.to_string()),
        (Some("Arc"), "Number") => st.arc.number = val.parse().ok(),
        (Some("Universe"), "Name") => info.universes.push(val.to_string()),
        (Some("Stories"), "Story") => {
            info.title = Some(match info.title.take() {
                Some(prev) => format!("{prev}; {val}"),
                None => val.to_string(),
            });
        }
        (Some("GTIN"), "ISBN") => {
            info.ids.insert("isbn".into(), val.to_string());
        }
        (Some("GTIN"), "UPC") => {
            info.ids.insert("upc".into(), val.to_string());
        }
        (Some("URLs"), "URL") => {
            if st.url_primary && info.primary_url.is_none() {
                info.primary_url = Some(val.to_string());
            } else {
                info.urls.push(val.to_string());
            }
        }

        // Scalars that are direct children of <MetronInfo>: the schema's
        // and the pre-schema Folio names.
        _ if top_level => match name {
            "Title" => str_field!(title),
            "Series" => str_field!(series),
            "Publisher" => str_field!(publisher),
            "Imprint" => str_field!(imprint),
            "Number" => str_field!(number),
            "Volume" => int_field!(volume),
            "Year" => int_field!(year),
            "Month" => int_field!(month),
            "Day" => int_field!(day),
            "CoverDate" => {
                if let Some((y, m, d)) = parse_xml_date(val) {
                    info.year = Some(y);
                    info.month = m;
                    info.day = d;
                }
            }
            "StoreDate" => {
                if looks_like_date(val) {
                    str_field!(store_date)
                }
            }
            "PageCount" => int_field!(page_count),
            "Summary" => str_field!(summary),
            "Notes" => str_field!(notes),
            "AgeRating" => str_field!(age_rating),
            "Language" => str_field!(language),
            "Manga" => str_field!(manga),
            "GTIN" => str_field!(gtin),
            "LastModified" => str_field!(last_modified),
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-schema shape Folio wrote until audit 2026-11 WP A4 (and
    /// the legacy `<Credit role>` shape before WP-8.1).
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

    /// The upstream `schema/v1.2/Sample.xml`, lightly trimmed.
    const SCHEMA_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetronInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="MetronInfo.xsd">
    <IDS>
        <ID source="Metron" primary="true">290431</ID>
        <ID source="Comic Vine">12345</ID>
        <ID source="Grand Comics Database">543</ID>
        <ID source="MangaDex">8b34f37a-0181-4f0b-8ce3-01217e9a602c</ID>
    </IDS>
    <Publisher id="12345">
        <Name>DC Comics</Name>
        <Imprint id="1234">Vertigo</Imprint>
    </Publisher>
    <Series id="65478" lang="en">
        <Name>Justice League</Name>
        <SortName>Justice League</SortName>
        <Volume>2</Volume>
        <Format>Single Issue</Format>
        <StartYear>1970</StartYear>
        <EndYear>1975</EndYear>
        <Status>Completed</Status>
        <IssueCount>60</IssueCount>
        <VolumeCount>3</VolumeCount>
        <AlternativeNames>
            <AlternativeName id="1234">Foo</AlternativeName>
        </AlternativeNames>
        <Summary>The World's Greatest Super-Heroes unite.</Summary>
    </Series>
    <Number>1</Number>
    <Stories>
        <Story id="12">Justice League, Part One</Story>
        <Story>Justice League, Part Two</Story>
    </Stories>
    <Summary>Batman has discovered a dark evil.</Summary>
    <Notes>Nothing really to say.</Notes>
    <Prices>
        <Price country="US">3.99</Price>
    </Prices>
    <CoverDate>2011-10-01</CoverDate>
    <StoreDate>2011-08-31</StoreDate>
    <PageCount>32</PageCount>
    <Genres>
        <Genre id="98745">Super-Hero</Genre>
        <Genre>Crime</Genre>
    </Genres>
    <Tags>
        <Tag id="78945">Foo</Tag>
    </Tags>
    <Arcs>
        <Arc id="78945">
            <Name>Origin</Name>
            <Number>1</Number>
        </Arc>
        <Arc>
            <Name>The New 52!</Name>
        </Arc>
    </Arcs>
    <Characters>
        <Character id="45678">Aquaman</Character>
        <Character>Batman</Character>
    </Characters>
    <Teams>
        <Team id="49948">Justice League</Team>
    </Teams>
    <Universes>
        <Universe id="24">
            <Name>ABC</Name>
            <Designation>Earth 25</Designation>
        </Universe>
    </Universes>
    <Locations>
        <Location id="12389">Gotham City</Location>
    </Locations>
    <GTIN>
        <ISBN>1234567890123</ISBN>
        <UPC>76194130593600111</UPC>
    </GTIN>
    <AgeRating>Everyone</AgeRating>
    <Reprints>
        <Reprint id="65498">Foo Bar #001 (2002)</Reprint>
    </Reprints>
    <URLs>
        <URL primary="true">https://comicvine.gamespot.com/justice-league-1/4000-290431/</URL>
        <URL>https://foo.bar</URL>
    </URLs>
    <Credits>
        <Credit>
            <Creator id="32165">Geoff Johns</Creator>
            <Roles>
                <Role id="32165">Writer</Role>
            </Roles>
        </Credit>
        <Credit>
            <Creator>Jim Lee</Creator>
            <Roles>
                <Role>Penciller</Role>
                <Role>Cover</Role>
            </Roles>
        </Credit>
    </Credits>
    <LastModified>2023-05-31T09:00:46.300882-04:00</LastModified>
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

    /// The v1.2 schema shape (upstream Sample.xml) maps onto the same
    /// struct the pre-schema shape fills.
    #[test]
    fn parses_the_schema_shape() {
        let info = parse(SCHEMA_SAMPLE.as_bytes()).expect("parse");
        assert_eq!(info.ids.get("metron").map(String::as_str), Some("290431"));
        assert_eq!(info.ids.get("comicvine").map(String::as_str), Some("12345"));
        assert_eq!(info.ids.get("gcd").map(String::as_str), Some("543"));
        assert_eq!(
            info.ids.get("mangadex").map(String::as_str),
            Some("8b34f37a-0181-4f0b-8ce3-01217e9a602c")
        );
        assert_eq!(info.primary_id_source.as_deref(), Some("metron"));
        assert_eq!(info.publisher.as_deref(), Some("DC Comics"));
        assert_eq!(info.imprint.as_deref(), Some("Vertigo"));
        assert_eq!(info.series.as_deref(), Some("Justice League"));
        assert_eq!(info.series_id.as_deref(), Some("65478"));
        assert_eq!(info.language.as_deref(), Some("en"));
        assert_eq!(info.series_sort_name.as_deref(), Some("Justice League"));
        assert_eq!(info.volume, Some(2));
        assert_eq!(info.series_format.as_deref(), Some("Single Issue"));
        assert_eq!(info.series_start_year, Some(1970));
        assert_eq!(info.series_end_year, Some(1975));
        assert_eq!(info.series_status.as_deref(), Some("Completed"));
        assert_eq!(info.series_issue_count, Some(60));
        assert_eq!(
            info.series_summary.as_deref(),
            Some("The World's Greatest Super-Heroes unite.")
        );
        assert_eq!(info.number.as_deref(), Some("1"));
        assert_eq!(
            info.title.as_deref(),
            Some("Justice League, Part One; Justice League, Part Two")
        );
        assert_eq!(
            info.summary.as_deref(),
            Some("Batman has discovered a dark evil.")
        );
        assert_eq!(info.notes.as_deref(), Some("Nothing really to say."));
        assert_eq!(
            (info.year, info.month, info.day),
            (Some(2011), Some(10), Some(1))
        );
        assert_eq!(info.store_date.as_deref(), Some("2011-08-31"));
        assert_eq!(info.page_count, Some(32));
        assert_eq!(info.genres, ["Super-Hero", "Crime"]);
        assert_eq!(info.tags, ["Foo"]);
        assert_eq!(info.story_arcs, ["Origin", "The New 52!"]);
        assert_eq!(info.arc_numbers.get("Origin"), Some(&1));
        assert!(!info.arc_numbers.contains_key("The New 52!"));
        assert_eq!(info.characters, ["Aquaman", "Batman"]);
        assert_eq!(info.teams, ["Justice League"]);
        assert_eq!(info.universes, ["ABC"]);
        assert_eq!(info.locations, ["Gotham City"]);
        assert_eq!(
            info.ids.get("isbn").map(String::as_str),
            Some("1234567890123")
        );
        assert_eq!(
            info.ids.get("upc").map(String::as_str),
            Some("76194130593600111")
        );
        assert_eq!(info.gtin, None);
        assert_eq!(info.age_rating.as_deref(), Some("Everyone"));
        assert_eq!(info.reprints, ["Foo Bar #001 (2002)"]);
        assert_eq!(
            info.primary_url.as_deref(),
            Some("https://comicvine.gamespot.com/justice-league-1/4000-290431/")
        );
        assert_eq!(info.urls, ["https://foo.bar"]);
        assert_eq!(info.web_url(), info.primary_url.as_deref());
        assert_eq!(info.writer().as_deref(), Some("Geoff Johns"));
        assert_eq!(info.penciller().as_deref(), Some("Jim Lee"));
        assert_eq!(info.cover_artist().as_deref(), Some("Jim Lee"));
        assert_eq!(
            info.last_modified.as_deref(),
            Some("2023-05-31T09:00:46.300882-04:00")
        );
        // Nested leaves never reach the passthrough map; elements Folio
        // doesn't model stay out of it too when nested (Prices,
        // VolumeCount, AlternativeNames are containers / nested).
        for k in [
            "Name",
            "ID",
            "Story",
            "Price",
            "Role",
            "Creator",
            "URL",
            "VolumeCount",
        ] {
            assert!(!info.raw.contains_key(k), "{k}: {:?}", info.raw);
        }
        // Unmodelled nested elements have no top-level home; a Folio
        // rewrite drops them (documented).
        assert!(info.raw.contains_key("Number"));
    }

    /// Pre-schema → parse → serialize (schema shape) → parse yields the
    /// same struct: a rescan of a Folio-rewritten archive loses nothing.
    #[test]
    fn old_shape_round_trips_through_the_schema_shape_losslessly() {
        let old = parse(SAMPLE.as_bytes()).expect("parse old");
        let xml = serialize(&old);
        let new = parse(xml.as_bytes()).expect("parse new");
        // What legitimately changes: the schema has no slot for `<Manga>`
        // (ComicInfo keeps it); the raw map holds the document's own
        // top-level names; `<CoverDate>` is a full date, so a month-only
        // date reads back with day `1`; `<AgeRating>` comes back as the
        // schema value. The scanner's merge treats the last two as the
        // same value (`merge_metron_into_comicinfo`), so a rescan of the
        // rewritten archive changes nothing in the database.
        let strip = |mut i: MetronInfo| {
            i.manga = None;
            i.raw.clear();
            i.day = i.day.or(Some(1));
            i.age_rating = i
                .age_rating
                .as_deref()
                .and_then(metron_age_rating)
                .map(str::to_owned);
            i
        };
        assert_eq!(strip(new), strip(old), "{xml}");
    }

    #[test]
    fn serialize_writes_the_schema_shape() {
        let parsed = parse(SAMPLE.as_bytes()).expect("parse");
        let xml = serialize(&parsed);
        assert!(
            xml.starts_with(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<MetronInfo \
                 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
                 xsi:noNamespaceSchemaLocation=\"https://raw.githubusercontent.com/\
                 Metron-Project/metroninfo/master/schema/v1.2/MetronInfo.xsd\">\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains(
                "  <IDS>\n    <ID source=\"Comic Vine\">67890</ID>\n    \
                 <ID source=\"Metron\">12345</ID>\n  </IDS>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("  <Publisher>\n    <Name>Image Comics</Name>\n  </Publisher>\n"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "  <Series>\n    <Name>Saga</Name>\n    <Volume>1</Volume>\n  </Series>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("  <Stories>\n    <Story>The Boy from Mars</Story>\n  </Stories>\n"),
            "{xml}"
        );
        assert!(xml.contains("<CoverDate>2012-03-01</CoverDate>"), "{xml}");
        assert!(
            xml.contains(
                "  <Arcs>\n    <Arc>\n      <Name>The Will</Name>\n    </Arc>\n    <Arc>\n      \
                 <Name>Volume 1</Name>\n    </Arc>\n  </Arcs>\n"
            ),
            "{xml}"
        );
        assert!(xml.contains("<AgeRating>Mature</AgeRating>"), "{xml}");
        // Pre-schema elements are gone from the root.
        for old in [
            "<Title>",
            "<Year>",
            "<Month>",
            "<Day>",
            "<Manga>",
            "<Language>",
            "<StoryArcs>",
            "<Imprint>",
            "\n  <ID ",
            "xmlns:xsd",
        ] {
            assert!(!xml.contains(old), "{old} still written:\n{xml}");
        }
        // Element order follows the XSD listing.
        let pos = |needle: &str| {
            xml.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{xml}"))
        };
        let order = [
            "<IDS>",
            "<Publisher>",
            "<Series>",
            "<Number>",
            "<Stories>",
            "<Summary>",
            "<CoverDate>",
            "<Arcs>",
            "<Characters>",
            "<AgeRating>",
            "<Credits>",
        ];
        for pair in order.windows(2) {
            assert!(
                pos(pair[0]) < pos(pair[1]),
                "{} before {}\n{xml}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn serialize_writes_every_schema_block() {
        let info = MetronInfo {
            title: Some("Origin, Part 1".into()),
            series: Some("Justice League".into()),
            series_id: Some("65478".into()),
            series_sort_name: Some("Justice League".into()),
            series_format: Some("Ongoing Series".into()),
            series_start_year: Some(2011),
            series_end_year: Some(2016),
            series_status: Some("ended".into()),
            series_issue_count: Some(52),
            series_summary: Some("The World's Greatest Heroes.".into()),
            language: Some("en".into()),
            publisher: Some("DC Comics".into()),
            imprint: Some("Vertigo".into()),
            number: Some("1".into()),
            volume: Some(2),
            year: Some(2011),
            month: Some(10),
            day: Some(1),
            store_date: Some("2011-08-31".into()),
            page_count: Some(32),
            summary: Some("Batman meets Green Lantern.".into()),
            notes: Some("Tagged by Folio.".into()),
            age_rating: Some("Teen".into()),
            genres: vec!["Super-Hero".into()],
            tags: vec!["New 52".into()],
            story_arcs: vec!["Origin".into()],
            arc_numbers: BTreeMap::from([("Origin".to_string(), 1u32)]),
            characters: vec!["Batman".into(), "Hal Jordan".into()],
            teams: vec!["Justice League".into()],
            universes: vec!["Prime Earth".into()],
            locations: vec!["Gotham City".into()],
            reprints: vec!["Justice League #1 (2016)".into()],
            ids: BTreeMap::from([
                ("metron".to_string(), "290431".to_string()),
                ("comicvine".to_string(), "4000-290431".to_string()),
                ("gcd".to_string(), "543".to_string()),
                ("isbn".to_string(), "9781401237882".to_string()),
                ("upc".to_string(), "76194130593600111".to_string()),
                ("asin".to_string(), "B00ABC".to_string()),
            ]),
            primary_id_source: Some("metron".into()),
            primary_url: Some("https://metron.cloud/issue/justice-league-2011-1/".into()),
            urls: vec![
                "https://comicvine.gamespot.com/x/4000-290431/".into(),
                "https://metron.cloud/issue/justice-league-2011-1/".into(),
            ],
            credits: BTreeMap::from([("Writer".to_string(), vec!["Geoff Johns".to_string()])]),
            last_modified: Some("2026-10-31T12:00:00Z".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(
            xml.contains("<ID source=\"Metron\" primary=\"true\">290431</ID>"),
            "{xml}"
        );
        assert!(!xml.contains("B00ABC"), "asin has no schema slot\n{xml}");
        assert!(
            xml.contains(
                "  <Publisher>\n    <Name>DC Comics</Name>\n    <Imprint>Vertigo</Imprint>\n  \
                 </Publisher>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains(
                "  <Series id=\"65478\" lang=\"en\">\n    <Name>Justice League</Name>\n    \
                 <SortName>Justice League</SortName>\n    <Volume>2</Volume>\n    \
                 <Format>Single Issue</Format>\n    <StartYear>2011</StartYear>\n    \
                 <EndYear>2016</EndYear>\n    <Status>Completed</Status>\n    \
                 <IssueCount>52</IssueCount>\n    <Summary>The World's Greatest Heroes.\
                 </Summary>\n  </Series>\n"
            ),
            "{xml}"
        );
        assert!(xml.contains("<StoreDate>2011-08-31</StoreDate>"), "{xml}");
        assert!(xml.contains("<PageCount>32</PageCount>"), "{xml}");
        assert!(
            xml.contains(
                "  <Arcs>\n    <Arc>\n      <Name>Origin</Name>\n      <Number>1</Number>\n    \
                 </Arc>\n  </Arcs>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("  <Universes>\n    <Universe>\n      <Name>Prime Earth</Name>\n    </Universe>\n  </Universes>\n"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "  <Reprints>\n    <Reprint>Justice League #1 (2016)</Reprint>\n  </Reprints>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains(
                "  <GTIN>\n    <ISBN>9781401237882</ISBN>\n    <UPC>76194130593600111</UPC>\n  </GTIN>\n"
            ),
            "{xml}"
        );
        assert!(xml.contains("<AgeRating>Teen</AgeRating>"), "{xml}");
        // The primary URL is written once, first; the duplicate in `urls` is skipped.
        assert!(
            xml.contains(
                "  <URLs>\n    <URL primary=\"true\">https://metron.cloud/issue/justice-league-2011-1/\
                 </URL>\n    <URL>https://comicvine.gamespot.com/x/4000-290431/</URL>\n  </URLs>\n"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("<LastModified>2026-10-31T12:00:00Z</LastModified>"),
            "{xml}"
        );
        // And it all reads back.
        let back = parse(xml.as_bytes()).unwrap();
        assert_eq!(back.ids.get("metron").map(String::as_str), Some("290431"));
        assert_eq!(back.primary_id_source.as_deref(), Some("metron"));
        assert_eq!(back.series_format.as_deref(), Some("Single Issue"));
        assert_eq!(back.series_status.as_deref(), Some("Completed"));
        assert_eq!(back.arc_numbers.get("Origin"), Some(&1));
        assert_eq!(back.universes, ["Prime Earth"]);
        assert_eq!(back.urls, ["https://comicvine.gamespot.com/x/4000-290431/"]);
        assert_eq!(
            back.ids.get("isbn").map(String::as_str),
            Some("9781401237882")
        );
        assert_eq!(
            back.ids.get("upc").map(String::as_str),
            Some("76194130593600111")
        );
    }

    /// The pre-schema bare `<GTIN>` (and its `<ID source="gtin">` twin the
    /// old composer also wrote) lands in `<GTIN><UPC>` or `<ISBN>` by shape.
    #[test]
    fn legacy_gtin_is_classified_into_the_gtin_block() {
        let mut info = MetronInfo {
            gtin: Some("76194130593600111".into()),
            ..MetronInfo::default()
        };
        info.ids.insert("gtin".into(), "76194130593600111".into());
        let xml = serialize(&info);
        assert!(
            xml.contains("  <GTIN>\n    <UPC>76194130593600111</UPC>\n  </GTIN>\n"),
            "{xml}"
        );
        assert!(!xml.contains("source=\"gtin\""), "{xml}");
        assert!(!xml.contains("<IDS>"), "{xml}");

        let isbn = MetronInfo {
            gtin: Some("978-1-4012-3788-2".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&isbn);
        assert!(xml.contains("<ISBN>978-1-4012-3788-2</ISBN>"), "{xml}");
        assert!(!xml.contains("<UPC>"), "{xml}");

        // Junk is not a barcode.
        let junk = MetronInfo {
            gtin: Some("n/a".into()),
            ..MetronInfo::default()
        };
        assert!(!serialize(&junk).contains("<GTIN>"));
    }

    #[test]
    fn schema_enumeration_mappings() {
        for (key, want) in [
            ("comicvine", Some("Comic Vine")),
            ("metron", Some("Metron")),
            ("gcd", Some("Grand Comics Database")),
            ("locg", Some("League of Comic Geeks")),
            ("mal", Some("MyAnimeList")),
            ("isbn", None),
            ("upc", None),
            ("gtin", None),
            ("asin", None),
            ("doi", None),
            ("future_provider", None),
        ] {
            assert_eq!(metron_id_source(key), want, "{key}");
        }
        for (name, want) in [
            ("Comic Vine", "comicvine"),
            ("comic vine", "comicvine"),
            ("comicvine", "comicvine"),
            ("cv", "comicvine"),
            ("Metron", "metron"),
            ("Grand Comics Database", "gcd"),
            ("gcd", "gcd"),
            ("League of Comic Geeks", "locg"),
            ("MyAnimeList", "mal"),
            ("MangaDex", "mangadex"),
            ("Future Provider", "futureprovider"),
        ] {
            assert_eq!(folio_id_source(name), want, "{name}");
        }
        for (value, want) in [
            ("Mature 17+", Some("Mature")),
            ("Teen", Some("Teen")),
            ("Teen Plus", Some("Teen Plus")),
            ("teen plus", Some("Teen Plus")),
            ("Everyone 10+", Some("Everyone")),
            ("MA15+", Some("Teen Plus")),
            ("Adults Only 18+", Some("Mature")),
            ("X18+", Some("Explicit")),
            ("Rating Pending", Some("Unknown")),
            ("Adult", Some("Adult")),
            ("Foo", None),
        ] {
            assert_eq!(metron_age_rating(value), want, "{value}");
        }
        for (value, want) in [
            ("Ongoing Series", Some("Single Issue")),
            ("Cancelled Series", Some("Single Issue")),
            ("Annual Series", Some("Annual")),
            ("Annual", Some("Annual")),
            ("Hard Cover", Some("Hardcover")),
            ("TPB", Some("Trade Paperback")),
            ("Trade Paperback", Some("Trade Paperback")),
            ("Limited Series", Some("Limited Series")),
            ("One-Shot", Some("One-Shot")),
            ("One Shot", Some("One-Shot")),
            ("Graphic Novel", Some("Graphic Novel")),
            ("Digital Chapters", Some("Digital Chapter")),
            ("Web Comic", None),
        ] {
            assert_eq!(metron_series_format(value), want, "{value}");
        }
        for (value, want) in [
            ("continuing", Some("Ongoing")),
            ("ended", Some("Completed")),
            ("cancelled", Some("Cancelled")),
            ("hiatus", Some("Hiatus")),
            ("Completed", Some("Completed")),
            ("limited", None),
        ] {
            assert_eq!(metron_series_status(value), want, "{value}");
        }
    }

    #[test]
    fn cover_date_defaults_missing_parts_to_the_first() {
        let mut info = MetronInfo {
            year: Some(2021),
            ..MetronInfo::default()
        };
        assert!(serialize(&info).contains("<CoverDate>2021-01-01</CoverDate>"));
        info.month = Some(7);
        assert!(serialize(&info).contains("<CoverDate>2021-07-01</CoverDate>"));
        info.day = Some(14);
        assert!(serialize(&info).contains("<CoverDate>2021-07-14</CoverDate>"));
        info.month = Some(13);
        assert!(!serialize(&info).contains("<CoverDate>"));
        // A day the month lacks is not an `xs:date`; the month survives.
        info.month = Some(2);
        info.day = Some(31);
        assert!(serialize(&info).contains("<CoverDate>2021-02-01</CoverDate>"));
        info.year = Some(2024);
        info.day = Some(29);
        assert!(serialize(&info).contains("<CoverDate>2024-02-29</CoverDate>"));
        info.year = Some(2023);
        assert!(serialize(&info).contains("<CoverDate>2023-02-01</CoverDate>"));
        info.month = None;
        info.year = None;
        assert!(!serialize(&info).contains("<CoverDate>"));
    }

    #[test]
    fn xml_dates_parse_tolerantly() {
        assert_eq!(
            parse_xml_date("2021-07-14"),
            Some((2021, Some(7), Some(14)))
        );
        assert_eq!(parse_xml_date("2021-07"), Some((2021, Some(7), None)));
        assert_eq!(parse_xml_date("2021"), Some((2021, None, None)));
        assert_eq!(parse_xml_date("2021Z"), Some((2021, None, None)));
        assert_eq!(
            parse_xml_date("2021-07-14Z"),
            Some((2021, Some(7), Some(14)))
        );
        assert_eq!(
            parse_xml_date("2021-07-14-04:00"),
            Some((2021, Some(7), Some(14)))
        );
        assert_eq!(
            parse_xml_date("2023-05-31T09:00:46.300882-04:00"),
            Some((2023, Some(5), Some(31)))
        );
        assert_eq!(parse_xml_date("2021-13-01"), None);
        assert_eq!(parse_xml_date("July 2021"), None);
        assert_eq!(parse_xml_date(""), None);
    }

    #[test]
    fn series_assertions_are_honoured() {
        // An end year before the start year is dropped; Ongoing gives way
        // to a written EndYear.
        let info = MetronInfo {
            series: Some("X".into()),
            series_start_year: Some(2011),
            series_end_year: Some(2001),
            series_status: Some("continuing".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(xml.contains("<StartYear>2011</StartYear>"), "{xml}");
        assert!(!xml.contains("<EndYear>"), "{xml}");
        assert!(xml.contains("<Status>Ongoing</Status>"), "{xml}");
        let info = MetronInfo {
            series_end_year: Some(2016),
            ..info
        };
        let xml = serialize(&info);
        assert!(xml.contains("<EndYear>2016</EndYear>"), "{xml}");
        assert!(!xml.contains("<Status>"), "{xml}");
        // No publisher name → no block (the schema requires <Name>); an
        // imprint alone is dropped. An invalid language tag is omitted.
        let info = MetronInfo {
            series: Some("X".into()),
            imprint: Some("Vertigo".into()),
            language: Some("English".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(!xml.contains("<Publisher>"), "{xml}");
        assert!(!xml.contains("Vertigo"), "{xml}");
        assert!(xml.contains("  <Series>\n"), "{xml}");
        assert!(is_language_code("en"));
        assert!(is_language_code("pt-BR"));
        assert!(is_language_code("zh-Hant"));
        assert!(!is_language_code("e"));
        assert!(!is_language_code("en_US"));
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
        // `Mature 17+` has no schema spelling; it comes back as the
        // schema value the scanner treats as the same rating.
        assert_eq!(reparsed.age_rating.as_deref(), Some("Mature"));
        assert_eq!(
            metron_age_rating(parsed.age_rating.as_deref().unwrap()),
            reparsed.age_rating.as_deref()
        );
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

    /// The schema's credit shape (MetronInfo v1.0+ `creditType`, as in
    /// the upstream `Sample.xml`): name as `<Creator>` text, roles under
    /// `<Roles>`, several roles per creator.
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
        // Each id round-trips through the schema's `<IDS><ID source>`.
        assert!(xml.contains(r#"<ID source="Metron">12345</ID>"#), "{xml}");
        assert!(
            xml.contains(r#"<ID source="Comic Vine">67890</ID>"#),
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
        assert_eq!(reparsed.primary_id_source, None);
    }

    /// Other tools' spellings: a bare top-level `<ID>` and schema source
    /// names in any case all land on the Folio keys.
    #[test]
    fn id_sources_are_read_in_every_spelling() {
        let xml = r#"<?xml version="1.0"?>
<MetronInfo>
  <ID source="Comic Vine">1</ID>
  <IDS>
    <ID source="metron" primary="true">2</ID>
    <ID source="GRAND COMICS DATABASE">3</ID>
    <ID source="Future Provider">4</ID>
    <ID>5</ID>
  </IDS>
</MetronInfo>"#;
        let info = parse(xml.as_bytes()).unwrap();
        assert_eq!(info.ids.get("comicvine").map(String::as_str), Some("1"));
        assert_eq!(info.ids.get("metron").map(String::as_str), Some("2"));
        assert_eq!(info.ids.get("gcd").map(String::as_str), Some("3"));
        assert_eq!(
            info.ids.get("futureprovider").map(String::as_str),
            Some("4")
        );
        assert_eq!(info.ids.get("unknown").map(String::as_str), Some("5"));
        assert_eq!(info.primary_id_source.as_deref(), Some("metron"));
        // Unknown sources are read but have no schema slot on write; the
        // top-level `<ID>` copy in `raw` is a typed name and is not
        // re-emitted either.
        let out = serialize(&info);
        assert!(!out.contains(">4<"), "{out}");
        assert!(!out.contains(">5<"), "{out}");
        assert!(!out.contains("\n  <ID "), "{out}");
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
    /// corrupt the document; the typed lists / credits carry those. A
    /// pre-schema typed element (`<Title>`) is in `raw` but is never
    /// re-emitted at the root either.
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
        assert_eq!(parsed.primary_url.as_deref(), Some("https://example.com/1"));

        let out = serialize(&parsed);
        assert!(out.contains("<MangaVolume>3</MangaVolume>"), "{out}");
        assert!(
            !out.contains("<Title>"),
            "pre-schema element re-emitted: {out}"
        );
        assert!(out.contains("<Story>X</Story>"), "{out}");
        assert!(
            out.contains("    <URL primary=\"true\">https://example.com/1</URL>\n"),
            "{out}"
        );
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
        assert!(xml.contains("<Story>Only Title</Story>"));
        assert!(!xml.contains("<Series"));
        assert!(!xml.contains("<Arcs"));
        assert!(!xml.contains("<Credits"));
        assert!(!xml.contains("<IDS"));
        assert!(!xml.contains("<GTIN"));
        assert!(!xml.contains("<URLs"));
    }

    #[test]
    fn serialize_escapes_xml_special_chars() {
        let info = MetronInfo {
            title: Some("Tom & Jerry: <ep1>".into()),
            series_id: Some("a\"b".into()),
            series: Some("S".into()),
            ..MetronInfo::default()
        };
        let xml = serialize(&info);
        assert!(
            xml.contains("<Story>Tom &amp; Jerry: &lt;ep1&gt;</Story>"),
            "{xml}"
        );
        assert!(xml.contains("<Series id=\"a&quot;b\">"), "{xml}");
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
