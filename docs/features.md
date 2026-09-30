# Folio — Feature Showcase

Folio is a self-hosted comic server and reader: a single Rust binary in front
of a modern web app, built for people with real collections — long runs,
manga, CBL reading lists, archives tagged by ComicTagger a decade ago, and
the occasional slightly-broken CBZ a strict reader refuses to open.

This document is the master list of what Folio does and how it works, written
for the comic-reading community. It doubles as the source outline for the
image-rich feature showcase (a suggested shot list is at the end).

---

## At a glance

- **A reader built for comics** — single / double / webtoon modes with smart
  auto-detection, true right-to-left manga support, double-page spread
  awareness, and a page strip mini-map.
- **A scanner that respects your files** — CBZ / CBR / CBT ingest, BLAKE3
  content hashing, and stable issue identity that survives retagging. It
  even repairs two classes of corrupt ZIPs that other readers reject.
- **Metadata that matches by cover art** — ComicVine + Metron with
  perceptual-hash cover matching as the primary signal, a per-field preview
  diff before anything is applied, and full provenance tracking ("this field
  was set by Metron on May 3rd").
- **Your archives stay canonical** — optional writeback composes both
  ComicInfo.xml *and* MetronInfo.xml into the archive itself, so
  ComicTagger, Komga, Mylar3, and KOReader all see the same metadata Folio
  does. Your library remains portable; Folio's database is just a cache.
- **CBL reading lists as a first-class citizen** — import from a file, a
  URL, or a built-in GitHub catalog browser; three-tier matching; refresh
  with a structural diff history; export back to `.cbl`.
- **Read anywhere over OPDS** — OPDS 1.2 and 2.0, Page Streaming, and
  bidirectional progress sync with Panels (via a Komga-compatible shim) and
  KOReader (via a sync-server shim).
- **Tap-to-copy text from speech bubbles** — server-side OCR with comic-aware
  bubble detection, in English and Japanese.
- **Genuinely fast** — pages stream straight out of the archive with no
  extraction step, and a measured baseline scans a 67 GB, 1,395-issue
  library cold in about 15 seconds — then re-checks it in half a second.

---

## The reader

### View modes that fit the material

Three view modes — **single**, **double**, and **webtoon** (continuous
vertical scroll) — cycled with a keypress. Double-page mode pairs pages
side-by-side and knows not to pair a page flagged `DoublePage` in
ComicInfo, so wraparound covers and splash spreads render alone instead of
being mangled into a fake pair.

On the first open of a series, Folio picks the mode for you: webtoon when
the median page is tall enough to clearly be a vertical strip, double when
a meaningful share of pages are flagged as spreads or the pages are
landscape, single otherwise. Your manual choice always wins and is
remembered per series.

Fit modes (fit-width / fit-height / original), a brightness slider, a sepia
toggle, and auto-hiding chrome round out the settings popover.

### Manga done properly

Reading direction is resolved through a documented precedence chain, most
specific first:

1. The issue's own ComicInfo `Manga=YesAndRightToLeft` flag.
2. A per-series override — which the scanner **sets automatically** when at
   least 80% of a series' issues declare themselves manga.
3. Your personal default direction preference.
4. The library's default direction.
5. Left-to-right fallback.

Direction awareness runs through everything: arrow keys, tap zones, swipe
gestures, double-page pair ordering, and the page strip all mirror in RTL.
A one-tap per-series toggle in the reader overrides the whole chain.

### Navigation, keyboard, and touch

- Every shortcut is **user-rebindable** (Settings → Keybinds), and `?`
  opens a live, route-aware shortcut sheet.
- Arrows turn pages (direction-aware), `Space` advances, `Home`/`End` jump
  to first/last, and vim users get `g g` / `Shift+G`. `Shift+N` /
  `Shift+P` move between issues.
- On touch: swipe to turn, tap zones (left / chrome-toggle center / right),
  and native pinch zoom — Folio deliberately hands zoom to the browser so
  you can magnify lettering, and suppresses swipe-to-turn while zoomed so
  panning a zoomed page never accidentally flips it.
- The page strip (`m`) is a horizontal thumbnail mini-map: click any page
  to jump, with the current page ringed and kept in view.

### Progress that can't be lost — or accidentally polluted

Progress is tracked per user, per issue, and synced across devices with a
simple, robust rule: within one read, the furthest page wins. Writes are
debounced while you read and force-flushed when the tab closes, so the
last page you flipped to before shutting the lid is the page you resume
on — and a stale flush from the phone you left open can never drag the
position backwards.

Re-reads are first-class: "Read from beginning", or reopening a finished
issue from its cover, starts a new reading run at page 0. A device still
open on the previous run is ignored until it catches up, and each run
counts separately in your stats.

"Finished" is sticky: jumping back to a bookmarked page can never un-finish
an issue — only an explicit "Mark as unread" or a new run does.

Two modes protect your reading history:

- **Incognito** — open an issue without recording progress or a reading
  session at all.
- **Peek** — the mode bookmark links use: glance at a marked page without
  moving your saved position or polluting On Deck, with a
  "Continue from here" banner if you decide to keep reading.

### "Up Next" that understands reading lists

Finish an issue and a small card slides in from the page-turn edge (never
covering the page) with the next thing to read. The resolver is
context-aware, in priority order:

1. **Reading list context** — if you entered the reader from a CBL list,
   next is the next *unfinished entry in that list*, even if it jumps
   publishers and series (with "Issue 12 of 87 in DC Year One" shown on
   the card). That's the difference between reading a crossover event in
   list order and being dumped back into a series you weren't following.
2. **Series order** — otherwise, the next unread issue in the series.
3. **Caught up** — a friendly empty state when there's nothing left.

---

## The library and scanner

### Formats

| Format | Support |
|---|---|
| `.cbz` (ZIP) | Full read + write |
| `.cbt` (TAR) | Full read + write |
| `.cbr` (RAR) | Read; converts to CBZ when edited (RAR can't be written) |
| `.cb7` (7z) | Recognized; flagged as not-yet-supported in library health |

Inside archives: JPEG, PNG, WebP, AVIF, GIF, and JPEG-XL pages.
Password-protected archives are detected and flagged rather than silently
skipped.

### The scanner repairs archives other readers reject

Two real-world corruption patterns — both found in actual published comic
files — are automatically repaired on open:

- **Unicode-path CRC damage**: some publisher tools write ZIP Unicode path
  extra fields with a checksum strict readers choke on. Folio strips the
  offending field in memory and opens the repaired archive.
- **Central-directory offset drift**: archives with stray bytes between
  entries carry wrong offsets in the central directory. Folio detects the
  mismatch, walks the file forward to find each entry's real local header,
  and rebuilds the directory with corrected offsets.

The net effect: archives that `unzip` tolerates but strict comic readers
reject still open in Folio. Failed recoveries fall back to a clear
library-health issue rather than a silent gap in your library.

Recovery is bounded by defensive limits on every archive open (entry
count, total and per-entry size, compression ratio, path traversal), so a
malicious archive can't take the server down.

### Your files' identity survives retagging

Every file is hashed with BLAKE3, but the hash is a *fingerprint*, not the
identity. When you retag an archive with ComicTagger — changing its bytes
— Folio recognizes it as the same issue: reading progress, bookmarks,
notes, ratings, and reading-list membership all survive. A file that's
been both moved *and* retagged still resolves to the same issue.

### Library management for collectors

- Flat (`Library/Series/`) and publisher-nested (`Library/Publisher/Series/`)
  layouts, auto-detected and mixable.
- Per-library cron schedules, scan-on-startup, manual and scoped scans,
  ignore globs, and full scan history.
- Optional per-library **file watching**: inotify on local disks, a light
  directory-mtime poll on NAS mounts (NFS / SMB / FUSE). Changes collapse
  into one scan of just the changed folders; the scan dashboard shows each
  library's watch mode and last trigger.
- Twelve typed **library health issues** (malformed archive, empty folder,
  unsupported format, encrypted archive, …) with automatic resolution when
  the underlying problem is fixed.
- Deleted files are **soft-deleted** with automatic restore if the file
  reappears — an unmounted NAS share doesn't wipe your read states.
- An admin **archive page editor** (opt-in per library) for removing ads
  or fixing page order in-place: atomic rewrites, `.bak` backups with
  one-click restore, and bulk operations across issues.

---

## Metadata

### Two providers that stack, matched by cover art

Folio fetches from **ComicVine** and **Metron**, fanned out in priority
order and merged — a search can show both providers' take on the same
series so you choose which provenance to trust. Grand Comics Database,
Marvel, and League of Comic Geeks IDs found in MetronInfo sidecars or
folder tags (`[cv-12345]`, `[metron-67890]`, `[gcd-…]`) are ingested as
external IDs, so a pre-tagged library lands already matched.

The matcher's primary signal is the **cover itself**. Every cover gets
three 64-bit perceptual hashes, and candidate matching uses ComicTagger's
exact confidence ladder: a near-identical cover confirms a match
regardless of noisy titles; a clearly different cover *vetoes* a
perfect-looking text match (goodbye, wrong-volume matches with identical
names). Variant covers are hashed and held to a stricter threshold, and
when two candidates' covers are almost equally close, Folio deliberately
downgrades confidence and asks you instead of guessing. Text similarity
(title / year / publisher / issue number) is the fallback when no cover
hash exists.

### Preview before apply, and your edits always win

Applying metadata is never a blind overwrite:

- A **preview diff** shows every field's current value against the
  proposed one — fill, replace, or no change — and you apply per-field.
- Every applied field records **provenance**: hover a field and see
  "set by Metron, 2 days ago."
- Any field you've edited by hand is **never silently overwritten** by a
  provider. Overriding user edits requires an explicit, admin-only,
  audited flag.

An optional weekly refresh keeps recent and stale series current, and the
whole pipeline is rate-limit-aware — when a provider's API quota runs out
mid-run, the job parks itself with a resume time instead of failing.

### Writeback: your archives stay the source of truth

With writeback enabled (off by default, double opt-in per library),
applying metadata inverts the usual pipeline: Folio composes **both
ComicInfo.xml and MetronInfo.xml**, rewrites them into the archive
atomically (temp file → full validation that every page survived → optional
`.bak` rotation → rename → fsync), and then re-scans the file so the
database is rebuilt *from the archive*.

The result matters to anyone who's been burned by a metadata silo: your
files are the canonical record. Point ComicTagger, Komga, Mylar3, or
KOReader at the same folder and they see exactly what Folio sees. Delete
Folio tomorrow and you lose nothing.

### Series boundaries, the eternal argument

Providers disagree about where series begin and end — ComicVine lumps a
legacy-renumbered relaunch into one volume where Metron splits it (the
classic case: *Fantastic Four* #600–611). Folio keeps **your** series
whole, exactly as your folders have it, and records the disagreement as a
per-provider issue-number range mapping: issues #600–611 resolve to the
other provider series automatically during search and apply. There's even
a "Detect from providers" button that finds these splits for you.

---

## Organizing a collection

- **Collections** — free-form shelves that hold both series *and*
  individual issues, with drag reordering.
- **Want to Read** — a built-in per-user list, one click from any cover's
  context menu.
- **CBL reading lists** — import from a local `.cbl` file, an HTTPS URL,
  or the built-in **catalog browser** backed by the community CBL GitHub
  repositories (faceted by publisher). Matching is three-tier: exact
  ComicVine ID → exact Metron ID → fuzzy series-name + volume + issue
  number. Every entry lands as matched / ambiguous (top candidates
  preserved for one-click resolution) / missing — and a post-scan hook
  automatically re-resolves missing entries when you add the issues later.
  Lists refresh from upstream (manually or on schedule) with a structural
  diff history, manual match overrides survive refreshes, and lists export
  back to `.cbl`.
- **Story arcs** — built automatically from ComicInfo `StoryArc` /
  `StoryArcNumber` tags.
- **Saved views** — a real filter engine over your library: publisher,
  imprint, year, status, age rating, genres, tags, every credit role
  (writer through translator), characters, teams, locations, plus per-user
  reading state (read / in-progress / unread, unread-issue count, last
  read) and collector rollups like **collection completeness** (do I have
  every issue?) and metadata completeness. Views are pinnable as rails.
- **Custom pages** — build up to 20 of your own pages out of rails (saved
  views, On Deck, lists), each with its own sidebar entry; the sidebar
  itself is fully reorderable with custom headers and spacers.
- **Multi-select everywhere** — bulk mark-read, bulk metadata edit, bulk
  add-to-collection across the major list surfaces.
- **Markers** — four kinds, all page-anchored: bookmarks, Markdown notes,
  region highlights, and page favorites. All searchable from one
  Bookmarks surface (including full-text over note bodies and captured
  OCR text); `]` / `[` jump between bookmarks inside the reader. Notes
  export to Markdown or JSON grouped series → issue → page, and every
  marker has a permalink (`/markers/{id}`, "Copy link") that opens the
  reader at its page.
- **Ratings** — half-star precision on both issues and series.

## Search

One search box (`Ctrl/Cmd+K` or `/`) covers series, issues, markers, and
people, backed by Postgres full-text search with field weighting (a title
hit ranks above a summary hit) and trigram fallback so typos still land.
Results come back with highlighted snippets. Prefixing the query with `>`
turns the box into a command palette with 27 actions. Creator pages give
every writer and artist a browsable presence of their own.

## Text capture (OCR)

Press `x` in the reader and Folio's server-side OCR pipeline outlines
every detected speech bubble on the page. **Tap a bubble** to extract its
text (typically well under a second) and copy it — or drag your own box,
which can snap itself to the tightest detected bubble polygon.

- Two recognizers: **Western** (Tesseract, English) and **Manga**
  (manga-ocr, Japanese) — auto-selected from the series' text language or
  reading direction, overridable per capture.
- Comic-aware post-processing strips the junk OCR usually returns at
  bubble borders (stray `|`, `~`, halftone noise) while preserving
  `!?` and ellipses.
- Results are cached against the archive's *content hash*, so a cache
  survives file moves and is correctly invalidated by retags.

Captured text lands on highlight markers, which makes quotes searchable
later from the Bookmarks page.

## Read anywhere: OPDS

Folio speaks **OPDS 1.2** (Atom) and **OPDS 2.0** (JSON) with identical
data on both, plus the **Page Streaming Extension** (signed, expiring page
URLs — stream pages in a client app without downloading the whole
archive). Auth is a per-app password with scopes (`read` or
`read+progress`), shown once with a ready-to-paste Basic-auth header.

Beyond the catalog basics, personal feeds mirror the web app: Continue
Reading, On Deck, History, New This Month, Want to Read, your CBL lists
(in list order), collections, saved views, custom pages, browse facets,
by-creator feeds, and OpenSearch.

**Progress syncs both ways**, meeting real clients where they are:

| Client | How |
|---|---|
| **Panels** (iOS) | Komga-compatible REST shim (opt-in server mode); verified end-to-end — reads position, writes it back |
| **KOReader** | KOReader-sync-server shim — its document hash is Folio's content hash, so it just works |
| **Chunky** (iOS) | OPDS 1.2 + Page Streaming |
| Anything else | OPDS Progression 1.0 endpoint, plus Folio's native progress API |

For the many readers that ignore progress metadata entirely, Folio plays
compatibility tricks: read-state glyphs in entry titles (`◯` unread, `◐`
in progress, `●` finished, with a page count), and reading-sequence feeds
that surface the next-unfinished issue at the top as "Up Next: …" (with a
per-list opt-out to preserve curated orders).

## Reading stats and log

- **Stats**: totals, per-day activity buckets computed in *your* timezone,
  current and longest reading **streaks**, day-of-week and time-of-day
  patterns, reading pace, re-read counts, and most-read creators.
- **Reading sessions** are captured with lightweight heartbeats and
  sensible thresholds (a 5-second accidental open doesn't count), with a
  full opt-out for the privacy-minded — and incognito mode for one-off
  private reads.
- The **Reading Log** unifies finishes, completed sessions, and new
  bookmarks/notes into one reverse-chronological timeline.
- **Series completion** is collector-aware: "Caught up — series ongoing"
  is distinct from "Complete", and annuals/specials only count toward
  completion if you want them to (per-user format preference, per-series
  override).

## Multi-user and self-hosting

- **Per-library access control**, enforced server-side on every surface —
  search results, creator pages, OPDS feeds, page bytes, streaming URLs.
  Kids' libraries stay invisible, not just hidden, to accounts without
  access (denied lookups 404 so nothing leaks).
- Local accounts (argon2id) and **OIDC/SSO** (Authentik, Dex, etc.), with
  refresh-token rotation and reuse detection.
- **App passwords** per device/app, individually revocable, with
  last-used tracking.
- A complete admin area: users, libraries, scan monitoring with live
  progress, job queues, server logs, an append-only **audit log** of every
  admin action, runtime configuration edited from the UI (SMTP, auth
  policy, workers — no container restarts), metadata dashboard, and stats.
- One Docker Compose stack; the Rust binary is the single public origin.
  Reverse-proxy templates for Caddy, nginx, and Traefik, a Kubernetes
  guide, Prometheus metrics at `/metrics`, and backup docs.
- The web app is an installable **PWA**, and the reader's first-load
  JavaScript is gated in CI so it stays fast on an iPad over Wi-Fi:
  about 117 KB gzip of reader-route JS on top of the shared framework
  runtime, with a 130 KB ceiling. Everything not needed to paint the
  first page (chrome, settings, page strip, markers, end-of-issue card)
  loads after it. Details in `docs/dev/pwa-performance.md`.

---

## Speed and performance

Folio's architecture is built around one idea: **the request path does
almost nothing**. Everything heavy — scanning, thumbnailing, metadata,
archive rewrites — runs in background workers; serving you a page is an
access check, a cache lookup, and a byte stream.

### Scanning: a 67 GB library in ~15 seconds

Measured baseline on a real library — 1,395 CBZs across 60 series, 67 GB,
every file carrying full ComicInfo metadata (developer-workstation NVMe;
absolute numbers vary by hardware):

| Scenario | Time |
|---|---|
| Cold scan of the full 67 GB library | **~15 s** (≈94 files/s; 4.8 GB/s hashed) |
| Re-scan, nothing changed | **~0.5 s** |
| Forced full re-scan over existing data | **~13.4 s** |

Profiling shows the cold scan is bounded by raw disk I/O — no Folio code
(hashing, ZIP parsing, XML, database) even appears in the top 30 CPU
frames. The scanner reads your files as fast as the kernel can deliver
them, in parallel across workers, with sequential-read hints to the OS
and large streaming buffers.

The half-second re-scan comes from a three-tier skip: unchanged folders
are eliminated by directory mtime, unchanged files by size+mtime
fingerprint, and files that did change get hashed and re-ingested — inside
batched transactions, with junction writes diffed so unchanged credits
and tags aren't churned. Concurrent scan triggers for the same library
coalesce into one run.

### Page serving: stream, don't extract

- Pages are **streamed directly out of the archive** — no extraction step,
  no temp files, ever. For the common CBZ case (images stored
  uncompressed), a precomputed offset index streams bytes from a private
  file handle with *no locking at all* — a slow client on page 3 never
  blocks your page 12.
- Full **HTTP Range** support (partial content, resume, `If-Range`), and
  content-derived **ETags**: a revisited page returns `304 Not Modified`
  before the archive is even opened.
- **Width-negotiated page variants** (480 / 720 / 1080 / 1600 px WebP)
  serve a phone-sized image to a phone and full resolution to a 4K
  monitor, via `srcset` — never upscaled, always full-res in
  original-fit and pinch-zoom. Variants live in a size-capped
  (default 2 GiB) LRU disk cache with atomic writes.
- An LRU of open archive handles skips re-opening and re-parsing archives
  across consecutive page requests.
- Meanwhile the client decodes ahead: 3 pages prefetched and *pre-decoded*
  (not just fetched) so a page turn paints instantly, with spread-aware
  walking in double mode.

### Thumbnails and covers

- WebP covers in two sizes plus lazy per-page strip thumbnails, generated
  by post-scan workers — and only for what needs it: a rescan of a warm
  library enqueues nothing, and covers are only regenerated when file
  bytes actually changed.
- Each cover is decoded **once** and reused for the thumbnail, the small
  variant, and all three perceptual hashes.
- Cold-cache thumbnail requests are generated inline behind a semaphore
  sized to browser connection bursts, so a fresh page of 40 covers
  backfills gracefully instead of stampeding the server.
- A daily sweep garbage-collects orphaned artifacts.

### Everything else

- **Rust end-to-end on the hot path.** The Rust server is the public
  origin; the web UI's Node process is only an internal SSR upstream for
  HTML — page bytes, thumbnails, OPDS, and streaming never touch it.
- **Cursor pagination everywhere**, with row counts computed only on the
  first page — deep-scrolling a 10,000-issue library never pays for a
  `COUNT(*)`, and no list surface silently truncates.
- **Cover matching costs almost nothing at decision time** — perceptual
  hashes are 64-bit integers compared with XOR + popcount; the image work
  happened once, at scan time.
- Long-running work returns `202` with a job id immediately and reports
  progress over WebSocket — scan and thumbnail progress land in the UI
  live, without polling.

---

## Appendix: suggested shot list for the visual showcase

For the follow-up image/video pass, these are the moments that demo best:

1. **Reader** — a double-page spread rendering unpaired; RTL manga with
   mirrored page strip; the end-of-issue Up Next card in CBL context
   ("Issue 12 of 87 in …"); the `?` shortcut sheet.
2. **Scanner** — live scan progress over a large library (the numbers
   sell it); a library-health panel showing a repaired/flagged archive;
   the ~0.5 s "nothing changed" rescan toast.
3. **Metadata** — the match dialog with cover-similarity candidates; the
   per-field preview diff with a user-edited field shown protected; the
   provenance tooltip; a MetronInfo.xml visible inside an archive after
   writeback.
4. **CBL** — the GitHub catalog browser; an import resolving with
   matched/ambiguous/missing chips; the refresh diff history.
5. **OCR** — bubble outlines appearing on a page, tap, extracted text
   copied — one continuous clip; a Japanese example.
6. **Organization** — a custom page assembled from rails; the saved-view
   filter builder showing `collection_completeness = incomplete`;
   multi-select bulk actions.
7. **OPDS** — Folio's library inside Panels with read-state glyphs and
   position sync; KOReader picking up progress mid-issue.
8. **Stats** — the streak + heatmap view; the reading log timeline.

---

*Numbers and behavior current as of v0.27.x (July 2026). Performance
figures are from the repository's measured baseline
(`docs/dev/scanner-perf.md`) on developer hardware; your absolute numbers
will vary with disk and CPU.*
