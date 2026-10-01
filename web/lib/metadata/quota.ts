import type { CandidatesResp, RequestBudget } from "@/lib/api/types";

/** One provider's live budget, as carried on the candidates response (B13). */
export type ProviderQuota = NonNullable<
  CandidatesResp["quota"]
>["providers"][number];

/**
 * Below this fraction of the headline window's budget the search dialog
 * shows the "N of M requests left" note (WP-2.9).
 */
export const LOW_BUDGET_FRACTION = 0.2;

/** `"this minute"` / `"this hour"` / `"today"` for a budget window. */
export function budgetWindowLabel(window: RequestBudget["window"]): string {
  switch (window) {
    case "minute":
      return "this minute";
    case "hour":
      return "this hour";
    case "day":
      return "today";
  }
}

/** Fraction of the window's budget still available, `0..1`. */
export function budgetFraction(b: RequestBudget): number {
  if (b.limit <= 0) return 0;
  return Math.max(0, Math.min(1, b.remaining / b.limit));
}

/**
 * Seconds until the budget window resets, from a server timestamp and
 * an explicit `now` (kept pure — no `Date.now()` during render).
 */
export function budgetResetSeconds(b: RequestBudget, nowMs: number): number {
  const reset = Date.parse(b.reset_at);
  if (Number.isNaN(reset)) return 0;
  return Math.max(0, Math.round((reset - nowMs) / 1000));
}

/**
 * Admin-card line, e.g. `"812 of 5,000 left today · resets in 3h"`.
 */
export function formatBudget(b: RequestBudget, nowMs: number): string {
  const base = `${b.remaining.toLocaleString()} of ${b.limit.toLocaleString()} left ${budgetWindowLabel(b.window)}`;
  const reset = budgetResetSeconds(b, nowMs);
  return reset > 0 ? `${base} · resets in ${formatCountdown(reset)}` : base;
}

/**
 * The search dialog's one-line low-budget note, e.g.
 * `"Metron: 812 of 5,000 requests left today"`. `null` when the provider
 * carries no budget or still has ≥ 20% of it.
 */
export function budgetNote(p: ProviderQuota): string | null {
  const b = p.budget;
  if (!b) return null;
  if (budgetFraction(b) >= LOW_BUDGET_FRACTION) return null;
  return `${providerLabel(p.provider)}: ${b.remaining.toLocaleString()} of ${b.limit.toLocaleString()} requests left ${budgetWindowLabel(b.window)}`;
}

const PROVIDER_LABELS: Record<string, string> = {
  comicvine: "ComicVine",
  metron: "Metron",
  gcd: "GCD",
};

export function providerLabel(id: string): string {
  return PROVIDER_LABELS[id] ?? id;
}

/** Compact countdown: `"now"` / `"<1m"` / `"47m"` / `"2h 3m"`. */
export function formatCountdown(seconds: number): string {
  if (seconds <= 0) return "now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const h = Math.floor(minutes / 60);
  const rem = minutes % 60;
  return rem ? `${h}h ${rem}m` : `${h}h`;
}

/** True when the provider has exhausted either bucket. */
export function isDepleted(p: ProviderQuota): boolean {
  return p.remaining_hour === 0 || p.remaining_day === 0;
}

/**
 * One provider's budget line, e.g. `"ComicVine: 180/hr"` or, when
 * exhausted, `"ComicVine: 0/hr (resets in 47m)"`. Phrasing matches the
 * admin dashboard's `/hr` · `/day` convention.
 */
export function summarizeProviderQuota(p: ProviderQuota): string {
  const parts: string[] = [];
  if (p.remaining_hour != null)
    parts.push(`${p.remaining_hour.toLocaleString()}/hr`);
  if (p.remaining_day != null)
    parts.push(`${p.remaining_day.toLocaleString()}/day`);
  let line = `${providerLabel(p.provider)}: ${parts.length ? parts.join(" · ") : "—"}`;
  if (p.seconds_until_reset != null && isDepleted(p)) {
    line += ` (resets in ${formatCountdown(p.seconds_until_reset)})`;
  }
  return line;
}

/**
 * ETA for a quota-parked retry from the server-computed relative
 * `retry_after_seconds`. Returns `null` when unknown so the caller can
 * fall back to the vague "try again shortly" copy. Relative (not an
 * absolute timestamp) so rendering stays a pure function — no
 * `Date.now()` during render.
 */
export function formatRetryEta(
  retryAfterSeconds: number | null | undefined,
): string | null {
  if (retryAfterSeconds == null) return null;
  return formatCountdown(retryAfterSeconds);
}
