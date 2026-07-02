# Reader page-size variants (FEP-1)

**Status:** shipped (audit 2026-07 Wave 5, decision D1: on-the-fly resize +
on-disk cache).
**Problem:** the reader always fetched full-resolution page bytes — a 390 px
phone downloaded and main-thread-decoded a 2400–4000 px scan for every page,
and the prefetcher warmed up to 5 full-res neighbours. Covers negotiated via
`srcset`; pages didn't. This was the single largest bandwidth + decode cost
in the product.

## Design decisions (settled per plan §Wave 5)

| Choice | Decision | Why |
|---|---|---|
| Width scheme | Fixed tier ladder **480 / 720 / 1080 / 1600 px** | Fixed tiers maximize cache reuse across devices and bound worst-case cache size to 4 files/page. Requests clamp to the nearest tier ≥ the ask; ≥ intrinsic width falls back to the original bytes (never upscale). |
| Format | **WebP-only, q=80** (libwebp, same encoder as the thumbnail pipeline) | One cache key per (page, tier); WebP decodes everywhere the app runs and is 5–10× smaller than `image`'s lossless codec. No `fmt=` param. |
| Cache | `data_path/cache/pages/{content_hash}/{page}-w{tier}.webp`, budget `COMIC_PAGE_VARIANT_CACHE_BYTES` (default 2 GiB), LRU by file mtime, debounced background sweep after writes | Keyed on `content_hash` (mutable across archive edits), so an edited archive naturally orphans its old variants — the LRU sweep ages them out; no separate orphan job. Atomic tmp→rename writes; concurrent misses double-compute benignly. |
| Pre-warm | **None** | The reader's decode-and-retain prefetcher already warms neighbours through normal requests, which now carry `?w=` — pre-warming would duplicate it server-side. |

## Server (`api/page_bytes.rs` + `library/page_variants.rs`)

`GET /issues/{id}/pages/{n}?w=<px>`:

1. ACL + issue load exactly as the full-res path (same handler).
2. `w` clamps to the tier ladder; the variant ETag is
   `"{content_hash[..32]}-{page}-w{tier}"`, honored for `If-None-Match`
   **before** any disk/archive work.
3. Cache **hit** → touch mtime (LRU signal) and stream the file with
   `Cache-Control: private, max-age=31536000, immutable` (safe: the key
   embeds `content_hash`).
4. Cache **miss** → read the entry, `decode_limited` (the 20k-px/256 MiB
   caps apply **before** any resize — a dimension bomb dies here, not in
   the resizer), SIMD Lanczos3 downscale (`fast_image_resize`), libwebp
   q=80 encode, atomic write, stream from memory.
5. Intrinsic width ≤ tier → stream the **original** bytes (original MIME)
   under the variant ETag; nothing cached. The client avoids these
   requests anyway (it knows intrinsic dimensions).
6. `Range` is ignored on variant requests (always a full 200) — `<img>`
   never issues ranges; the full-res path keeps its Range/If-Range
   support untouched.

**No `w=` → byte-identical to the pre-FEP-1 behavior** (lock-free Stored
fast path, Range, ETag/304). That is the escape hatch for original-fit and
pinch-zoom.

## Client (`Reader.tsx`, `PageImage.tsx`, `lib/urls.ts`)

- `pageBytesSrcSet(url, intrinsicWidth)` emits one entry per tier below the
  intrinsic width plus the full-res URL at `{intrinsicWidth}w`; `sizes`
  reflects the fit mode (`100vw` for fit-width/webtoon; viewport-derived
  for fit-height).
- `fitMode === "original"` and pinch-zoom (`scale > 1`) swap to the plain
  full-res `src` — zooming always gets original pixels.
- The prefetcher requests the same tier the visible `<img>` would pick
  (`selectPageVariant(cssPx × devicePixelRatio, intrinsicWidth)`), so warmed
  bytes are the bytes actually rendered.

## Ops notes

- Cache sizing: `COMIC_PAGE_VARIANT_CACHE_BYTES` (env, infrastructure side
  of the runtime-config split). 0 disables variant caching (variants are
  still served, recomputed per request).
- The sweep logs at `debug` per eviction pass; the cache directory is safe
  to delete wholesale at any time (cold-start recompute).
