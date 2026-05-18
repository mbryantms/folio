# OPDS progress-write protocol (`progress-write-v1`)

Folio extends the OPDS catalog with a per-issue progress-write endpoint
so OPDS clients (Panels, Chunky, KOReader, custom integrations) can sync
reading position back to the server. This page documents the wire
format, discovery rel, and migration notes for new clients.

Profile URI: `https://folio.bryhome.live/spec/progress-write-v1`

## Discovery

Both `/opds/v1` (Atom) and `/opds/v2` (OPDS 2.0 JSON) root catalogs
carry a sync-advertisement link:

```xml
<!-- /opds/v1 -->
<link rel="http://opds-spec.org/sync"
      href="/opds/v1/issues/{issue_id}/progress"
      type="application/json"/>
```

```json
// /opds/v2 — root links[] entry
{
  "rel": "http://opds-spec.org/sync",
  "href": "/opds/v1/issues/{issue_id}/progress",
  "type": "application/json",
  "templated": true,
  "profile": "https://folio.bryhome.live/spec/progress-write-v1"
}
```

The rel is **not** a canonical OPDS spec value — there's no
ecosystem-wide consensus on a sync rel yet, so `http://opds-spec.org/sync`
is a Folio-namespaced placeholder. Clients should match on the
`profile` URL (or feature-detect the request shape) and switch to a
spec-blessed rel when one emerges.

The href is a URI template; clients substitute `{issue_id}` with the
target issue's id (BLAKE3 hex). The same endpoint serves both
`/opds/v1` and `/opds/v2` clients — there is no `/opds/v2/.../progress`
mirror; both protocol surfaces converge on the v1 path.

## Authentication

`PUT /opds/v1/issues/{issue_id}/progress` requires one of:

- **Cookie session** (interactive — same auth the web app uses)
- **Bearer app password** (`Authorization: Bearer app_…`) with scope
  `read+progress`. The default-scope `read` token is rejected with
  `403 Forbidden`. See [auth-hardening M7](../../README.md) for how to
  mint scoped tokens.
- **HTTP Basic** carrying an app password (`Authorization: Basic <b64(user:app_…)>`).
  Same scope rule applies. JWT-via-Basic is rejected as a footgun guard.

`PUT` is CSRF-exempt when accompanied by Basic auth with an app password
(M7 of opds-readiness widened the exempt list). Cookie callers must
include the standard `X-CSRF-Token` header.

## Request body

```json
{
  "page": 14,
  "position": 0.4375,
  "finished": false,
  "device": "Chunky/iPad"
}
```

| Field      | Type           | Required | Notes |
|------------|----------------|----------|-------|
| `page`     | `int`          | one of `page` / `position` | 0-based page index. Precise source-of-truth when both are present. |
| `position` | `float [0,1]`  | one of `page` / `position` | Readium-style fractional progression. Server converts via `round(position * page_count)` clamped to `[0, page_count - 1]`. Requires the issue to have a known `page_count`. |
| `finished` | `bool`         | no       | When omitted, the server preserves the previous `finished` flag (sticky). Send explicit `true` on last-page-auto-finish, explicit `false` to mark-as-unread. |
| `device`   | `string`       | no       | Free-form client identifier echoed back via the `device` column; useful for tracing multi-device reads. |

### Precedence rules

- `page` and `position` are both optional individually but at least one
  must be present, else 400 `validation`.
- When both are present, `page` wins. `position` is purely a convenience
  for clients that don't track integer page counts (Readium-derived
  toolchains often think in fractions).
- `position` requires the issue to have `page_count > 0`. Without a
  known page count, the conversion is undefined — the server returns
  400 `validation` with message
  `"position requires the issue to have a known page_count"`.
- `position` is clamped to `[0.0, 1.0]` before conversion. Non-finite
  values (NaN, ±∞) are rejected with 400 `validation`.

### `finished` semantics

`finished` is sticky on per-page writes: omitting it preserves whatever
the previous row had. This matches the web reader's auto-finish rule —
when the reader reaches the last page, it sends `{page: N-1,
finished: true}`; mid-issue bookmark deep-links send only `page` and
don't accidentally clear a previously-finished state.

To explicitly clear: send `"finished": false`. To explicitly set: send
`"finished": true`.

## Response body

```json
{
  "issue_id": "9f1e…",
  "page": 14,
  "position": 0.4375,
  "percent": 0.4375,
  "finished": false,
  "updated_at": "2026-05-15T18:42:11+00:00"
}
```

`position` and `percent` carry the same value — they're aliases so
clients can pick whichever name matches their domain language. The
response always echoes the server-resolved row, not the request body —
so a client writing `page=10` to a row that already had `page=15` gets
back `page=15` (sticky-advance semantics live in the storage layer for
explicit writes too; M3's implicit-write monotonic guard sits in the
PSE caller).

## Implicit progress (PSE stream)

OPDS-PSE clients (Panels, Chunky, KOReader) that don't explicitly POST
progress still record it indirectly: every authenticated page-stream
hit at `GET /opds/pse/{issue_id}/{n}?<sig>` fires a fire-and-forget
upsert with:

- `page = n`
- `finished = true` when `n == page_count - 1`, else preserved
- `device = "opds-pse"`

The implicit-write guard is **monotonic-only** — a backwards-jump
(`n < current.last_page`) is treated as a buffered prefetch and
dropped. This protects KOReader-style readers that fetch a few pages
ahead from regressing the recorded position. See
[opds-sync-1.0 M3](../../README.md).

## Curl examples

Write explicit page progress:

```sh
curl -X PUT \
  -H "Authorization: Bearer app_<token>" \
  -H "Content-Type: application/json" \
  -d '{"page": 14, "device": "Chunky/iPad"}' \
  https://folio.example/opds/v1/issues/9f1e…/progress
```

Write Readium-style fractional progress:

```sh
curl -X PUT \
  -H "Authorization: Bearer app_<token>" \
  -H "Content-Type: application/json" \
  -d '{"position": 0.4375}' \
  https://folio.example/opds/v1/issues/9f1e…/progress
```

Mark-as-read:

```sh
curl -X PUT \
  -H "Authorization: Bearer app_<token>" \
  -H "Content-Type: application/json" \
  -d '{"page": 31, "finished": true}' \
  https://folio.example/opds/v1/issues/9f1e…/progress
```

## Multi-device conflict resolution

Concurrent writes from different devices are accepted as they arrive;
each write fully replaces the row. There is **no** automatic
`max(last_page)` merge on explicit writes — the most recent write wins,
including backwards jumps (a user re-reading from page 1 on their phone
overrides last week's page-50 record from their tablet). This is
intentional: bookmark deep-links and mark-as-unread are legitimate
backwards moves.

If you need automatic max-only semantics, use the PSE stream surface
instead (M3 — implicit progress with the monotonic guard).

## Audit

Every successful explicit write lands one `opds.progress.write` audit
row carrying `actor_id`, `target_id` (issue id), `payload = {page,
finished}`, plus the request's IP and user-agent. Implicit PSE writes
land one `opds.pse.access` row on the first-page hit; per-page writes
themselves are not audited (would saturate the log on a long read).

## Compatibility

Pre-M4 clients that send `{"page": N, ...}` continue to work unchanged
— `page` was renamed from `i32` to `Option<i32>` on the request DTO,
which serde's `#[serde(default)]` accepts back-compat as either
"missing" or "value". Only the new `position`-only payload requires
M4-era server code.
