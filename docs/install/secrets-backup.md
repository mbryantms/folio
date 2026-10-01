# Server-side secrets

On first boot the Rust server auto-generates four cryptographic secrets
inside `${COMIC_DATA_PATH}/secrets/`. They are loaded on every
subsequent boot — never regenerated unless deleted. They are not in the
database; they live on the filesystem so an operator can rotate them
without a schema migration.

| File | Used for | Bytes | What breaks if you lose it |
|---|---|---|---|
| `pepper` | argon2id password-hashing pepper | 32 | Every existing password hash becomes unverifiable. Every local user must reset their password via email. |
| `pepper.previous` | *Optional.* The pre-rotation pepper, present only during a [pepper rotation](#rotating-the-pepper); never auto-generated | 32 | Accounts not yet rehashed onto the current pepper stop verifying (same as finishing the rotation early). |
| `jwt-ed25519.key` | Ed25519 signing key for access tokens | 32 raw | Every issued access JWT is rejected. Browsers refresh on next request (refresh tokens still valid) so the impact is one round-trip per session. |
| `email-token.key` | HMAC-SHA256 for verify-email + password-reset tokens | 32 | Every outstanding verification link + password-reset link 400s. Users must request new ones. |
| `url-signing.key` | HMAC-SHA256 for signed OPDS Page-Streaming URLs | 32 | Every cached signed URL (OPDS reader history, push notifications, e-reader bookmarks) returns 403. Folio regenerates them silently on the next OPDS hit. |

File permissions are enforced by the server: directory mode `0700`,
files mode `0600`, owned by the container's nonroot UID. The server
refuses to load a secret with the wrong length (mismatched file size is
fatal at startup).

## Why secrets matter for backups

The whole `${COMIC_DATA_PATH}` volume is in your weekly backup
([`backup.md`](./backup.md)), so secrets are covered if you follow the
default cadence. The reason to call them out is the **failure mode** is
silent: if you restore Postgres without restoring the secrets dir,
every user will appear to have a "wrong password" because the pepper
the hashes were computed against is different from the one verifying
them now. The fix is to restore the matching secrets, not to reset
passwords blindly.

## Rotating a secret

Routine rotation isn't necessary — these are filesystem-local secrets
with no exposure surface. But after a suspected compromise of the data
volume:

```bash
cd /opt/folio
docker compose -f compose.prod.yml down app

# Pull the named volume into a host path you can edit.
VOLUME_PATH=$(docker volume inspect folio_comic_data --format '{{.Mountpoint}}')
sudo rm "$VOLUME_PATH/secrets/jwt-ed25519.key"
# Optionally rotate others; deleting any subset is safe.

docker compose -f compose.prod.yml up -d app
```

The server logs `secrets.generate <name>` for each freshly created
secret at INFO level. Consequences of each rotation:

- `jwt-ed25519.key` rotation: all access JWTs invalidated. Users
  experience one extra round-trip; refresh tokens still work.
- `email-token.key` rotation: outstanding verify-email + password-reset
  links 400. New ones work.
- `url-signing.key` rotation: cached OPDS signed URLs 403. Readers
  re-fetch the feed and pick up new URLs.
- `pepper`: don't just delete it — the server refuses to boot when a
  fresh pepper would orphan existing password hashes. Use the
  rotation procedure below, which keeps existing passwords working.

## Rotating the pepper

The argon2 pepper is mixed into every password and app-password hash, so
replacing it outright would invalidate all of them. Folio supports a
**dual-pepper rotation** instead (security audit L-1): the old pepper is
kept as `secrets/pepper.previous`, every verify tries the current pepper
first and then the previous one, and a hash that only verifies against the
previous pepper is **rehashed under the current one on the spot** (on a
successful sign-in, or the next time an app password is used).

```bash
cd /opt/folio
docker compose -f compose.prod.yml down app
VOLUME_PATH=$(docker volume inspect folio_comic_data --format '{{.Mountpoint}}')

# 1. Move the current pepper aside. Keep mode 0600 — the server refuses a
#    group/world-readable or symlinked secret file.
sudo mv "$VOLUME_PATH/secrets/pepper" "$VOLUME_PATH/secrets/pepper.previous"

# 2. Either let the server mint the new pepper on boot, or pre-populate it
#    from your vault (32 raw bytes, mode 0600, owned by 65532):
#    your-vault-tool fetch folio/pepper-v2 > "$VOLUME_PATH/secrets/pepper"

docker compose -f compose.prod.yml up -d app
```

On boot the server logs a `WARN` that a pepper rotation is in progress.
Each rehash logs `password rehashed onto the current pepper` (or the
app-password equivalent) and increments
`folio_auth_pepper_rehash_total{kind="user"|"app_password"}`.

**Ending the rotation.** Once the window is over — typically a few weeks,
long enough for active users and OPDS readers to sign in or sync —
delete `secrets/pepper.previous` and restart. Any account whose hash was
never rehashed stops verifying at that point: those users go through
`/forgot-password` (or an admin issues a temporary password), and
app passwords that were never used during the window must be re-issued.

Notes:

- While `pepper.previous` is present, a wrong password costs two argon2
  verifies instead of one; the missing-user dummy path does the same so
  sign-in timing still doesn't reveal whether an account exists.
- `pepper.previous` is never generated by the server and is validated
  exactly like the other secrets (32 bytes, regular file, mode `0600`).
- Back up `pepper.previous` alongside `pepper` for the duration of the
  rotation — restoring a database snapshot taken before the rotation
  needs it.

## Pre-populating with operator-owned secrets

If you already manage secrets in a vault (HashiCorp Vault, sops, SSM
Parameter Store), pre-populate the directory before first boot:

```bash
VOLUME_PATH=$(docker volume create folio_comic_data && \
  docker volume inspect folio_comic_data --format '{{.Mountpoint}}')
sudo mkdir -p "$VOLUME_PATH/secrets"
sudo chmod 700 "$VOLUME_PATH/secrets"

# Each value is raw bytes (no base64, no PEM); use `head -c 32 /dev/urandom`
# or fetch from your vault. The server validates length on read.
your-vault-tool fetch folio/pepper           > "$VOLUME_PATH/secrets/pepper"
your-vault-tool fetch folio/jwt-ed25519.key  > "$VOLUME_PATH/secrets/jwt-ed25519.key"
your-vault-tool fetch folio/email-token.key  > "$VOLUME_PATH/secrets/email-token.key"
your-vault-tool fetch folio/url-signing.key  > "$VOLUME_PATH/secrets/url-signing.key"

sudo chmod 600 "$VOLUME_PATH/secrets"/*
sudo chown -R 65532:65532 "$VOLUME_PATH/secrets"  # distroless nonroot UID
```

On first boot the server will find your values, validate the lengths,
and skip the auto-generation step.

## What secrets are NOT here

- **OIDC client secret** — set via `COMIC_OIDC_CLIENT_SECRET` env var.
- **SMTP password** — set via `COMIC_SMTP_PASSWORD` env var.
- **Postgres password** — set via `POSTGRES_PASSWORD` in `.env` (consumed
  by compose to initialize the postgres container).

These are operator-owned configuration, not server-generated runtime
keys. They're in your `.env` file (or wherever you load env from).
