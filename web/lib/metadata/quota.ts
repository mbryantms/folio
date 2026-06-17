import type { CandidatesResp } from "@/lib/api/types";

/** One provider's live budget, as carried on the candidates response (B13). */
export type ProviderQuota = NonNullable<
  CandidatesResp["quota"]
>["providers"][number];

const PROVIDER_LABELS: Record<string, string> = {
  comicvine: "ComicVine",
  metron: "Metron",
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
