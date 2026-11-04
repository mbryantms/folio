"use client";

import { useSyncExternalStore } from "react";

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

/**
 * Header style (exploration): `pills` (badges for every fact) or `text`
 * (facts as a text line, bare rating stars, source links as text; chips
 * only for people, badges only for warnings). Both variants are in the
 * page markup; `data-hero-style` on <html> shows one (globals.css).
 */
export const HERO_STYLE_OPTIONS = [
  { value: "pills", label: "Pills" },
  { value: "text", label: "Text" },
] as const;
export type HeroStyle = (typeof HERO_STYLE_OPTIONS)[number]["value"];

const HERO_KEY = "folio:hero-style";
const heroListeners = new Set<() => void>();

function readHero(): HeroStyle {
  try {
    return localStorage.getItem(HERO_KEY) === "text" ? "text" : "pills";
  } catch {
    return "pills";
  }
}

function applyHero(v: HeroStyle) {
  document.documentElement.setAttribute("data-hero-style", v);
}

if (typeof window !== "undefined") applyHero(readHero());

export function setHeroStyle(v: HeroStyle) {
  try {
    localStorage.setItem(HERO_KEY, v);
  } catch {
    // Per-tab only, then.
  }
  applyHero(v);
  heroListeners.forEach((l) => l());
}

export function useHeroStyle(): HeroStyle {
  return useSyncExternalStore(
    (l) => {
      heroListeners.add(l);
      return () => heroListeners.delete(l);
    },
    readHero,
    () => "pills",
  );
}
