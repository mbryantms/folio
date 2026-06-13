/**
 * Semantic status-color helper (audit F1).
 *
 * The single source for status coloring across the app. Call sites stop
 * hardcoding palette utilities (`text-emerald-400`, `bg-amber-500/10`,
 * `text-red-500`, …) — which only ever looked right on one theme — and
 * use these tones, which resolve through the `--success` / `--warning` /
 * `--info` / `--destructive` theme tokens (see `globals.css`) and so are
 * correct + WCAG-AA on dark, light, and amber automatically.
 *
 *   statusTone("success")      // subtle surface: tinted bg + text + border (pills/badges)
 *   statusToneText("warning")  // text color only
 *   statusToneSolid("info")    // filled: solid bg + on-color foreground
 *   statusToneDot("error")     // bg color for a status dot/indicator
 *
 * `error` maps to the existing `--destructive`; `neutral` to `--muted`.
 * A `--rating` token exists for the gold star tint (kept distinct from
 * `warning`'s orange) — use `text-rating` / `fill-rating` directly.
 *
 * NOTE: every class string below is a *literal* (not a template
 * concatenation) so Tailwind v4's source scanner generates the
 * utilities. Don't refactor these into `bg-${t}` interpolation — the JIT
 * can't see dynamically-built class names and the colors silently vanish.
 */

export type StatusTone = "success" | "warning" | "info" | "error" | "neutral";

/** Subtle surface — tinted background + on-tone text + hairline border.
 *  The default for status pills/badges/chips. */
const SUBTLE: Record<StatusTone, string> = {
  success: "bg-success/10 text-success border-success/25",
  warning: "bg-warning/10 text-warning border-warning/25",
  info: "bg-info/10 text-info border-info/25",
  error: "bg-destructive/10 text-destructive border-destructive/25",
  neutral: "bg-muted text-muted-foreground border-border",
};

const TEXT: Record<StatusTone, string> = {
  success: "text-success",
  warning: "text-warning",
  info: "text-info",
  error: "text-destructive",
  neutral: "text-muted-foreground",
};

const SOLID: Record<StatusTone, string> = {
  success: "bg-success text-success-foreground",
  warning: "bg-warning text-warning-foreground",
  info: "bg-info text-info-foreground",
  error: "bg-destructive text-destructive-foreground",
  neutral: "bg-muted text-muted-foreground",
};

const DOT: Record<StatusTone, string> = {
  success: "bg-success",
  warning: "bg-warning",
  info: "bg-info",
  error: "bg-destructive",
  neutral: "bg-muted-foreground",
};

/** Subtle status surface (tinted bg + text + border) — pills/badges. */
export function statusTone(tone: StatusTone): string {
  return SUBTLE[tone];
}

/** Text-color class for a tone. */
export function statusToneText(tone: StatusTone): string {
  return TEXT[tone];
}

/** Filled surface — solid background + on-color foreground. */
export function statusToneSolid(tone: StatusTone): string {
  return SOLID[tone];
}

/** Background-color class for a small dot / indicator of this tone. */
export function statusToneDot(tone: StatusTone): string {
  return DOT[tone];
}
