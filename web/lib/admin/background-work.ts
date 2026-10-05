/**
 * Pure helpers for the Background work surfaces (the admin header pill and
 * `/admin/background-work`). Kept free of React so they unit-test in node.
 */
import type {
  BackgroundWorkView,
  LibraryWorkView,
  QueueDepthView,
} from "@/lib/api/types";

/** The one page that shows everything in flight; the header pill, the
 *  dashboard card and the nav entry all point here. */
export const BACKGROUND_WORK_HREF = "/admin/background-work";

/** Friendly label for every apalis queue. */
export const QUEUE_LABELS: Record<string, string> = {
  scan: "Library scans",
  scan_series: "Series scans",
  post_scan_thumbs: "Thumbnails",
  post_scan_search: "Search index",
  post_scan_dictionary: "Dictionary",
  metadata_search_series: "Metadata search (series)",
  metadata_search_issue: "Metadata search (issue)",
  metadata_apply_series: "Metadata apply (series)",
  metadata_apply_issue: "Metadata apply (issue)",
  rewrite_issue_sidecars: "Sidecar rewrite",
  archive_edit: "Archive edits",
  backfill: "Backfills",
  hash_backfill: "Content hashing",
  relationship_suggest: "Relationship suggestions",
  provider_coverage: "Provider coverage",
};

export function queueLabel(queue: string): string {
  return QUEUE_LABELS[queue] ?? queue;
}

/** Short nouns for the pill tooltip ("14,200 thumbnails · 3 scans"). Queues
 *  that are one kind of work to an operator share a noun and are summed. */
const PILL_NOUNS: Record<string, string> = {
  scan: "scans",
  scan_series: "scans",
  post_scan_thumbs: "thumbnails",
  post_scan_search: "search index",
  post_scan_dictionary: "dictionary",
  metadata_search_series: "metadata",
  metadata_search_issue: "metadata",
  metadata_apply_series: "metadata",
  metadata_apply_issue: "metadata",
  rewrite_issue_sidecars: "sidecar rewrites",
  archive_edit: "archive edits",
  backfill: "backfills",
  hash_backfill: "content hashing",
  relationship_suggest: "relationship suggestions",
  provider_coverage: "provider coverage",
};

/** `16,204` → `16.2k`; small numbers stay exact. */
export function compactCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(Math.floor(n / 100) / 10).toFixed(1)}k`;
  if (n < 1_000_000) return `${Math.floor(n / 1000)}k`;
  return `${(Math.floor(n / 100_000) / 10).toFixed(1)}M`;
}

/**
 * What the pending total is made of, largest first — the pill's tooltip.
 * At most `max` kinds are named; the rest fold into "N other".
 */
export function pillBreakdown(
  depth: Pick<QueueDepthView, "queues">,
  max = 3,
): string {
  const byNoun = new Map<string, number>();
  for (const q of depth.queues) {
    const n = q.waiting + q.scheduled + q.in_flight;
    if (n <= 0) continue;
    const noun = PILL_NOUNS[q.queue] ?? q.queue;
    byNoun.set(noun, (byNoun.get(noun) ?? 0) + n);
  }
  const ranked = [...byNoun.entries()].sort((a, b) => b[1] - a[1]);
  const named = ranked
    .slice(0, max)
    .map(([noun, n]) => `${n.toLocaleString("en-US")} ${noun}`);
  const rest = ranked.slice(max).reduce((sum, [, n]) => sum + n, 0);
  if (rest > 0) named.push(`${rest.toLocaleString("en-US")} other`);
  return named.join(" · ");
}

export function pct(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((done / total) * 100)));
}

/** Busy libraries first (scanning before thumbnail-only), then by name. */
export function sortLibraries(libs: LibraryWorkView[]): LibraryWorkView[] {
  const live = (l: LibraryWorkView) => l.scan && !l.scan.stalled;
  const rank = (l: LibraryWorkView) =>
    live(l) && l.scan?.state === "running" ? 0 : live(l) ? 1 : l.busy ? 2 : 3;
  return [...libs].sort(
    (a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name),
  );
}

/** Human label for a scanner phase. */
export function phaseLabel(phase: string | null | undefined): string {
  switch (phase) {
    case "planning":
    case "planning_complete":
      return "Planning";
    case "scanning":
      return "Scanning files";
    case "reconciling":
    case "reconciled":
      return "Reconciling";
    case "enqueueing_thumbnails":
      return "Queueing thumbnails";
    case "complete":
      return "Finishing";
    default:
      return "Starting";
  }
}

/** Where an "other work" queue's detail lives. */
export function queueHref(queue: string): string {
  return queue.startsWith("metadata_") || queue === "provider_coverage"
    ? "/admin/metadata"
    : "/admin/queue";
}

/** Queues the per-library table already accounts for. */
const LIBRARY_TABLE_QUEUES = new Set([
  "scan",
  "scan_series",
  "post_scan_thumbs",
]);

export type OtherWorkRow = {
  queue: string;
  label: string;
  href: string;
  pending: number;
  inFlight: number;
};

/** Non-empty queues that have no column in the per-library table. */
export function otherWork(view: BackgroundWorkView): OtherWorkRow[] {
  return view.queues
    .filter((q) => !LIBRARY_TABLE_QUEUES.has(q.queue))
    .map((q) => ({
      queue: q.queue,
      label: queueLabel(q.queue),
      href: queueHref(q.queue),
      pending: q.waiting + q.scheduled + q.in_flight,
      inFlight: q.in_flight,
    }))
    .filter((r) => r.pending > 0);
}
