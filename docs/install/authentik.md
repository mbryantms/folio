# Authentik integration

For homelabbers using Authentik 2025.10+. See [comic-reader-spec.md §12.7](../../comic-reader-spec.md).

## Application setup

1. **Authentik → Applications → Create**
   - Name: `Comic Reader`
   - Slug: `comics`
   - Provider: create new (next step)

2. **Authentik → Providers → Create OAuth2/OpenID Provider**
   - Name: `Comic Reader OIDC`
   - Authorization flow: `default-authorization-flow`
   - Client type: `Confidential`
   - Client ID: copy this — you'll set `COMIC_OIDC_CLIENT_ID` to it.
   - Client secret: copy this — you'll set `COMIC_OIDC_CLIENT_SECRET` to it (or use the `_FILE` variant with Docker secrets).
   - Redirect URIs (one per line):
     ```
     https://comics.example.com/auth/oidc/callback
     ```
   - Signing key: `authentik Self-signed Certificate`

3. **Scope mapping** — the important step. Authentik 2025.10 defaults can omit `email_verified`, which Comic Reader treats as `false` (§12.7).

   - Authentik → Customisation → Property Mappings → Create → Scope Mapping
   - Name: `comic-email-verified`
   - Scope name: `email`
   - Expression:
     ```python
     return {
         "email_verified": True if request.user.is_active else False,
     }
     ```
   - Attach this mapping to the OIDC provider's "Scopes" list.

   This makes `email_verified: true` appear in the userinfo claims for any active Authentik user.

   **Alternative (not recommended):** set `COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true`. The app will log a `warn` on every startup. Only safe if Authentik self-service signup is disabled.

4. **Bind to a group** — Authentik → Applications → Comic Reader → Policy/Group/User Bindings. Add the group(s) of users who should have access. Users not in those groups get a clean Authentik denial, never reaching Comic Reader.

## Comic Reader env

```env
COMIC_AUTH_MODE=oidc
COMIC_OIDC_ISSUER=https://auth.example.com/application/o/comics/
COMIC_OIDC_CLIENT_ID=<from step 2>
COMIC_OIDC_CLIENT_SECRET=<from step 2>
COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=false
```

## First-login admin bootstrap (§12.8)

The first user to authenticate becomes admin automatically. Make sure that user is *you*:

1. Bind the Authentik application only to your own user account first.
2. Start Comic Reader.
3. Log in. Verify `users.role = 'admin'` in Postgres or via `/auth/me`.
4. *Then* expand the Authentik application binding to other groups.

## Troubleshooting

| Symptom | Fix |
|---|---|
| Login redirects loop | Check `COMIC_PUBLIC_URL` matches the URL Authentik redirects back to (scheme + host + path). |
| `account.unverified` after Authentik login | Authentik isn't sending `email_verified`. Apply the scope mapping above, or set the trust flag (with caveats). |
| 403 after successful Authentik login | The Authentik application isn't bound to your user/group. |
| "JWKS fetch failed" in logs | Comic Reader can't reach `${OIDC_ISSUER}/.well-known/jwks.json`. Network/DNS issue between the containers. |
