//! Comic Book List (CBL) reading-list parser (saved-views M4).
//!
//! Parses the ComicRack `.cbl` XML format used by community catalogs
//! like `DieselTech/CBL-ReadingLists`. Sample shape:
//!
//! ```xml
//! <ReadingList>
//!   <Name>Invincible Universe</Name>
//!   <NumIssues>269</NumIssues>
//!   <Books>
//!     <Book Series="Tech Jacket" Number="1" Volume="2002" Year="2002">
//!       <Database Name="cv" Series="22158" Issue="133284" />
//!     </Book>
//!     ...
//!   </Books>
//!   <Matchers />
//! </ReadingList>
//! ```
//!
//! Tolerant of unknown tags (forward-compat); only the path
//! `ReadingList/Books/Book[/Database]` is interpreted. `<Matchers>` is
//! the optional smart-list rules section — v1 doesn't evaluate them but
//! flags `matchers_present` so the UI can warn.
//!
//! XXE-safe: built on `quick-xml`, which doesn't resolve external
//! entities by default. Any `<!DOCTYPE>` declaration causes a parse
//! failure with [`ParseError::DoctypeRejected`].
//!
//! Capped at 4 MiB input size per the saved-views plan (Q7/C7).

use crate::ParseError;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
/// Soft cap on books per file. The plan caps at 5000; lift the limit
/// here so the parser is tolerant and let the API enforce the cap with
/// a clear error.
const MAX_BOOKS: usize = 50_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedCbl {
    pub name: String,
    pub num_issues_declared: Option<i32>,
    pub matchers_present: bool,
    pub books: Vec<ParsedCblBook>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedCblBook {
    pub series: String,
    pub number: String,
    pub volume: Option<String>,
    pub year: Option<String>,
    /// External-database IDs from `<Database>` children. `name`
    /// canonicalized to lowercase (`"cv"` / `"metron"` / `"gcd"` etc.).
    pub databases: Vec<ParsedCblDatabase>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedCblDatabase {
    pub name: String,
    pub series: Option<String>,
    pub issue: Option<String>,
}

impl ParsedCblBook {
    /// ComicVine issue id when a `<Database Name="cv">` row is present.
    pub fn comicvine_issue_id(&self) -> Option<i32> {
        self.databases
            .iter()
            .find(|d| d.name == "cv")
            .and_then(|d| d.issue.as_deref())
            .and_then(|s| s.parse().ok())
    }

    /// ComicVine series id when present.
    pub fn comicvine_series_id(&self) -> Option<i32> {
        self.databases
            .iter()
            .find(|d| d.name == "cv")
            .and_then(|d| d.series.as_deref())
            .and_then(|s| s.parse().ok())
    }

    pub fn metron_issue_id(&self) -> Option<i32> {
        self.databases
            .iter()
            .find(|d| d.name == "metron")
            .and_then(|d| d.issue.as_deref())
            .and_then(|s| s.parse().ok())
    }

    pub fn metron_series_id(&self) -> Option<i32> {
        self.databases
            .iter()
            .find(|d| d.name == "metron")
            .and_then(|d| d.series.as_deref())
            .and_then(|s| s.parse().ok())
    }
}

pub fn parse(bytes: &[u8]) -> Result<ParsedCbl, ParseError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ParseError::TooLarge {
            actual: bytes.len(),
            limit: MAX_INPUT_BYTES,
        });
    }

    let mut reader = Reader::from_reader(bytes);
    let cfg = reader.config_mut();
    cfg.trim_text(true);
    cfg.expand_empty_elements = false;

    let mut out = ParsedCbl::default();
    let mut buf = Vec::with_capacity(2048);
    let mut path: Vec<String> = Vec::with_capacity(8);
    let mut current_text = String::new();
    let mut current_book: Option<ParsedCblBook> = None;
    // Tracks whether the currently-open <Matchers> contains any nested
    // start elements. Empty `<Matchers />` doesn't set the flag.
    let mut in_matchers = false;
    let mut matchers_has_children = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::DocType(_)) => return Err(ParseError::DoctypeRejected),

            Ok(Event::Start(e)) => {
                let name = element_name(&e)?;
                if in_matchers && name != "Matchers" {
                    matchers_has_children = true;
                }
                match name.as_str() {
                    "Book" if path.last().map(String::as_str) == Some("Books") => {
                        let book = book_from_start(&e)?;
                        current_book = Some(book);
                    }
                    "Matchers" => in_matchers = true,
                    _ => {}
                }
                path.push(name);
                current_text.clear();
            }

            Ok(Event::Empty(e)) => {
                let name = element_name(&e)?;
                if in_matchers && name != "Matchers" {
                    matchers_has_children = true;
                }
                let parent = path.last().map(String::as_str);
                if name == "Database" && parent == Some("Book") {
                    if let Some(book) = current_book.as_mut() {
                        book.databases.push(database_from_attrs(&e)?);
                    }
                } else if name == "Book" && parent == Some("Books") {
                    let book = book_from_start(&e)?;
                    if out.books.len() < MAX_BOOKS {
                        out.books.push(book);
                    }
                }
                // Empty <Matchers /> is the common case; don't flip the flag.
            }

            Ok(Event::End(e)) => {
                let name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| ParseError::Malformed(e.to_string()))?
                    .to_string();
                match name.as_str() {
                    "Book" => {
                        if let Some(book) = current_book.take()
                            && out.books.len() < MAX_BOOKS
                        {
                            out.books.push(book);
                        }
                    }
                    "Matchers" => {
                        if matchers_has_children {
                            out.matchers_present = true;
                        }
                        in_matchers = false;
                        matchers_has_children = false;
                    }
                    "Name"
                        if path.first().map(String::as_str) == Some("ReadingList")
                            && path.len() == 2 =>
                    {
                        out.name = current_text.trim().to_string();
                    }
                    "NumIssues"
                        if path.first().map(String::as_str) == Some("ReadingList")
                            && path.len() == 2 =>
                    {
                        out.num_issues_declared = current_text.trim().parse().ok();
                    }
                    _ => {}
                }
                path.pop();
                current_text.clear();
            }

            Ok(Event::Text(t)) => {
                // quick-xml 0.40 removed `BytesText::unescape()`; the
                // old single-shot is now `decode()` followed by an
                // explicit `escape::unescape()`. Both can fail (encoding
                // error / invalid entity) and roll up as Malformed.
                let decoded = t
                    .decode()
                    .map_err(|e| ParseError::Malformed(e.to_string()))?;
                let s = quick_xml::escape::unescape(&decoded)
                    .map_err(|e| ParseError::Malformed(e.to_string()))?;
                current_text.push_str(&s);
            }

            Ok(Event::Eof) => break,

            Ok(_) => {}

            Err(e) => return Err(ParseError::Malformed(e.to_string())),
        }
        buf.clear();
    }

    if out.name.is_empty() {
        return Err(ParseError::Malformed(
            "missing <Name> at ReadingList root".into(),
        ));
    }

    Ok(out)
}

fn element_name<'a>(e: &'a quick_xml::events::BytesStart<'a>) -> Result<String, ParseError> {
    let name = std::str::from_utf8(e.name().as_ref())
        .map_err(|e| ParseError::Malformed(e.to_string()))?
        .to_string();
    Ok(name)
}

fn book_from_start(e: &quick_xml::events::BytesStart<'_>) -> Result<ParsedCblBook, ParseError> {
    let mut book = ParsedCblBook::default();
    for attr in e.attributes().with_checks(false).flatten() {
        let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let v = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|c| c.into_owned())
            .unwrap_or_default();
        match k.as_str() {
            "Series" => book.series = v,
            "Number" => book.number = v,
            "Volume" => book.volume = Some(v),
            "Year" => book.year = Some(v),
            _ => {}
        }
    }
    Ok(book)
}

fn database_from_attrs(
    e: &quick_xml::events::BytesStart<'_>,
) -> Result<ParsedCblDatabase, ParseError> {
    let mut db = ParsedCblDatabase::default();
    for attr in e.attributes().with_checks(false).flatten() {
        let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let v = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|c| c.into_owned())
            .unwrap_or_default();
        match k.as_str() {
            "Name" => db.name = v.to_lowercase(),
            "Series" => db.series = Some(v),
            "Issue" => db.issue = Some(v),
            _ => {}
        }
    }
    Ok(db)
}

/// Serialize a [`ParsedCbl`] back to CBL XML — the inverse of [`parse`].
/// Round-trips: `parse(to_xml(&cbl)?.as_bytes())` reproduces the same
/// `name` + `books`. Used by the collections→CBL export (data-liberation
/// 3.3). `<Matchers>` / `num_issues_declared` aren't emitted — they're
/// read-only hints the importer derives, not data we own.
pub fn to_xml(cbl: &ParsedCbl) -> Result<String, quick_xml::Error> {
    use quick_xml::events::{BytesEnd, BytesStart, BytesText};
    use quick_xml::writer::Writer;

    let mut w = Writer::new_with_indent(Vec::new(), b' ', 2);
    w.write_event(Event::Start(BytesStart::new("ReadingList")))?;

    w.write_event(Event::Start(BytesStart::new("Name")))?;
    w.write_event(Event::Text(BytesText::new(&cbl.name)))?;
    w.write_event(Event::End(BytesEnd::new("Name")))?;

    w.write_event(Event::Start(BytesStart::new("Books")))?;
    for b in &cbl.books {
        let mut book = BytesStart::new("Book");
        book.push_attribute(("Series", b.series.as_str()));
        book.push_attribute(("Number", b.number.as_str()));
        if let Some(v) = &b.volume {
            book.push_attribute(("Volume", v.as_str()));
        }
        if let Some(y) = &b.year {
            book.push_attribute(("Year", y.as_str()));
        }
        if b.databases.is_empty() {
            w.write_event(Event::Empty(book))?;
        } else {
            w.write_event(Event::Start(book))?;
            for d in &b.databases {
                let mut db = BytesStart::new("Database");
                db.push_attribute(("Name", d.name.as_str()));
                if let Some(s) = &d.series {
                    db.push_attribute(("Series", s.as_str()));
                }
                if let Some(i) = &d.issue {
                    db.push_attribute(("Issue", i.as_str()));
                }
                w.write_event(Event::Empty(db))?;
            }
            w.write_event(Event::End(BytesEnd::new("Book")))?;
        }
    }
    w.write_event(Event::End(BytesEnd::new("Books")))?;
    w.write_event(Event::End(BytesEnd::new("ReadingList")))?;

    // quick-xml only ever emits valid UTF-8, so the conversion can't fail
    // in practice; map it through the crate error to keep `?` ergonomic.
    let body = String::from_utf8(w.into_inner())
        .map_err(|e| quick_xml::Error::from(std::io::Error::other(e)))?;
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{body}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../docs/sample.cbl");

    #[test]
    fn parses_sample_cbl_metadata() {
        let parsed = parse(SAMPLE.as_bytes()).expect("sample parses");
        assert_eq!(parsed.name, "[Image] Invincible Universe (WEB-KCV)");
        assert_eq!(parsed.num_issues_declared, Some(269));
        // Sample's <Matchers /> is empty.
        assert!(!parsed.matchers_present);
    }

    #[test]
    fn parses_sample_books_with_database_ids() {
        let parsed = parse(SAMPLE.as_bytes()).expect("sample parses");
        assert_eq!(parsed.books.len(), 269, "all books captured");

        let first = &parsed.books[0];
        assert_eq!(first.series, "Tech Jacket");
        assert_eq!(first.number, "1");
        assert_eq!(first.volume.as_deref(), Some("2002"));
        assert_eq!(first.year.as_deref(), Some("2002"));
        assert_eq!(first.comicvine_series_id(), Some(22158));
        assert_eq!(first.comicvine_issue_id(), Some(133284));
        assert_eq!(first.metron_issue_id(), None);
    }

    #[test]
    fn parses_xml_entities_in_series_name() {
        // The sample has "Brit: Red White Black & Blue" stored as
        // `&amp;` in XML. Make sure the unescape happens.
        let parsed = parse(SAMPLE.as_bytes()).unwrap();
        let brit = parsed
            .books
            .iter()
            .find(|b| b.series.contains("Red White"))
            .expect("Brit: Red White entry present");
        assert!(brit.series.contains('&'));
    }

    #[test]
    fn detects_non_empty_matchers() {
        let xml = r#"<?xml version="1.0"?>
            <ReadingList>
                <Name>With Rules</Name>
                <Books></Books>
                <Matchers>
                    <Matcher>some-rule</Matcher>
                </Matchers>
            </ReadingList>"#;
        let parsed = parse(xml.as_bytes()).unwrap();
        assert!(parsed.matchers_present);
    }

    #[test]
    fn rejects_doctype() {
        let xml = r#"<?xml version="1.0"?>
            <!DOCTYPE foo>
            <ReadingList>
                <Name>Bad</Name>
                <Books></Books>
            </ReadingList>"#;
        assert!(matches!(
            parse(xml.as_bytes()),
            Err(ParseError::DoctypeRejected)
        ));
    }

    #[test]
    fn rejects_oversize_input() {
        let xml = "<".repeat(MAX_INPUT_BYTES + 1);
        assert!(matches!(
            parse(xml.as_bytes()),
            Err(ParseError::TooLarge { .. })
        ));
    }

    #[test]
    fn rejects_missing_name() {
        let xml = r"<ReadingList><Books></Books></ReadingList>";
        assert!(matches!(
            parse(xml.as_bytes()),
            Err(ParseError::Malformed(_))
        ));
    }

    #[test]
    fn to_xml_round_trips_through_parse() {
        let cbl = ParsedCbl {
            name: "My Capes".to_string(),
            num_issues_declared: None,
            matchers_present: false,
            books: vec![
                ParsedCblBook {
                    series: "Saga".to_string(),
                    number: "1".to_string(),
                    volume: Some("2012".to_string()),
                    year: Some("2012".to_string()),
                    databases: vec![ParsedCblDatabase {
                        name: "cv".to_string(),
                        series: Some("44144".to_string()),
                        issue: Some("344037".to_string()),
                    }],
                },
                ParsedCblBook {
                    series: "Tech Jacket".to_string(),
                    number: "2".to_string(),
                    volume: None,
                    year: None,
                    databases: vec![],
                },
            ],
        };
        let xml = to_xml(&cbl).unwrap();
        let re = parse(xml.as_bytes()).unwrap();
        assert_eq!(re.name, "My Capes");
        assert_eq!(re.books.len(), 2);
        assert_eq!(re.books[0].series, "Saga");
        assert_eq!(re.books[0].number, "1");
        assert_eq!(re.books[0].volume.as_deref(), Some("2012"));
        assert_eq!(re.books[0].year.as_deref(), Some("2012"));
        assert_eq!(re.books[0].comicvine_series_id(), Some(44144));
        assert_eq!(re.books[0].comicvine_issue_id(), Some(344037));
        assert_eq!(re.books[1].series, "Tech Jacket");
        assert!(re.books[1].databases.is_empty());
    }

    /// The writer still *escapes* special chars to valid XML even though
    /// the reader can't currently reassemble entity-split text (a parser
    /// limitation, tracked separately). Asserting the escaped output
    /// keeps the serializer honest without depending on the reader.
    #[test]
    fn to_xml_escapes_special_chars() {
        let cbl = ParsedCbl {
            name: r#"Cape & "Cowl""#.to_string(),
            num_issues_declared: None,
            matchers_present: false,
            books: vec![ParsedCblBook {
                series: r#"A & B"#.to_string(),
                number: "1".to_string(),
                volume: None,
                year: None,
                databases: vec![],
            }],
        };
        let xml = to_xml(&cbl).unwrap();
        // quick-xml escapes `&` and `"` in both text and attributes.
        assert!(
            xml.contains("Cape &amp; &quot;Cowl&quot;"),
            "name text: {xml}"
        );
        assert!(xml.contains(r#"Series="A &amp; B""#), "series attr: {xml}");
    }
}
