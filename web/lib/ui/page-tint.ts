"use client";

/**
 * Page tint (folio-flair-1.0).
 *
 * A page with a tinted hero (series / issue, `HeroBackdrop`) publishes its
 * cover colours on `<html>`: `--page-tint` (most vibrant), `--page-tint-2`
 * (average), `--page-cover` (the cover image) and `data-page-tint`. Today
 * only the page backdrop uses them; they are there so other surfaces (for
 * example the accent colour) can pick the tint up later.
 */
let owner = 0;

/** Publish a page's cover colours to the shell; returns the cleanup. */
export function publishPageTint(t: {
  vibrant: string;
  average: string;
  src: string;
}): () => void {
  const root = document.documentElement;
  const me = ++owner;
  root.style.setProperty("--page-tint", t.vibrant);
  root.style.setProperty("--page-tint-2", t.average);
  root.style.setProperty(
    "--page-cover",
    `url("${t.src.replace(/"/g, "%22")}")`,
  );
  root.setAttribute("data-page-tint", "");
  return () => {
    // A newer page may already have published its own tint.
    if (owner !== me) return;
    root.removeAttribute("data-page-tint");
    root.style.removeProperty("--page-tint");
    root.style.removeProperty("--page-tint-2");
    root.style.removeProperty("--page-cover");
  };
}
