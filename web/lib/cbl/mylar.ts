/**
 * Add-to-Mylar helpers (add-to-mylar 1.0): where a missing reading-list
 * entry stands with Mylar, and the wording of a request's result.
 */
import type {
  CblEntryView,
  MylarAddSummary,
  MylarListStatusView,
  MylarTargetStatus,
} from "@/lib/api/types";

/** Most entries one request may name; a longer selection uses "all". */
export const MYLAR_MAX_ENTRY_IDS = 500;

export type MylarEntryState =
  /** No ComicVine series id: Mylar cannot take it. */
  | { kind: "no_id" }
  /** Nothing known: can be added. */
  | { kind: "addable" }
  /** The series is on Mylar's watchlist; this issue is not marked wanted. */
  | { kind: "series_in_mylar" }
  /** The issue is wanted (or already fetched) in Mylar. */
  | { kind: "wanted" }
  /** Queued in Folio, or waiting for Mylar to import the series. */
  | { kind: "requested" }
  | { kind: "failed"; error: string | null };

export type MylarStatusIndex = {
  series: Map<number, MylarTargetStatus>;
  issues: Map<number, MylarTargetStatus>;
};

export function indexMylarStatus(
  status: Pick<MylarListStatusView, "series" | "issues"> | undefined,
): MylarStatusIndex {
  return {
    series: new Map((status?.series ?? []).map((s) => [s.cv_id, s])),
    issues: new Map((status?.issues ?? []).map((s) => [s.cv_id, s])),
  };
}

/** The issue's own state wins; otherwise the series' state. */
export function mylarEntryState(
  entry: Pick<CblEntryView, "cv_series_id" | "cv_issue_id">,
  index: MylarStatusIndex,
): MylarEntryState {
  if (entry.cv_series_id == null) return { kind: "no_id" };
  const issue =
    entry.cv_issue_id != null ? index.issues.get(entry.cv_issue_id) : undefined;
  const series = index.series.get(entry.cv_series_id);
  if (issue?.state === "in_mylar") return { kind: "wanted" };
  for (const target of [issue, series]) {
    if (target?.state === "failed") {
      return { kind: "failed", error: target.error ?? null };
    }
  }
  if (issue?.state === "requested" || series?.state === "requested") {
    return { kind: "requested" };
  }
  if (series?.state === "in_mylar") return { kind: "series_in_mylar" };
  return { kind: "addable" };
}

/** Whether "Add to Mylar" can do anything for an entry in this state. */
export function mylarCanAdd(state: MylarEntryState): boolean {
  return (
    state.kind === "addable" ||
    state.kind === "series_in_mylar" ||
    state.kind === "failed"
  );
}

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The toast after a request: what was queued, and what was left out. */
export function mylarAddMessage(s: MylarAddSummary): string {
  const queued: string[] = [];
  if (s.series_requested > 0) {
    queued.push(count(s.series_requested, "series", "series"));
  }
  if (s.issues_requested > 0) {
    queued.push(count(s.issues_requested, "issue", "issues"));
  }
  const notes: string[] = [];
  if (s.series_in_mylar > 0) {
    notes.push(
      `${count(s.series_in_mylar, "series", "series")} already in Mylar`,
    );
  }
  if (s.already_requested > 0) {
    notes.push(`${s.already_requested} already requested`);
  }
  if (s.without_comicvine_id > 0) {
    notes.push(
      `${count(s.without_comicvine_id, "entry has", "entries have")} no ComicVine id`,
    );
  }
  const head =
    queued.length > 0
      ? `Queued ${queued.join(" and ")} for Mylar`
      : "Nothing new to send to Mylar";
  return notes.length > 0 ? `${head} · ${notes.join(" · ")}` : head;
}
