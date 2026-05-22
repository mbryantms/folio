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
 * Icons reference files under `web/public/icons/` that must exist
 * at runtime. See `web/public/icons/README.md` for the required
 * sizes and the generator recipe. The manifest will still emit if
 * the files are missing; the install UX will degrade until they
 * land.
 */
export default function manifest(): MetadataRoute.Manifest {
  return {
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
    background_color: "#0c1012",
    theme_color: "#0c1012",
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
    categories: ["books", "entertainment"],
  };
}
