# Brand masters (INTERIM)

**These are interim placeholder marks, not the final Folio brand.** They
exist so PWA install prompts, Home Screen icons, shortcut menus, favicons,
and iOS splash screens render a real, consistent mark instead of browser
placeholders while the production brand is pending.

| File                     | Used for                                                                           |
| ------------------------ | ---------------------------------------------------------------------------------- |
| `icon-master.svg`        | App icon master: every manifest/Apple/favicon/splash raster, and the lockup glyph. |
| `logotype-master.svg`    | The name "Folio" as outlines: the lettering of the title-bar lockup.               |
| `shortcut-library.svg`   | Manifest shortcut icon for "Library".                                              |
| `shortcut-bookmarks.svg` | Manifest shortcut icon for "Bookmarks".                                            |

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

## The title-bar lockup

The shells' title bars (`MainShell`, `AdminShell`, `StatusScreen`) show
one lockup, `web/components/shell/Wordmark.tsx`: the book glyph beside
the name. Both halves are inline SVG, so the lockup needs no font and
looks the same on every device.

- **Glyph**: the five paths of `icon-master.svg` without the tile,
  `viewBox="100 126 312 272"`, drawn 27 × 24 px in the theme's
  `--primary`, so it follows the account's accent.
- **Lettering**: `logotype-master.svg`, `viewBox="0 0 270 100"`, drawn
  43.2 × 16 px (a 16 px cap height) in the text colour. Geometric
  letters with 16-unit strokes; the dot of the "i" takes `--primary`.
- **Gap**: 9 px between the two.

The component copies both drawings; `web/tests/shell/wordmark.test.tsx`
fails when it and the masters disagree. To change the lettering or the
glyph, edit the master and the component together.
