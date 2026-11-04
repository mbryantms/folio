/**
 * Live scan dashboard state (folio-flair phase 5): one running scan, reduced
 * from the admin scan-event stream (`/ws/scan-events`) and seeded from the
 * background-work snapshot. Pure, so it's unit-testable; the panel keeps one
 * of these per library. Plan: ~/.claude/plans/folio-flair-phase5-admin.md
 */
import type { ActiveScanView, ScanEvent } from "@/lib/api/types";

export type LiveScanStatus = "queued" | "running" | "complete" | "failed";

export type LogTone = "" | "ok" | "warn" | "error" | "new";
export type LogLine = { at: number; tone: LogTone; text: string };

export type Discovery = {
  seriesId: string;
  name: string;
  coverUrl: string | null;
  /** New to the library (vs. changed), when the server says so. */
  created: boolean | null;
};

export type LiveScan = {
  libraryId: string;
  scanId: string | null;
  status: LiveScanStatus;
  phase: string | null;
  completed: number;
  total: number;
  label: string | null;
  counters: {
    files: number;
    newIssues: number;
    updated: number;
    newSeries: number;
    errors: number;
  };
  /** Recent throughput samples (files/s), oldest first, at most `SAMPLES`. */
  samples: number[];
  /** Current files/s (smoothed). */
  rate: number;
  /** Remaining time, ms; null until there's a stable rate. */
  etaMs: number | null;
  discovered: Discovery[];
  log: LogLine[];
  /** Totals reported when the scan finished. */
  result: {
    added: number;
    updated: number;
    removed: number;
    durationMs: number;
  } | null;
  error: string | null;
  /** Last progress reading, for the client-side rate fallback. */
  last: { at: number; files: number; completed: number } | null;
};

export const SAMPLES = 40;
export const MAX_DISCOVERED = 24;
export const MAX_LOG = 80;

export function emptyScan(libraryId: string): LiveScan {
  return {
    libraryId,
    scanId: null,
    status: "queued",
    phase: null,
    completed: 0,
    total: 0,
    label: null,
    counters: { files: 0, newIssues: 0, updated: 0, newSeries: 0, errors: 0 },
    samples: [],
    rate: 0,
    etaMs: null,
    discovered: [],
    log: [],
    result: null,
    error: null,
    last: null,
  };
}

/** Seed (or refresh) from the background-work snapshot of an in-flight run. */
export function seedFromActive(
  state: LiveScan,
  scan: ActiveScanView,
): LiveScan {
  // Live events win once they're flowing for this run.
  if (state.scanId === scan.id && state.last) return state;
  return {
    ...state,
    scanId: scan.id,
    status: scan.state === "queued" ? "queued" : "running",
    phase: scan.phase ?? state.phase,
    completed: scan.completed ?? state.completed,
    total: scan.total ?? state.total,
    label: scan.current_label ?? state.label,
    rate: scan.files_per_sec ?? state.rate,
  };
}

const push = <T>(arr: readonly T[], item: T, max: number): T[] => {
  const next = [...arr, item];
  return next.length > max ? next.slice(next.length - max) : next;
};

const line = (at: number, tone: LogTone, text: string): LogLine => ({
  at,
  tone,
  text,
});

const PHASE_TEXT: Record<string, string> = {
  planning: "planning folders",
  planning_complete: "plan ready",
  scanning: "scanning files",
  reconciling: "reconciling",
  reconciled: "reconciled",
  enqueueing_thumbnails: "queueing covers",
  complete: "complete",
};

/** The scanner's generic "Scanning files" label isn't a folder name. */
const isFolderLabel = (label: string | null | undefined): label is string =>
  !!label && !/^scanning files$/i.test(label.trim());

/** "OrphanedSeriesJson" / "orphaned_series_json" → "orphaned series json". */
export function humanizeKind(kind: string): string {
  return kind
    .replace(/_/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase();
}

export function phaseText(phase: string | null): string {
  if (!phase) return "starting";
  return PHASE_TEXT[phase] ?? phase.replace(/_/g, " ");
}

/**
 * Apply one event for this library. `now` is the arrival time (ms), used
 * for the client-side rate when the server doesn't send a recent one.
 */
export function applyScanEvent(
  state: LiveScan,
  evt: ScanEvent,
  now: number,
): LiveScan {
  if (!("library_id" in evt) || evt.library_id !== state.libraryId)
    return state;
  switch (evt.type) {
    case "scan.started":
      return {
        ...emptyScan(state.libraryId),
        scanId: evt.scan_id,
        status: "running",
        log: push(state.log, line(now, "", "scan started"), MAX_LOG),
      };
    case "scan.progress": {
      if (state.scanId && evt.scan_id !== state.scanId && state.last)
        return state;
      const files = evt.files_seen ?? state.counters.files;
      // Rate: the server's 10s-window figure when it sends one (lightly
      // smoothed); otherwise files per second between this reading and the
      // last, smoothed harder.
      let rate = state.rate;
      if (evt.files_per_sec != null) {
        rate =
          state.rate > 0
            ? state.rate * 0.5 + evt.files_per_sec * 0.5
            : evt.files_per_sec;
      } else if (state.last && now > state.last.at) {
        const dt = (now - state.last.at) / 1000;
        const inst = Math.max(0, (files - state.last.files) / dt);
        rate = state.rate > 0 ? rate * 0.75 + inst * 0.25 : inst;
      }
      const serverEta = "eta_ms" in evt ? (evt.eta_ms ?? null) : null;
      // Client ETA from the completed-units pace, once a few samples exist.
      let etaMs = serverEta;
      if (etaMs == null && state.last && now > state.last.at) {
        const unitsPerMs =
          (evt.completed - state.last.completed) / (now - state.last.at);
        const remaining = Math.max(0, evt.total - evt.completed);
        etaMs =
          unitsPerMs > 0 && state.samples.length >= 3
            ? remaining / unitsPerMs
            : state.etaMs;
      }
      const phaseChanged = evt.phase !== state.phase;
      return {
        ...state,
        scanId: evt.scan_id,
        status: evt.phase === "complete" ? "complete" : "running",
        phase: evt.phase,
        completed: evt.completed,
        total: evt.total,
        label: isFolderLabel(evt.current_label)
          ? evt.current_label
          : state.label,
        counters: {
          files,
          newIssues: evt.files_added ?? state.counters.newIssues,
          updated: evt.files_updated ?? state.counters.updated,
          newSeries: state.counters.newSeries,
          errors: evt.health_issues ?? state.counters.errors,
        },
        samples: push(state.samples, Math.round(rate * 10) / 10, SAMPLES),
        rate,
        etaMs: evt.phase === "complete" ? 0 : etaMs,
        last: { at: now, files, completed: evt.completed },
        log: phaseChanged
          ? push(state.log, line(now, "", phaseText(evt.phase)), MAX_LOG)
          : isFolderLabel(evt.current_label) &&
              evt.current_label !== state.label
            ? push(
                state.log,
                line(now, "", `scanned ${evt.current_label}`),
                MAX_LOG,
              )
            : state.log,
      };
    }
    case "scan.series_updated": {
      const created =
        "created" in evt && typeof evt.created === "boolean"
          ? evt.created
          : null;
      const coverUrl =
        "cover_url" in evt && typeof evt.cover_url === "string"
          ? evt.cover_url
          : null;
      const seen = state.discovered.some((d) => d.seriesId === evt.series_id);
      const discovered = seen
        ? state.discovered.map((d) =>
            d.seriesId === evt.series_id
              ? { ...d, coverUrl: coverUrl ?? d.coverUrl }
              : d,
          )
        : [
            { seriesId: evt.series_id, name: evt.name, coverUrl, created },
            ...state.discovered,
          ].slice(0, MAX_DISCOVERED);
      return {
        ...state,
        discovered,
        counters:
          !seen && created
            ? { ...state.counters, newSeries: state.counters.newSeries + 1 }
            : state.counters,
        log: seen
          ? state.log
          : push(
              state.log,
              line(
                now,
                created ? "new" : "",
                `${created ? "new series" : "updated"} · ${evt.name}`,
              ),
              MAX_LOG,
            ),
      };
    }
    case "scan.health_issue":
      return {
        ...state,
        log: push(
          state.log,
          line(
            now,
            evt.severity === "error" ? "error" : "warn",
            `${humanizeKind(evt.kind)}${evt.path ? ` · ${evt.path}` : ""}`,
          ),
          MAX_LOG,
        ),
      };
    case "scan.completed":
      if (state.scanId && evt.scan_id !== state.scanId) return state;
      return {
        ...state,
        scanId: evt.scan_id,
        status: "complete",
        phase: "complete",
        completed: state.total || state.completed,
        etaMs: 0,
        rate: 0,
        result: {
          added: evt.added,
          updated: evt.updated,
          removed: evt.removed,
          durationMs: evt.duration_ms,
        },
        log: push(
          state.log,
          line(
            now,
            "ok",
            `scan finished · ${evt.added} added, ${evt.updated} updated, ${evt.removed} removed`,
          ),
          MAX_LOG,
        ),
      };
    case "scan.failed":
      if (state.scanId && evt.scan_id !== state.scanId) return state;
      return {
        ...state,
        scanId: evt.scan_id,
        status: "failed",
        error: evt.error,
        rate: 0,
        etaMs: null,
        log: push(
          state.log,
          line(now, "error", `scan failed · ${evt.error}`),
          MAX_LOG,
        ),
      };
    default:
      return state;
  }
}

/** "1m 05s" / "42s" / "1h 03m". */
export function formatEta(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return "—";
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/** SVG path for the throughput sparkline in a `w`×`h` box. */
export function sparkPath(
  samples: readonly number[],
  w: number,
  h: number,
): { line: string; area: string } | null {
  if (samples.length < 2) return null;
  const max = Math.max(1, ...samples) * 1.1;
  const step = w / (SAMPLES - 1);
  const x0 = w - (samples.length - 1) * step;
  const pts = samples.map(
    (v, i) => [x0 + i * step, h - 2 - (v / max) * (h - 6)] as const,
  );
  const line = `M${pts.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" L")}`;
  return {
    line,
    area: `${line} L${w},${h} L${pts[0]![0].toFixed(1)},${h} Z`,
  };
}
