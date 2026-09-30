import { THEME_COLORS } from "@/lib/pwa/theme-colors";
import type { MetadataRoute } from "next";

/**
 * Web App Manifest. Drives the install experience on every platform
 * that honours the manifest (Android Chrome, Edge, desktop Chrome /
 * Firefox / Edge). On iOS the manifest is partially honoured —
 * `display: standalone` and the manifest icons are respected from
 * iOS 16.4+, but legacy iOS still relies on the `apple-mobile-web-
 * app-*` meta tags emitted from `layout.tsx`.
 *
 * Theme + background colors mirror the dark `--background` token
 * from `web/styles/globals.css` (HSL 222 22% 6%), which is the
 * canonical theme; if a user has a light or amber theme preference
 * the splash will briefly flash dark before the in-app theme cookie
 * is applied. That trade-off is intentional — dark is the
 * canonical app theme per the comment at the top of globals.css.
 *
 * Icons live under `web/public/icons/` and are generated from the SVG
 * masters in `web/public/brand/` by `pnpm --filter web run build-icons`
 * (see `web/public/icons/README.md`). `any` icons carry the master's own
 * rounded tile; the `maskable` icon is full-bleed with the glyph inside
 * the 80% safe zone, so the two purposes are deliberately separate
 * files rather than one `"any maskable"` entry. `tests/pwa/assets.test.ts`
 * asserts every referenced file exists at its declared size.
 */
export default function manifest(): MetadataRoute.Manifest {
  return {
    id: "/",
    name: "Folio",
    short_name: "Folio",
    description: "Self-hostable comic reader",
    start_url: "/",
    scope: "/",
    display: "standalone",
    // `any` rather than locking portrait — the reader is meaningfully
    // better in landscape on tablets, and the library grid uses the
    // extra width well.
    orientation: "any",
    background_color: THEME_COLORS.dark,
    theme_color: THEME_COLORS.dark,
    icons: [
      {
        src: "/icons/icon-192.png",
        sizes: "192x192",
        type: "image/png",
        purpose: "any",
      },
      {
        src: "/icons/icon-512.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "any",
      },
      {
        src: "/icons/icon-512-maskable.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "maskable",
      },
    ],
    shortcuts: [
      {
        name: "Library",
        url: "/?library=all",
        icons: [
          {
            src: "/icons/shortcut-library-96.png",
            sizes: "96x96",
            type: "image/png",
          },
        ],
      },
      {
        name: "Bookmarks",
        url: "/bookmarks",
        icons: [
          {
            src: "/icons/shortcut-bookmarks-96.png",
            sizes: "96x96",
            type: "image/png",
          },
        ],
      },
    ],
    categories: ["books", "entertainment"],
  };
}
