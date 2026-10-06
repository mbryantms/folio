/**
 * Provider-complete search: per-provider state of one metadata search run
 * (`metadata_run.provider_status`). A run asks every configured provider;
 * one denied by quota is *owed* and the run parks until it can be asked,
 * one that errors is flagged. These helpers turn the list into the short
 * line the Review queue and the match dialog show next to a result, so an
 * operator can tell "all three matched" from "ComicVine only — Metron and
 * GCD never answered" before applying.
 */
import type { ProviderStatus } from "@/lib/api/types";
import { providerLabel } from "@/lib/metadata/quota";

export type ProviderCoverage = "complete" | "partial" | "unknown";

/** `complete` when every provider answered (matched or not); `partial`
 *  when any is still owed, pending, or failed; `unknown` for runs that
 *  predate the bookkeeping (empty list). */
export function providerCoverage(list: ProviderStatus[]): ProviderCoverage {
  if (list.length === 0) return "unknown";
  return list.every((p) => p.state === "answered") ? "complete" : "partial";
}

/** Providers that answered with at least one candidate. */
export function matchedProviders(list: ProviderStatus[]): string[] {
  return list
    .filter((p) => p.state === "answered" && (p.candidates ?? 0) > 0)
    .map((p) => p.source);
}

/** One short clause per provider: `ComicVine ✓3`, `Metron —`,
 *  `GCD awaiting quota`, `Metron failed`. */
export function providerStatusClause(p: ProviderStatus): string {
  const name = providerLabel(p.source);
  switch (p.state) {
    case "answered":
      return (p.candidates ?? 0) > 0 ? `${name} ✓${p.candidates}` : `${name} —`;
    case "quota":
      return `${name} awaiting quota`;
    case "failed":
      return `${name} failed`;
    case "pending":
    default:
      return `${name} not asked`;
  }
}

/** The whole line, e.g. `ComicVine ✓1 · Metron ✓1 · GCD —`. Empty string
 *  for runs without bookkeeping. */
export function providerStatusLine(list: ProviderStatus[]): string {
  return list.map(providerStatusClause).join(" · ");
}

/** Headline for a finalized run: how many providers matched out of how
 *  many answered, plus the gap when the run didn't cover every provider.
 *  `null` when nothing is known. */
export function providerCoverageSummary(
  list: ProviderStatus[],
): { text: string; partial: boolean } | null {
  if (list.length === 0) return null;
  const matched = matchedProviders(list).length;
  const total = list.length;
  const gaps = list.filter((p) => p.state !== "answered");
  const base =
    matched === 0
      ? `No provider matched`
      : matched === total
        ? `All ${total} providers matched`
        : `${matched} of ${total} providers matched`;
  if (gaps.length === 0) return { text: base, partial: false };
  const why = gaps
    .map((p) => {
      const name = providerLabel(p.source);
      return p.state === "quota"
        ? `${name} awaiting quota`
        : p.state === "failed"
          ? `${name} failed`
          : `${name} not asked`;
    })
    .join(", ");
  return { text: `${base} · ${why}`, partial: true };
}
