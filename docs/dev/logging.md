# Logging conventions

Folio's server uses `tracing` for structured logging with a JSON stdout sink and
an in-process ring buffer that backs the admin `/admin/logs` page. This doc
covers what to log, how, and what *never* to log.

> For how the ring buffer fits into the two-stream admin observability model
> (Server stream vs Library stream, the durable `library_events` manifest, and
> the `domain` / `error_code` classification on log entries), see
> [observability.md](observability.md).

## The `#[handler]` macro

Every axum handler in `crates/server/src/api/` is annotated with `#[handler]`
from the `server_macros` crate. The macro expands to a
`#[tracing::instrument]` span carrying a few default fields:

```rust
use server_macros::handler;

#[utoipa::path(get, path = "/admin/users", ...)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListUsersQuery>,
) -> impl IntoResponse { ... }
```

This expands to:

```rust
#[tracing::instrument(skip_all, name = "list", fields(user_id = %_admin.0.id))]
pub async fn list(...) -> impl IntoResponse { ... }
```

The macro detects the following extractor types and auto-populates the
`user_id` span field:

| Pattern                              | Span field          |
|--------------------------------------|---------------------|
| `user: CurrentUser`                  | `user_id = %user.id`     |
| `_user: CurrentUser`                 | `user_id = %_user.id`    |
| `RequireAdmin(actor): RequireAdmin`  | `user_id = %actor.id`    |
| `admin: RequireAdmin`                | `user_id = %admin.0.id`  |

Handlers that don't carry user identity (health probes, OPDS bytes, etc.)
get the named span without a `user_id` field — that's fine.

**Convention:** add `#[handler]` to every new `pub async fn` in
`crates/server/src/api/`. The lint in M10 will flag misses.

### Extending the span with more fields

For request-specific fields like `library_id` / `series_id` / `issue_id`,
record them inside the handler body after the path extractor has run:

```rust
#[handler]
pub async fn delete(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(library_slug): AxPath<String>,
) -> Response {
    tracing::Span::current().record("library", &tracing::field::display(&library_slug));
    // ...
}
```

Pre-declare the field in the attribute if you want it to render as a
structured field rather than appearing under the `message` body — `fields()`
inside the `instrument` attribute reserves the slot. The `#[handler]` macro
doesn't currently support adding extra fields to the seeded `fields(...)`
list; the manual `Span::current().record(...)` pattern is the escape hatch.

## What never to log

The risk is leaking these into:
- The JSON stdout sink (captured by container runtimes / log shippers).
- The in-memory ring buffer (visible to any admin via `/admin/logs`).
- Crash reports / structured-error responses sent to clients.

**Forbidden logging targets:**

1. **Raw passwords or password fields.** Bcrypt/argon2 hashes are also
   forbidden — they're brute-forceable offline.
2. **Bearer tokens, refresh tokens, app-password values.** Hash with
   `crate::auth::cookies::sha256_hex(value)[..12]` if you need a stable
   identifier for log correlation.
3. **OAuth `code` / `state` / `access_token` / `id_token` values.** These
   show up in URL query strings if you log the full request URL.
4. **OIDC `client_secret` or `smtp_password`.** Even error responses from
   misconfigured providers may echo them back; sanitize.
5. **Plain email addresses.** Hash to `email_hash = %sha256_hex(email)[..12]`.
   (Auth logs use this pattern at `crates/server/src/auth/oidc.rs`.)
6. **Full request bodies on mutation paths.** A `PATCH /me/preferences`
   that ships `{"password": "..."}` would otherwise hit the ring buffer.
   The default tower-http `TraceLayer` doesn't log bodies, and we don't
   override that — but don't add per-handler body logging without thinking
   about which fields you're echoing.

## The `sanitize_error` helper

For third-party error types that may carry secret-shaped substrings in
their `Display` impl (the openidconnect `RequestTokenError` is the
canonical case), wrap with `crate::observability::sanitize_error`:

```rust
tracing::warn!(
    error = %crate::observability::sanitize_error(&e),
    "oidc token exchange failed"
);
```

The sanitizer strips:
- URL query strings (`?code=...&state=...` → `?<redacted>`).
- `password=` / `token=` / `secret=` / `authorization=` k/v pairs.
- `Bearer <token>` / `Basic <token>` header values.

It's a heuristic, not a guarantee. Prefer logging the error *type* or
*variant* over its full Display when the error wraps a network response
body. Run `cargo test -p server --lib observability::` for the unit-test
coverage anchoring these substitutions.

## Severity levels

| Level   | When to use                                                   |
|---------|---------------------------------------------------------------|
| `error` | A failure the user can't recover from automatically. DB write rejected, queue job permanently failed, panic from a worker. |
| `warn`  | Recoverable degradation. Discovery cache miss, retry after backoff, lockout triggered, recovery from a malformed ZIP. |
| `info`  | Lifecycle events. Server start/stop, scan begun, settings reload. Default operator level. |
| `debug` | Per-request internals. Cursor decode, ACL check shortcut, OCR worker pick. Off by default. |
| `trace` | Hot-path details. Per-row SQL query log, per-page thumb generation. Never the default. |

The runtime log level is controlled by `observability.log_level` in
[runtime-configuration.md](runtime-configuration.md) (DB-backed,
hot-swappable via `PATCH /admin/settings`). Test deployments use `info`;
operator-debugging sessions can temporarily flip to `debug`.

## Span field naming

Stable names so dashboards / grep stay sane:
- `user_id` — UUID string (set by `#[handler]`).
- `library_id` / `library` — UUID or slug.
- `series_id` / `series` — UUID or slug.
- `issue_id` / `issue` — BLAKE3 hex (the issue id is content-derived).
- `cbl_list_id` — UUID.
- `route` — bare path, e.g. `/admin/users/{id}`.
- `count` — integer (rows affected, items returned).
- `latency_ms` — float milliseconds.
- `error` — sanitized Display (use `sanitize_error` for unknown types).
- `error_kind` — discriminated variant name when the error type is known
  and we want grep-able categorisation rather than freeform Display.

## Where the logs go

1. **stdout (JSON)** — written by the `tracing-subscriber` `fmt::layer().json()`
   in `crate::observability::init`. Format: one JSON object per line; fields
   include `timestamp`, `level`, `target`, `message`, plus span fields.
2. **In-process ring buffer** — same events get copied via the custom
   `RingLayer` into a [`LogRingBuffer`](../../crates/server/src/observability.rs)
   capped at 5,000 entries. Accessed via `GET /admin/logs`. Triage scale —
   ship to Loki / Cloudwatch / etc. if you need longer retention.
3. **OTLP** — wired but intentionally not shipped in v1
   (see [incompleteness-audit.md](incompleteness-audit.md) §D-9).
