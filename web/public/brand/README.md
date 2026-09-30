# Brand masters (INTERIM)

**These are interim placeholder marks, not the final Folio brand.** They
exist so PWA install prompts, Home Screen icons, shortcut menus, favicons,
and iOS splash screens render a real, consistent mark instead of browser
placeholders while the production brand is pending.

| File                     | Used for                                                     |
| ------------------------ | ------------------------------------------------------------ |
| `icon-master.svg`        | App icon master: every manifest/Apple/favicon/splash raster. |
| `shortcut-library.svg`   | Manifest shortcut icon for "Library".                        |
| `shortcut-bookmarks.svg` | Manifest shortcut icon for "Bookmarks".                      |

The interim mark is an open book whose pages are split into comic panels,
in the dark theme's amber `--primary` (`#f6a823`, `hsl(38 92% 55%)`) on
the dark `--background` (`#0c0e13`). It uses no text and no third-party
marks.

## Replacing with the real brand

1. Replace `icon-master.svg` with the final mark (512 × 512 viewBox, full
   square). The master is rendered as-is for `any` icons, so it should
   carry its own tile/rounding. For the maskable icon, Apple touch icon,
   and splash screens it is centred on an opaque `#0c0e13` canvas; keep the
   glyph within a circle of radius 230 px around the centre so that, at
   the build script's 0.88 maskable scale, it stays inside the 80 % safe
   zone (radius 204.8 px). The interim glyph reaches about 200 px.
2. Optionally replace the two shortcut glyphs (96 × 96 viewBox).
3. Run `pnpm --filter web run build-icons` and commit the regenerated files
   under `public/icons/` plus `public/favicon.ico` and `public/icon.svg`.
4. `pnpm --filter web test tests/pwa/assets.test.ts` checks every declared
   file exists at its declared size; then delete the "INTERIM" wording here.

The planned `logotype-master.svg` (wordmark) is not referenced by the app
yet and is not part of this interim set.
