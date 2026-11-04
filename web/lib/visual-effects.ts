"use client";

import { useSyncExternalStore } from "react";

import { useMe } from "@/lib/api/queries";
import type { MeView } from "@/lib/api/types";

/**
 * Optional visual effects (folio-flair-1.0). Each is a per-account switch
 * stored in `users.effects` (Settings → Theme → Effects). The server only
 * stores what the user changed; a missing key falls back to `default` here,
 * so defaults can change without a migration.
 *
 * `motion: true` marks effects that are animation, not just looks: they
 * switch off while reduced motion is in force (the Motion setting, or the
 * OS preference when Motion is "Follow system"), whatever the switch says.
 */
export type EffectPrefs = MeView["effects"];
export type VisualEffect = Extract<keyof EffectPrefs, string>;

export type EffectGroup = "library" | "series" | "reader";

export interface EffectDef {
  key: VisualEffect;
  group: EffectGroup;
  label: string;
  description: string;
  default: boolean;
  motion: boolean;
}

export const EFFECT_GROUPS: ReadonlyArray<{ id: EffectGroup; label: string }> =
  [
    { id: "library", label: "Library covers" },
    { id: "series", label: "Series pages" },
    { id: "reader", label: "Reader" },
  ];

export const EFFECTS: readonly EffectDef[] = [
  {
    key: "cover_tilt",
    group: "library",
    label: "Cover tilt and sheen",
    description:
      "Covers lean toward your pointer and catch a little light. Mouse and trackpad only.",
    default: false,
    motion: true,
  },
  {
    key: "page_scrub",
    group: "library",
    label: "Flip through pages on hover",
    description:
      "Moving across a cover shows a few of its pages. Mouse and trackpad only.",
    default: false,
    motion: false,
  },
  {
    key: "spine_stack",
    group: "library",
    label: "Series thickness",
    description:
      "Series cards show stacked page edges that grow with the number of issues.",
    default: true,
    motion: false,
  },
  {
    key: "hero_backdrop",
    group: "series",
    label: "Tinted backdrop",
    description:
      "The top of series and issue pages takes on the colours of the cover.",
    default: true,
    motion: false,
  },
  {
    key: "hover_cards",
    group: "series",
    label: "Creator and character previews",
    description:
      "Hovering, or long-pressing, a creator or character shows who they are and what else they appear in.",
    default: true,
    motion: false,
  },
  {
    key: "ambient_glow",
    group: "reader",
    label: "Ambient page glow",
    description:
      "The space around the page glows with the colours at its edges.",
    default: false,
    motion: false,
  },
  {
    key: "page_curl",
    group: "reader",
    label: "Page curl",
    description:
      "Page turns in paged mode curl like paper. Mouse and trackpad only; swipes keep the regular slide.",
    default: false,
    motion: true,
  },
];

const BY_KEY = new Map(EFFECTS.map((e) => [e.key, e]));

/** The stored value, else the built-in default. Ignores reduced motion. */
export function effectSetting(
  stored: EffectPrefs | null | undefined,
  key: VisualEffect,
): boolean {
  const v = stored?.[key];
  return typeof v === "boolean" ? v : (BY_KEY.get(key)?.default ?? false);
}

const OS_REDUCED = "(prefers-reduced-motion: reduce)";

function subscribeOsReduced(cb: () => void) {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mq = window.matchMedia(OS_REDUCED);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
const osReducedSnapshot = () =>
  typeof window !== "undefined" && !!window.matchMedia?.(OS_REDUCED).matches;

/** True when animations should be cut: the account's Motion setting is
 *  "reduced", or it follows the system and the OS asks for reduced motion. */
export function useReducedMotion(): boolean {
  const me = useMe();
  const os = useSyncExternalStore(
    subscribeOsReduced,
    osReducedSnapshot,
    () => false,
  );
  return me.data?.motion === "reduced" || os;
}

/** Whether `key` should render right now: the user's switch (or default),
 *  minus motion effects while reduced motion is in force. */
export function useVisualEffect(key: VisualEffect): boolean {
  const me = useMe();
  const reduced = useReducedMotion();
  const on = effectSetting(me.data?.effects, key);
  return on && !(reduced && BY_KEY.get(key)?.motion);
}
