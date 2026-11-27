# Damaged pages

A RAR page whose data fails its checksum used to keep the whole issue out
of the library: the scan-time CBR → CBZ conversion stopped at the first
page it could not read, and the file was reported as
`UnsupportedArchiveFormat` on every scan. (Production: The Avengers (1963)
#253 and Iron Man (1968) #120 / #141, one corrupt page each.) Folio now
converts such a file with every page, records which pages are damaged and
says so wherever the issue is shown, while keeping the original.

## Reading a damaged page (`archive`)

UnRAR decodes an entry and checks its CRC only at the end. The `unrar`
crate collects the decoded bytes through UnRAR's data callback and drops
them when the check fails. `archive::rar_salvage` makes the same calls into
the same linked library (`unrar_sys`, the crate's own FFI layer) and keeps
them.

- `ComicArchive::read_entry_bytes_salvaging` returns an `EntryRead`:
  the bytes plus `Option<EntryDamage>` (`archive::damage::CHECKSUM_FAILED`,
  declared size). The default is a plain read; CBR overrides it, using the
  salvage pass only after the crate's read answered "bad data".
- `read_entry_bytes` is unchanged: a damaged page is still an error for the
  page server, thumbnails and everything else.

**Why this is safe.** A CRC catches accidental damage, not tampering: a
hostile file can carry any bytes with a matching checksum. Bytes that fail
it are no less trustworthy than any other page bytes, and they reach the
same decoders (the `image` crate on the server, the browser in the reader)
under the same limits. Nothing protective is relaxed: the entry is found
through its header, its declared size is held to `max_entry_bytes` before
decoding, the callback refuses a byte past that size, and encrypted
entries, other volumes and every error other than UnRAR's "bad data" are
still refused. The unsafe code is the open / read-header / process / close
calls and a callback that only appends to a bounded `Vec`; the handle is
closed by a guard on every path. `unrar_sys` is pinned to the exact version
`unrar` uses (both link the static UnRAR library).

## Converting (`scanner::cbr_convert`)

The scanner converts with `convert_to_cbz_salvaging`; every other caller
(sidecar rewrite, restore-from-backup rebuild) stays strict. For each
damaged page the converter writes

- **salvaged**: the decoded bytes, when they still start with an image
  signature (part of the picture is usually wrong; browsers and the
  thumbnailer draw what decodes), or
- **placeholder**: `damaged_pages::placeholder_page()`, a fixed dark PNG
  with a framed cross, written as `pNNNN.png`, when nothing usable
  survived, so the page count and order stay right.

The conversion returns a `ConversionDamage` report (pages, original file
name, the `.cbr.bak` path). A file conversion still cannot read (a damaged
header, a page even the salvage pass cannot decode) is reported as
`MalformedArchive` with "damaged archive: page …", not
`UnsupportedArchiveFormat`.

## The record (`library::damaged_pages`, `issue_damaged_pages`)

One row per damaged page: issue, entry name in the CBZ, original entry
name, page index, kind, reason, sizes, the **CRC-32 of the bytes the CBZ
holds**, original file name and backup path, first detection time.

A record follows the bytes, not the file name. Whenever the scanner reads
the file it matches the rows against the file's pages (`resolve`): same
name and CRC, else an unclaimed page with the same CRC. Unmatched rows are
deleted. So:

| Event | Record |
| --- | --- |
| Rescan of an unchanged file | untouched (fast path; the finding is kept open) |
| Metadata writeback rewrite (pages copied byte for byte) | kept |
| File moved or renamed (issue id adopted by content hash) | kept, re-checked on the adopted issue |
| Page editor reorder or rename | kept, page index updated |
| Page deleted, or the file replaced by a good copy | cleared, finding resolved |
| Issue deleted | rows cascade |

The CBZ reader exposes the central directory's CRC (`entry_crc32`), so the
check reads no page bytes. The scan manifest carries a `has_damage` flag
per row, so a scan queries the table only for issues that have rows.

## Where it shows

- **Health**: `DamagedPage` (warning), one per page, re-emitted by every
  scan that reads the file.
- **Issue page**: `DamagedPagesBadge` from `IssueDetailView.damaged_pages`
  (every reader, not only admins); a popover names each page and says to
  replace the file.
- **Reader**: `DamagedPageNotice`, a small pill at the bottom while a
  damaged page is on screen (lazy chunk; nothing at the top edge), and the
  one-time open toast names the pages.
- **Backups**: `jobs::backup_prune` never deletes a backup a record points
  at (`damaged_pages::kept_backups`), whatever `archive_backup_retain_days`
  says: it is the only copy of the original pages. If the list cannot be
  read, that library is not pruned that run.

## Not covered

CBZ and CB7 pages that fail their checksum are still read strictly (the
zip and 7z readers error at the end of the entry); they could take the same
path later. Restoring a damaged conversion's `.cbr.bak` through the page
editor rebuilds strictly and so refuses it.
