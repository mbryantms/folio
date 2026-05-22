# PWA icons

This directory holds the icons referenced from
[`web/app/manifest.ts`](../../app/manifest.ts) and from the
`appleWebApp.icons` field in [`web/app/layout.tsx`](../../app/layout.tsx).
None of the PNG files are checked into the repo yet because Folio does
not have a finalised brand mark; the manifest still emits with the
references in place, and the install UX degrades gracefully (browsers
fall back to a generated tile or a screenshot of the page) until the
files land.

## Required files

| File                       | Size      | Purpose                                                                                                   |
| -------------------------- | --------- | --------------------------------------------------------------------------------------------------------- |
| `icon-192.png`             | 192 × 192 | Web App Manifest `any` icon. Used by Android Chrome / desktop browsers for the install tile.              |
| `icon-512.png`             | 512 × 512 | Web App Manifest `any` icon at the larger size that Play Store / TWA wrappers want.                       |
| `icon-512-maskable.png`    | 512 × 512 | Maskable icon. Android adaptive icons crop to a circle / squircle; the safe zone is the central 80 % of the canvas. |
| `apple-touch-icon.png`     | 180 × 180 | iOS Home Screen icon. Without this, iOS scrapes a screenshot of the page (usually ugly).                  |

A favicon is not strictly required for PWA install but is a sensible
companion file:

| File           | Size           | Notes                                                                  |
| -------------- | -------------- | ---------------------------------------------------------------------- |
| `favicon.ico`  | 16 / 32 / 48   | Browser tab icon. Multi-resolution `.ico` so old browsers stay happy.  |
| `icon.svg`     | scalable       | Modern browsers prefer the SVG favicon for crisp rendering at any DPI. |

The favicon files belong in the parent `web/public/` directory rather
than `web/public/icons/`, because Next.js auto-discovers `favicon.ico`
and `icon.svg` at the root of `public/`.

## Generating

Once a brand mark exists as a single high-resolution source (SVG ideally,
or a 1024 × 1024 PNG), `pwa-asset-generator` produces every size at once:

```sh
npx pwa-asset-generator <source.svg> ./web/public/icons \
  --icon-only \
  --background "#0c1012" \
  --padding "10%" \
  --type png \
  --opaque false
```

The `--background "#0c1012"` matches the manifest's `theme_color` and
`background_color`, both of which are derived from the dark
`--background` token in `web/styles/globals.css`. The `--padding "10%"`
inset keeps the mark inside the 80 % safe zone required for the
maskable variant.

For the maskable variant specifically, run the generator a second time
with `--maskable true` so the central safe zone is enforced:

```sh
npx pwa-asset-generator <source.svg> ./web/public/icons \
  --icon-only \
  --maskable true \
  --background "#0c1012" \
  --padding "20%" \
  --type png
```

Then rename the maskable output to `icon-512-maskable.png` to match
the manifest's reference.

## iOS splash screens

A second tier of PWA polish — proper splash screens during the
launch-from-Home-Screen sequence on iOS — needs an additional set of
per-device-size PNGs declared via `<link rel="apple-touch-startup-image">`
tags. Apple requires the dimensions to match each device's screen
exactly, which means a dozen or so files. `pwa-asset-generator` can
emit those too:

```sh
npx pwa-asset-generator <source.svg> ./web/public/icons \
  --splash-only \
  --background "#0c1012" \
  --padding "30%" \
  --type png
```

Wiring the resulting `<link>` tags is not yet done in
[`web/app/layout.tsx`](../../app/layout.tsx); see the Tier 2 list in
the PWA hardening notes.
