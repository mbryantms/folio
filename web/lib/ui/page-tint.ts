"use client";

import { useSyncExternalStore } from "react";

/**
 * Page tint reach (folio-flair-1.0 exploration).
 *
 * A page with a tinted hero (series / issue, `HeroBackdrop`) publishes its
 * cover colours on `<html>`: `--page-tint` (most vibrant), `--page-tint-2`
 * (average), `--page-cover` (the cover image) and `data-page-tint`. How far
 * the tint reaches beyond the page is `data-tint-reach` on `<html>`, a
 * space-separated set read by globals.css ("Page tint reach"):
 *
 *   ""            the page only (the original backdrop)
 *   "top"         the top bar carries the cover glow too
 *   "top side"    top bar + a fade down the sidebar
 *   "accent"      the app accent (Read button, nav pill, focus) takes the
 *                 cover's colour; chrome stays neutral
 *   "top accent"  top bar glow + cover accent
 *
 * Under test: chosen per browser (localStorage), not an account setting.
 */
export const TINT_REACH_OPTIONS = [
  { value: "", label: "Page only" },
  { value: "top", label: "+ Top bar" },
  { value: "top side", label: "+ Top bar & sidebar" },
  { value: "accent", label: "Accent colour" },
  { value: "top accent", label: "Top bar + accent" },
] as const;
export type TintReach = (typeof TINT_REACH_OPTIONS)[number]["value"];

const KEY = "folio:tint-reach";
const listeners = new Set<() => void>();

function read(): TintReach {
  try {
    const v = localStorage.getItem(KEY) ?? "";
    return TINT_REACH_OPTIONS.some((o) => o.value === v)
      ? (v as TintReach)
      : "";
  } catch {
    return "";
  }
}

function apply(v: TintReach) {
  const root = document.documentElement;
  if (v) root.setAttribute("data-tint-reach", v);
  else root.removeAttribute("data-tint-reach");
}

if (typeof window !== "undefined") apply(read());

export function setTintReach(v: TintReach) {
  try {
    localStorage.setItem(KEY, v);
  } catch {
    // Per-tab only, then.
  }
  apply(v);
  listeners.forEach((l) => l());
}

export function useTintReach(): TintReach {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    read,
    () => "",
  );
}

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
