# PWA icons

Every file in this directory (plus `../favicon.ico` and `../icon.svg`) is
**generated** from the SVG masters in [`../brand/`](../brand/README.md) by
[`scripts/build-icons.mjs`](../../scripts/build-icons.mjs). Do not edit the
PNGs by hand; change a master and regenerate:

```sh
pnpm --filter web run build-icons
# or with a different master:
pnpm --filter web run build-icons path/to/icon-master.svg
```

The masters are currently **interim** (see `../brand/README.md`).

## Generated files

| File                            | Size          | Referenced from                                   |
| ------------------------------- | ------------- | ------------------------------------------------- |
| `icon-192.png`, `icon-512.png`  | 192², 512²    | `app/manifest.ts`, `purpose: "any"`               |
| `icon-512-maskable.png`         | 512²          | `app/manifest.ts`, `purpose: "maskable"` (opaque) |
| `shortcut-library-96.png`       | 96²           | manifest shortcut "Library"                       |
| `shortcut-bookmarks-96.png`     | 96²           | manifest shortcut "Bookmarks"                     |
| `apple-touch-icon.png`          | 180² (opaque) | `app/layout.tsx` `icons.apple`                    |
| `splash-<w>x<h>.png` (18 files) | device pixels | `app/layout.tsx` via `lib/pwa/apple-splash.ts`    |
| `../favicon.ico`                | 16 / 32 / 48  | `app/layout.tsx` `icons.icon`                     |
| `../icon.svg`                   | scalable      | `app/layout.tsx` `icons.icon` (copy of master)    |

`any` and `maskable` are separate files on purpose: the `any` icon keeps
the master's rounded tile and transparent corners, while the maskable one
is full-bleed and opaque with the glyph inside the 80 % safe zone.

The iOS startup-image device list lives in
[`lib/pwa/apple-splash-devices.json`](../../lib/pwa/apple-splash-devices.json),
shared by the build script and the layout. Adding a device there and
rerunning the script produces both its portrait and landscape files and
the matching `<link rel="apple-touch-startup-image">` tags.

## Checks

- `tests/pwa/assets.test.ts` (vitest): every manifest, shortcut, Apple, and
  startup image is a committed PNG at its declared size, maskable/Apple
  icons are opaque, and file sizes stay within budget.
- `scripts/check-pwa-assets.mjs` and `tests/e2e/pwa.spec.ts`: the same
  assets served by the booted public origin (docker-smoke CI).
