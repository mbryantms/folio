//! Pages Folio kept from a damaged archive.
//!
//! A RAR page whose data fails its checksum used to keep the whole issue
//! out of the library: conversion to CBZ stopped at the first unreadable
//! page. The converter now reads with
//! [`archive::comic_archive::ComicArchive::read_entry_bytes_salvaging`] and
//! writes, for each damaged page,
//!
//! - **salvaged**: the bytes UnRAR decoded, when they still start as an
//!   image (part of the picture is usually wrong; browsers and the
//!   thumbnailer draw what decodes), or
//! - **placeholder**: Folio's stand-in page ([`placeholder_page`]) when
//!   nothing usable survived, so the page count and order stay right.
//!
//! Either way the damage is recorded, never hidden: one
//! `issue_damaged_pages` row per page ([`ConvertedDamage`] at conversion),
//! a `DamagedPage` health finding on every scan that reads the file, the
//! issue's `damaged_pages` on the issue page and in the reader, and the
//! original archive's backup kept past `.bak` retention
//! ([`kept_backups`]).
//!
//! **A record follows the bytes, not the file name.** Each row holds the
//! CRC-32 of the bytes the CBZ stores for the page. Whenever the scanner
//! reads the file it matches the rows against the file's pages
//! ([`resolve`]): same name and CRC, else the same CRC under another name
//! (a page editor rename). A row with no match is gone — the file or the
//! page was replaced — and is deleted. A metadata rewrite copies pages
//! byte for byte, and a moved file keeps its issue id, so neither loses
//! the record.

use entity::issue_damaged_page;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Set};
use std::collections::HashSet;
use std::sync::OnceLock;

/// What the converter wrote for a damaged page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    /// The bytes UnRAR decoded.
    Salvaged,
    /// Folio's stand-in page: nothing usable survived.
    Placeholder,
}

impl DamageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Salvaged => "salvaged",
            Self::Placeholder => "placeholder",
        }
    }
}

/// One damaged page of a conversion, as the converter wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedDamage {
    /// The page's entry name in the CBZ.
    pub entry_name: String,
    /// The entry's name in the original archive.
    pub source_entry: String,
    pub kind: DamageKind,
    /// Static reason from `archive::damage`.
    pub reason: &'static str,
    pub declared_size: u64,
    /// Bytes UnRAR decoded (0 when nothing was).
    pub recovered_size: u64,
    /// CRC-32 of the bytes written for the page.
    pub crc32: u32,
}

/// The damage report of one conversion: the pages and where the original
/// went.
#[derive(Debug, Clone, Default)]
pub struct ConversionDamage {
    pub pages: Vec<ConvertedDamage>,
    /// File name of the archive that was converted.
    pub source_name: String,
    /// Full path of its backup (`<name>.cbr.bak`), when one was kept.
    pub source_backup: Option<String>,
}

/// What a damaged page becomes in the CBZ: the salvaged bytes when they
/// still carry an image signature, else the placeholder. `name` is the
/// entry's name in the original; `written_name` gives the name the CBZ
/// will hold for the kind chosen.
pub fn page_for_damaged_entry(
    name: &str,
    bytes: Vec<u8>,
    damage: &archive::comic_archive::EntryDamage,
    written_name: impl FnOnce(DamageKind) -> String,
) -> (Vec<u8>, ConvertedDamage) {
    let recovered_size = bytes.len() as u64;
    let head = &bytes[..bytes.len().min(archive::image_sniff::SNIFF_LEN)];
    let (out, kind) = if archive::image_sniff::sniff(head).is_some() {
        (bytes, DamageKind::Salvaged)
    } else {
        (placeholder_page().to_vec(), DamageKind::Placeholder)
    };
    let crc32 = crc32fast::hash(&out);
    (
        out,
        ConvertedDamage {
            entry_name: written_name(kind),
            source_entry: name.to_owned(),
            kind,
            reason: damage.reason,
            declared_size: damage.declared_size,
            recovered_size,
            crc32,
        },
    )
}

/// Folio's stand-in for a page nothing usable survived of: a dark 2:3 page
/// with a framed cross, as PNG (pages are served by their sniffed type, so
/// it keeps the original entry name). Built once; always the same bytes.
pub fn placeholder_page() -> &'static [u8] {
    static PAGE: OnceLock<Vec<u8>> = OnceLock::new();
    PAGE.get_or_init(|| {
        use image::{ImageFormat, Rgb, RgbImage};
        use imageproc::drawing::{draw_hollow_rect_mut, draw_line_segment_mut};
        use imageproc::rect::Rect;
        const W: u32 = 800;
        const H: u32 = 1200;
        let mut img = RgbImage::from_pixel(W, H, Rgb([38, 38, 42]));
        let ink = Rgb([110, 110, 118]);
        for inset in 0..6 {
            let m = 120 + inset;
            draw_hollow_rect_mut(
                &mut img,
                Rect::at(m as i32, m as i32).of_size(W - 2 * m, H - 2 * m),
                ink,
            );
        }
        for dx in -3i32..=3 {
            let d = dx as f32;
            draw_line_segment_mut(&mut img, (120.0 + d, 120.0), (680.0 + d, 1080.0), ink);
            draw_line_segment_mut(&mut img, (680.0 + d, 120.0), (120.0 + d, 1080.0), ink);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png)
            .expect("encoding an in-memory PNG cannot fail");
        out.into_inner()
    })
}

/// A recorded damaged page as [`resolve`] matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageRecord {
    pub entry_name: String,
    pub source_entry: String,
    pub kind: String,
    pub reason: String,
    pub declared_size: i64,
    pub recovered_size: i64,
    pub entry_crc32: u32,
    pub source_name: String,
    pub source_backup: Option<String>,
}

impl From<issue_damaged_page::Model> for DamageRecord {
    fn from(m: issue_damaged_page::Model) -> Self {
        Self {
            entry_name: m.entry_name,
            source_entry: m.source_entry,
            kind: m.kind,
            reason: m.reason,
            declared_size: m.declared_size,
            recovered_size: m.recovered_size,
            entry_crc32: u32::try_from(m.entry_crc32).unwrap_or(0),
            source_name: m.source_name,
            source_backup: m.source_backup,
        }
    }
}

impl DamageRecord {
    /// The records a conversion's report starts.
    pub fn from_conversion(report: &ConversionDamage) -> Vec<Self> {
        report
            .pages
            .iter()
            .map(|p| Self {
                entry_name: p.entry_name.clone(),
                source_entry: p.source_entry.clone(),
                kind: p.kind.as_str().to_owned(),
                reason: p.reason.to_owned(),
                declared_size: i64::try_from(p.declared_size).unwrap_or(i64::MAX),
                recovered_size: i64::try_from(p.recovered_size).unwrap_or(i64::MAX),
                entry_crc32: p.crc32,
                source_name: report.source_name.clone(),
                source_backup: report.source_backup.clone(),
            })
            .collect()
    }
}

/// A record that still holds, with where its page is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDamage {
    pub record: DamageRecord,
    pub page_index: u32,
}

/// Match `records` against a file's pages (`(entry name, CRC-32)` in page
/// order): a record holds while a page carries its bytes — the same name
/// and CRC first, else an unclaimed page with the same CRC (the page was
/// renamed). A record no page matches is dropped: the file or page was
/// replaced. A page with no CRC (a format that records none) never
/// matches.
pub fn resolve(records: &[DamageRecord], pages: &[(String, Option<u32>)]) -> Vec<ResolvedDamage> {
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut out: Vec<Option<ResolvedDamage>> = vec![None; records.len()];
    let hit = |rec: &DamageRecord, page: &(String, Option<u32>)| page.1 == Some(rec.entry_crc32);
    for (ri, rec) in records.iter().enumerate() {
        if let Some(pi) = pages
            .iter()
            .position(|p| p.0.eq_ignore_ascii_case(&rec.entry_name) && hit(rec, p))
        {
            claimed.insert(pi);
            out[ri] = Some(placed(rec, pi, &pages[pi].0));
        }
    }
    for (ri, rec) in records.iter().enumerate() {
        if out[ri].is_some() {
            continue;
        }
        if let Some(pi) = (0..pages.len()).find(|pi| !claimed.contains(pi) && hit(rec, &pages[*pi]))
        {
            claimed.insert(pi);
            out[ri] = Some(placed(rec, pi, &pages[pi].0));
        }
    }
    let mut resolved: Vec<ResolvedDamage> = out.into_iter().flatten().collect();
    resolved.sort_by_key(|r| r.page_index);
    resolved
}

fn placed(rec: &DamageRecord, page_index: usize, name: &str) -> ResolvedDamage {
    let mut record = rec.clone();
    record.entry_name = name.to_owned();
    ResolvedDamage {
        record,
        page_index: u32::try_from(page_index).unwrap_or(u32::MAX),
    }
}

/// The records of `issue_id`.
pub async fn load_for_issue<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<Vec<DamageRecord>, sea_orm::DbErr> {
    Ok(issue_damaged_page::Entity::find()
        .filter(issue_damaged_page::Column::IssueId.eq(issue_id))
        .all(db)
        .await?
        .into_iter()
        .map(DamageRecord::from)
        .collect())
}

/// The records of any issue in `library_id` whose file has `content_hash`:
/// a file at a new path (moved, or a copy) with the same bytes is as
/// damaged as the one recorded.
pub async fn load_for_content<C: ConnectionTrait>(
    db: &C,
    library_id: uuid::Uuid,
    content_hash: &str,
) -> Result<Vec<DamageRecord>, sea_orm::DbErr> {
    let issue_ids: Vec<String> = entity::issue::Entity::find()
        .select_only()
        .column(entity::issue::Column::Id)
        .filter(entity::issue::Column::LibraryId.eq(library_id))
        .filter(entity::issue::Column::ContentHash.eq(content_hash))
        .into_tuple()
        .all(db)
        .await?;
    if issue_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    Ok(issue_damaged_page::Entity::find()
        .filter(issue_damaged_page::Column::IssueId.is_in(issue_ids))
        .all(db)
        .await?
        .into_iter()
        .map(DamageRecord::from)
        .filter(|r| seen.insert(r.entry_name.to_ascii_lowercase()))
        .collect())
}

/// Replace `issue_id`'s records with `resolved` (an empty list clears
/// them). A record keeps its first `detected_at`.
pub async fn store<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    resolved: &[ResolvedDamage],
) -> Result<(), sea_orm::DbErr> {
    let existing: Vec<issue_damaged_page::Model> = issue_damaged_page::Entity::find()
        .filter(issue_damaged_page::Column::IssueId.eq(issue_id))
        .all(db)
        .await?;
    let keep: HashSet<String> = resolved
        .iter()
        .map(|r| r.record.entry_name.clone())
        .collect();
    let stale: Vec<String> = existing
        .iter()
        .filter(|m| !keep.contains(&m.entry_name))
        .map(|m| m.entry_name.clone())
        .collect();
    if !stale.is_empty() {
        issue_damaged_page::Entity::delete_many()
            .filter(issue_damaged_page::Column::IssueId.eq(issue_id))
            .filter(issue_damaged_page::Column::EntryName.is_in(stale))
            .exec(db)
            .await?;
    }
    let now = chrono::Utc::now().fixed_offset();
    for r in resolved {
        let detected_at = existing
            .iter()
            .find(|m| m.entry_name == r.record.entry_name)
            .map_or(now, |m| m.detected_at);
        let am = issue_damaged_page::ActiveModel {
            issue_id: Set(issue_id.to_owned()),
            entry_name: Set(r.record.entry_name.clone()),
            source_entry: Set(r.record.source_entry.clone()),
            page_index: Set(i32::try_from(r.page_index).unwrap_or(i32::MAX)),
            kind: Set(r.record.kind.clone()),
            reason: Set(r.record.reason.clone()),
            declared_size: Set(r.record.declared_size),
            recovered_size: Set(r.record.recovered_size),
            entry_crc32: Set(i64::from(r.record.entry_crc32)),
            source_name: Set(r.record.source_name.clone()),
            source_backup: Set(r.record.source_backup.clone()),
            detected_at: Set(detected_at),
        };
        issue_damaged_page::Entity::insert(am)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    issue_damaged_page::Column::IssueId,
                    issue_damaged_page::Column::EntryName,
                ])
                .update_columns([
                    issue_damaged_page::Column::SourceEntry,
                    issue_damaged_page::Column::PageIndex,
                    issue_damaged_page::Column::Kind,
                    issue_damaged_page::Column::Reason,
                    issue_damaged_page::Column::DeclaredSize,
                    issue_damaged_page::Column::RecoveredSize,
                    issue_damaged_page::Column::EntryCrc32,
                    issue_damaged_page::Column::SourceName,
                    issue_damaged_page::Column::SourceBackup,
                ])
                .to_owned(),
            )
            .exec_without_returning(db)
            .await?;
    }
    Ok(())
}

/// Backups of damaged conversions under `library_id`: the only copy of the
/// original pages, so `.bak` retention never removes them while a record
/// points at them.
pub async fn kept_backups<C: ConnectionTrait>(
    db: &C,
    library_id: uuid::Uuid,
) -> Result<HashSet<String>, sea_orm::DbErr> {
    let rows: Vec<Option<String>> = issue_damaged_page::Entity::find()
        .select_only()
        .column(issue_damaged_page::Column::SourceBackup)
        .inner_join(entity::issue::Entity)
        .filter(entity::issue::Column::LibraryId.eq(library_id))
        .filter(issue_damaged_page::Column::SourceBackup.is_not_null())
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, crc: u32) -> DamageRecord {
        DamageRecord {
            entry_name: name.to_owned(),
            source_entry: name.to_owned(),
            kind: "salvaged".to_owned(),
            reason: archive::damage::CHECKSUM_FAILED.to_owned(),
            declared_size: 10,
            recovered_size: 10,
            entry_crc32: crc,
            source_name: "x.cbr".to_owned(),
            source_backup: None,
        }
    }

    fn pages(list: &[(&str, u32)]) -> Vec<(String, Option<u32>)> {
        list.iter()
            .map(|(n, c)| ((*n).to_owned(), Some(*c)))
            .collect()
    }

    #[test]
    fn a_record_holds_while_its_page_has_the_same_bytes() {
        let r = resolve(
            &[rec("p2.jpg", 7)],
            &pages(&[("p1.jpg", 1), ("p2.jpg", 7), ("p3.jpg", 3)]),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].page_index, 1);
        assert_eq!(r[0].record.entry_name, "p2.jpg");
    }

    #[test]
    fn a_replaced_page_clears_its_record() {
        let r = resolve(
            &[rec("p2.jpg", 7)],
            &pages(&[("p1.jpg", 1), ("p2.jpg", 9), ("p3.jpg", 3)]),
        );
        assert!(r.is_empty(), "new bytes under the same name: not damaged");
        assert!(resolve(&[rec("p2.jpg", 7)], &pages(&[("p1.jpg", 1)])).is_empty());
    }

    #[test]
    fn a_renamed_or_moved_page_keeps_its_record() {
        let r = resolve(
            &[rec("p2.jpg", 7)],
            &pages(&[("page-1.jpg", 7), ("page-2.jpg", 1)]),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].page_index, 0);
        assert_eq!(r[0].record.entry_name, "page-1.jpg");
    }

    #[test]
    fn two_placeholders_with_the_same_bytes_claim_one_page_each() {
        let r = resolve(
            &[rec("b.jpg", 5), rec("a.jpg", 5)],
            &pages(&[("a.jpg", 5), ("b.jpg", 5), ("c.jpg", 6)]),
        );
        let got: Vec<(u32, &str)> = r
            .iter()
            .map(|d| (d.page_index, d.record.entry_name.as_str()))
            .collect();
        assert_eq!(got, [(0, "a.jpg"), (1, "b.jpg")]);
    }

    #[test]
    fn a_page_without_a_crc_never_matches() {
        assert!(resolve(&[rec("p.jpg", 7)], &[("p.jpg".to_owned(), None)]).is_empty());
    }

    #[test]
    fn placeholder_is_a_png_with_a_stable_checksum() {
        let a = placeholder_page();
        assert!(a.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(crc32fast::hash(a), crc32fast::hash(placeholder_page()));
        let img = image::load_from_memory(a).unwrap();
        assert_eq!((img.width(), img.height()), (800, 1200));
    }

    #[test]
    fn salvaged_bytes_that_are_no_image_become_the_placeholder() {
        let damage = archive::comic_archive::EntryDamage {
            reason: archive::damage::CHECKSUM_FAILED,
            declared_size: 4,
        };
        let (bytes, d) = page_for_damaged_entry("p.jpg", vec![0, 1, 2, 3], &damage, |k| {
            format!(
                "p0001.{}",
                if k == DamageKind::Placeholder {
                    "png"
                } else {
                    "jpg"
                }
            )
        });
        assert_eq!(d.kind, DamageKind::Placeholder);
        assert_eq!(d.entry_name, "p0001.png");
        assert_eq!(d.source_entry, "p.jpg");
        assert_eq!(bytes, placeholder_page());
        assert_eq!(d.recovered_size, 4);
        assert_eq!(d.crc32, crc32fast::hash(placeholder_page()));

        let jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 9, 9,
        ];
        let (bytes, d) =
            page_for_damaged_entry("p.jpg", jpeg.clone(), &damage, |_| "p0001.jpg".to_owned());
        assert_eq!(d.kind, DamageKind::Salvaged);
        assert_eq!(bytes, jpeg);
        assert_eq!(d.crc32, crc32fast::hash(&jpeg));
    }
}
