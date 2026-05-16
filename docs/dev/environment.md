# Dev Environment

Cheat sheet. For background, see [§13 of the spec](../../comic-reader-spec.md).

## Tools

| Tool          | Why                              | Pin / install                       |
| ------------- | -------------------------------- | ----------------------------------- |
| Rust          | server                           | `rust-toolchain.toml` (1.91)        |
| Node          | web                              | `.nvmrc` (22+)                      |
| pnpm          | web                              | `package.json#packageManager` (10)  |
| just          | task runner                      | `cargo install just`                |
| cargo-watch   | server hot-reload under `just dev` | `cargo install cargo-watch`       |
| Docker        | dev services                     | any recent                          |

Without `cargo-watch`, `just dev` falls back to plain `cargo run` and you'll
need to Ctrl-C and re-run after every server-side change.

## What runs where

|                    | `just` (native)            | Docker                          |
| ------------------ | -------------------------- | ------------------------------- |
| Rust server        | ✅ hot reload via `cargo watch` | only in `compose.prod.yml` |
| Next.js web        | ✅ Turbopack HMR           | only in `compose.prod.yml`      |
| Postgres           | —                          | ✅ `compose.dev.yml` (host `:5432`) |
| Redis              | —                          | ✅ `compose.dev.yml` (host `:6380` — see Port note) |

**Rule:** native `just` for the things you're editing; Docker for stateful deps.

### Port note

Comic's dev redis is published on host port **6380**, not the usual 6379. This
sidesteps a system valkey/redis daemon that ships pre-enabled on Arch and some
other distros (`systemctl is-active valkey`). The container itself still listens
on 6379 internally; only the host mapping moves.

`COMIC_REDIS_URL=redis://localhost:6380` in `.env.example` reflects this.

Inside `compose.prod.yml` the app reaches redis over the docker network as
`redis:6379` — no port-mapping juggling there.

## One-time setup

```sh
just bootstrap         # installs deps, copies .env from .env.example
just dev-services-up   # starts postgres + redis (+ dex)
just migrate           # applies all migrations
```

Then register the first user via the web (becomes admin), or via:

```sh
curl -sS -X POST http://127.0.0.1:8080/auth/local/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"you@example.com","password":"some-12-char-password"}'
```

## Daily

```sh
just dev               # runs server + web in parallel; Ctrl-C kills both
# or:
just run-server        # server only
just run-web           # web only
```

Browser entry: **`http://localhost:8080`** (the Rust binary). Rust handles its own routes and reverse-proxies HTML / RSC / `/_next/*` (including HMR over WebSockets) to the Next dev server at `:3000` internally. The Next port is still bound to loopback by `pnpm dev`, so hitting `:3000` directly works for raw Next debugging — but it bypasses the Rust middleware stack (auth, CSRF, security headers, rate limits), so it's not the supported entry point.

### Access the web app via `localhost` (not your LAN IP)

Open `http://localhost:8080`, **not** `http://192.168.x.x:8080` or
`http://devbox.lan:8080`. Auth cookies are minted with the `Secure` flag and
the `__Host-` / `__Secure-` prefixes (per the spec's auth model). Browsers
only accept these cookies when the URL's origin is a **secure context** —
that means HTTPS, OR plain HTTP to `localhost` / `127.0.0.1`. Any other
hostname (including a LAN IP or `.local` name) silently drops the cookies
and you'll bounce back to the sign-in page after every login.

If you need to test from a different machine, the cleanest options are:

```sh
# A. SSH tunnel to the dev box (recommended). Only the Rust port needs
#    forwarding — Next isn't a separate browser entry point any more.
ssh -L 8080:localhost:8080 user@devbox
# Then open http://localhost:8080 in the local browser.

# B. Use a real cert + HTTPS (e.g. via mkcert + Caddy in front).
#    Reuse docs/install/caddy.md as a starting point.
```

## Fixtures

CBZ fixtures live in `fixtures/library/`. To wire them into a library + scan:

```sh
just seed-fixtures                       # uses default first@example.com creds
just seed-fixtures email='you@…' password='…'   # override creds
```

Idempotent: re-runnable. Reports created vs already-exists, then scan stats.

## Reset levers (least → most destructive)

```sh
rm -rf .dev-data/app/thumbs              # regenerate covers on next scan
rm -rf .dev-data/app/secrets             # rotate JWT key + pepper (invalidates sessions + app passwords)
just seed-fixtures                       # re-scan; fixes drift after editing CBZs
```

Database resets (require `just dev-services-up` to be running):

```sh
just migrate-fresh                       # drop & re-run all migrations (keeps services up; wipes data)
just dev-services-reset                  # nukes the postgres volume entirely
just dev-services-up && just migrate     # bring it back fresh
```

Full-nuke:

```sh
just dev-services-down                   # stop containers
rm -rf .dev-data/                        # postgres volume + secrets + thumbs
just dev-services-up && just migrate     # back to a virgin install
```

## New environment from a fresh clone

```sh
git clone <repo> && cd folio
just bootstrap
just dev-services-up
just migrate
just dev                 # in one terminal
# in another, after registering the admin:
just seed-fixtures
```

Open <http://localhost:8080>.

## Auth modes for dev

Default `.env` sets `COMIC_AUTH_MODE=both`, which requires real OIDC config. For local-only:

```sh
COMIC_AUTH_MODE=local just run-server
```

`COMIC_LOCAL_REGISTRATION_OPEN=true` (default in `.env.example`) lets you register the first admin from the sign-in page. Set to `false` after.

## Where things live

| Path                     | Contents                                        |
| ------------------------ | ----------------------------------------------- |
| `.dev-data/postgres/`    | Postgres data (Docker-managed; safe to delete with `just dev-services-reset`) |
| `.dev-data/app/secrets/` | JWT signing key, argon2 pepper, URL-signing key |
| `.dev-data/app/thumbs/`  | Generated WebP covers (`<issue-id>.webp`)       |
| `fixtures/library/`      | CBZ fixtures (committed)                        |
| `target/`                | Rust build artifacts                            |
| `web/.next/`             | Next.js build cache                             |

## Production

`compose.prod.yml` runs everything in containers (`app + postgres + redis`). Use it for the smoke test before publishing an image:

```sh
just docker-build
just docker-test                         # spin up prod stack against test fixtures
```

Don't use prod compose for daily dev — there's no hot reload, and rebuilding the image on every change is slow.
