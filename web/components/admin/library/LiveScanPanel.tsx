"use client";

import * as React from "react";
import Link from "next/link";
import { CheckCircle2, Play, Square, X, XCircle } from "lucide-react";

import { BumpNumber } from "@/components/admin/BumpNumber";
import { ScanAllButton } from "@/components/admin/library/ScanAllButton";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/folio/select";
import {
  applyScanEvent,
  emptyScan,
  formatEta,
  phaseText,
  seedFromActive,
  sparkPath,
  type LiveScan,
  type LogTone,
} from "@/lib/admin/live-scan";
import { useCancelScanRun, useTriggerScan } from "@/lib/api/mutations";
import { useBackgroundWork } from "@/lib/api/queries";
import { useScanEvents } from "@/lib/api/scan-events";
import type { LibraryWorkView, ScanEvent } from "@/lib/api/types";
import { libraryScanHref } from "@/lib/admin/routes";
import { cn } from "@/lib/utils";

type Lib = Pick<LibraryWorkView, "id" | "name" | "slug">;

/**
 * Live scan dashboard (folio-flair phase 5): the top of Admin → Scans.
 * Start a scan for one library (or all); every running scan
 * gets a live card: a progress ring with ticking counters, a throughput
 * sparkline with an ETA, newly found series popping in, and a log. Driven
 * by the admin scan-event socket, seeded from the background-work snapshot
 * (which also says which libraries are scanning). A finished scan stays
 * with its summary until dismissed.
 *
 * With `libraryIds` (a "Scan all" batch's members) the panel shows only
 * those libraries' cards and drops the start bar — the batch view already
 * has its own overall progress and status list.
 * Plan: ~/.claude/plans/folio-flair-phase5-admin.md
 */
export function LiveScanPanel({
  libraryIds,
}: {
  /** Restrict the cards to these libraries (batch view). */
  libraryIds?: readonly string[];
} = {}) {
  const work = useBackgroundWork();
  const { events, status } = useScanEvents({
    toastCompletions: false,
    maxBuffer: 400,
  });
  const libs = React.useMemo<Lib[]>(
    () =>
      (work.data?.libraries ?? [])
        .map(({ id, name, slug }) => ({ id, name, slug }))
        .sort((a, b) => a.name.localeCompare(b.name)),
    [work.data],
  );

  // One live state per library, fed each event exactly once.
  const [scans, setScans] = React.useState<Record<string, LiveScan>>({});
  const [dismissed, setDismissed] = React.useState<Set<string>>(new Set());
  const seen = React.useRef(new WeakSet<ScanEvent>());
  React.useEffect(() => {
    const fresh = events.filter((e) => !seen.current.has(e));
    if (!fresh.length) return;
    for (const e of fresh) seen.current.add(e);
    const now = Date.now();
    setScans((prev) => {
      let next = prev;
      for (const e of fresh) {
        if (!("library_id" in e) || !e.type.startsWith("scan.")) continue;
        const cur = next[e.library_id] ?? emptyScan(e.library_id);
        const upd = applyScanEvent(cur, e, now);
        if (upd !== cur) next = { ...next, [e.library_id]: upd };
      }
      return next;
    });
    // A new run un-dismisses its library.
    const started = fresh.filter((e) => e.type === "scan.started");
    if (started.length)
      setDismissed((d) => {
        const n = new Set(d);
        for (const e of started) if ("library_id" in e) n.delete(e.library_id);
        return n;
      });
  }, [events]);

  // Seed / refresh from the snapshot for runs the socket hasn't shown yet.
  React.useEffect(() => {
    const active = (work.data?.libraries ?? []).filter((l) => l.scan);
    if (!active.length) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- snapshot seed
    setScans((prev) => {
      let next = prev;
      for (const l of active) {
        const cur = next[l.id] ?? emptyScan(l.id);
        const upd = seedFromActive(cur, l.scan!);
        if (upd !== cur) next = { ...next, [l.id]: upd };
      }
      return next;
    });
  }, [work.data]);

  const activeIds = new Set(
    (work.data?.libraries ?? []).filter((l) => l.scan).map((l) => l.id),
  );
  const scope = libraryIds ? new Set(libraryIds) : null;
  const shown = libs.filter((l) => {
    if (scope && !scope.has(l.id)) return false;
    const s = scans[l.id];
    if (dismissed.has(l.id))
      return activeIds.has(l.id) && s?.status !== "complete";
    return (
      activeIds.has(l.id) ||
      (s &&
        (s.status === "running" ||
          s.status === "complete" ||
          s.status === "failed"))
    );
  });

  if (scope) {
    // Batch view: nothing live (all queued, or finished before this page
    // opened) means the status list above says it all.
    if (shown.length === 0) return null;
    return (
      <section className="space-y-4" aria-label="Live scans">
        {shown.map((l) => (
          <LiveScanCard
            key={l.id}
            lib={l}
            scan={scans[l.id] ?? emptyScan(l.id)}
            onDismiss={() => setDismissed((d) => new Set(d).add(l.id))}
          />
        ))}
      </section>
    );
  }

  return (
    <section className="space-y-4" aria-label="Live scans">
      <StartScanBar libs={libs} busy={activeIds} live={status === "open"} />
      {shown.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-xl border border-dashed px-6 py-10 text-center text-sm">
          No scan running. Start one above and watch it here live: progress,
          throughput, and new series as they&rsquo;re found.
        </div>
      ) : (
        shown.map((l) => (
          <LiveScanCard
            key={l.id}
            lib={l}
            scan={scans[l.id] ?? emptyScan(l.id)}
            onDismiss={() => setDismissed((d) => new Set(d).add(l.id))}
          />
        ))
      )}
    </section>
  );
}

function StartScanBar({
  libs,
  busy,
  live,
}: {
  libs: Lib[];
  busy: Set<string>;
  live: boolean;
}) {
  const [pick, setPick] = React.useState<string>("");
  const libraryId = pick || libs[0]?.id || "";
  // The scan route is keyed by slug (the hook's arg is named `libraryId`
  // but every caller passes the route slug).
  const slug = libs.find((l) => l.id === libraryId)?.slug ?? "";
  const trigger = useTriggerScan(slug);
  const running = busy.has(libraryId);
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select value={libraryId} onValueChange={setPick}>
        <SelectTrigger
          className="folio-control h-9 w-52"
          aria-label="Library to scan"
        >
          <SelectValue placeholder="Choose a library" />
        </SelectTrigger>
        <SelectContent>
          {libs.map((l) => (
            <SelectItem key={l.id} value={l.id}>
              {l.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Button
        type="button"
        disabled={!slug || running || trigger.isPending}
        onClick={() => trigger.mutate({ mode: "normal" })}
      >
        <Play className="size-4" aria-hidden />
        {running ? "Scanning…" : "Start scan"}
      </Button>
      <ScanAllButton />
      <span
        className="text-muted-foreground ml-auto inline-flex items-center gap-1.5 text-xs"
        title={live ? "Connected to the live scan stream" : "Reconnecting…"}
      >
        <span
          aria-hidden
          className={cn(
            "size-2 rounded-full",
            live ? "bg-success animate-pulse" : "bg-muted-foreground/50",
          )}
        />
        {live ? "Live" : "Connecting…"}
      </span>
    </div>
  );
}

const RING_R = 52;
const RING_C = 2 * Math.PI * RING_R;

function LiveScanCard({
  lib,
  scan,
  onDismiss,
}: {
  lib: Lib;
  scan: LiveScan;
  onDismiss: () => void;
}) {
  const cancel = useCancelScanRun(lib.slug);
  const done = scan.status === "complete";
  const failed = scan.status === "failed";
  const pct = done
    ? 100
    : scan.total > 0
      ? Math.min(99, Math.floor((scan.completed / scan.total) * 100))
      : 0;
  return (
    <article
      className="bg-card space-y-4 rounded-xl border p-4 sm:p-5"
      aria-label={`${lib.name} scan`}
    >
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <h2 className="truncate text-lg font-semibold">
            <Link href={libraryScanHref(lib.slug)} className="fact-link">
              {lib.name}
            </Link>
          </h2>
          <p className="text-muted-foreground text-sm">
            {failed
              ? "Scan failed"
              : done
                ? "Scan complete"
                : scan.status === "queued"
                  ? "Queued"
                  : `Running · ${phaseText(scan.phase)}`}
          </p>
        </div>
        {done || failed ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={onDismiss}
            aria-label={`Dismiss ${lib.name} scan`}
          >
            <X className="size-4" aria-hidden /> Dismiss
          </Button>
        ) : scan.scanId ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={cancel.isPending}
            onClick={() => cancel.mutate({ scanId: scan.scanId! })}
          >
            <Square className="size-3.5" aria-hidden /> Stop
          </Button>
        ) : null}
      </header>

      {done && scan.result ? (
        <div
          role="status"
          className="border-success/40 bg-success/10 flex items-center gap-3 rounded-lg border px-3 py-2.5 text-sm"
        >
          <CheckCircle2 className="text-success size-5 shrink-0" aria-hidden />
          <span>
            <b className="font-semibold">Scan complete</b> ·{" "}
            {scan.counters.files.toLocaleString()} files in{" "}
            {formatEta(scan.result.durationMs)} · {scan.result.added} added,{" "}
            {scan.result.updated} updated, {scan.result.removed} removed
          </span>
        </div>
      ) : failed ? (
        <div
          role="alert"
          className="border-destructive/40 bg-destructive/10 flex items-center gap-3 rounded-lg border px-3 py-2.5 text-sm"
        >
          <XCircle className="text-destructive size-5 shrink-0" aria-hidden />
          <span className="min-w-0 break-words">{scan.error}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <div className="bg-background/40 flex items-center gap-5 rounded-lg border p-4">
          <Ring pct={pct} done={done} />
          <dl className="grid min-w-0 flex-1 grid-cols-2 gap-x-4 gap-y-3">
            <Counter label="Files scanned" value={scan.counters.files} quiet />
            <Counter label="New series" value={scan.counters.newSeries} />
            <Counter label="New issues" value={scan.counters.newIssues} />
            {/* Health issues the scan found (missing files, bad archives, …);
                mostly warnings, so amber rather than red. */}
            <Counter label="Health issues" value={scan.counters.errors} warn />
          </dl>
        </div>
        <Throughput scan={scan} />
      </div>

      <Discovered scan={scan} />
      <LogFeed scan={scan} />
    </article>
  );
}

function Ring({ pct, done }: { pct: number; done: boolean }) {
  return (
    <div className="relative size-28 shrink-0">
      <svg viewBox="0 0 120 120" aria-hidden className="size-full -rotate-90">
        <circle
          cx="60"
          cy="60"
          r={RING_R}
          fill="none"
          strokeWidth="9"
          className="stroke-muted"
        />
        <circle
          cx="60"
          cy="60"
          r={RING_R}
          fill="none"
          strokeWidth="9"
          strokeLinecap="round"
          strokeDasharray={RING_C}
          strokeDashoffset={RING_C * (1 - pct / 100)}
          className={cn(
            "transition-[stroke-dashoffset,stroke] duration-300 ease-linear",
            done ? "stroke-success" : "stroke-primary",
          )}
        />
      </svg>
      <span
        role="status"
        aria-label={`${pct}% scanned`}
        className={cn(
          "absolute inset-0 grid place-items-center text-2xl font-bold tabular-nums",
          done && "text-success",
        )}
      >
        {pct}%
      </span>
    </div>
  );
}

/** A counter that bumps when it ticks up (not for the busy file count,
 *  which would never stop bouncing). */
function Counter({
  label,
  value,
  warn,
  quiet,
}: {
  label: string;
  value: number;
  warn?: boolean;
  quiet?: boolean;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground truncate text-xs">{label}</dt>
      <dd
        className={cn(
          "text-xl font-bold tabular-nums",
          warn && value > 0 && "text-warning",
        )}
      >
        <BumpNumber value={value} on={quiet ? "never" : "up"} />
      </dd>
    </div>
  );
}

function Throughput({ scan }: { scan: LiveScan }) {
  const W = 400;
  const H = 84;
  const path = sparkPath(scan.samples, W, H);
  const running = scan.status === "running";
  // Once finished, the whole-run average says more than a trailing 0.
  const avg =
    scan.result && scan.result.durationMs > 0
      ? scan.counters.files / (scan.result.durationMs / 1000)
      : null;
  const shownRate = running ? scan.rate : (avg ?? 0);
  // An ETA only means something while files are being scanned; the
  // reconcile and cover phases have no per-file pace, so name the phase.
  const showEta =
    running && scan.phase === "scanning" && (scan.etaMs ?? 0) >= 1000;
  return (
    <div className="bg-background/40 flex flex-col gap-1 rounded-lg border p-4">
      <div className="text-muted-foreground flex items-baseline justify-between text-xs">
        <span>Throughput</span>
        <span>
          <b className="text-foreground text-xl font-bold tabular-nums">
            {shownRate.toFixed(shownRate < 10 ? 1 : 0)}
          </b>{" "}
          files/s{avg != null && !running ? " avg" : ""}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        aria-hidden
        className="h-[84px] w-full"
      >
        <line
          x1="0"
          y1={H - 0.5}
          x2={W}
          y2={H - 0.5}
          className="stroke-border"
          vectorEffect="non-scaling-stroke"
        />
        {path ? (
          <>
            <path d={path.area} className="fill-primary/15" />
            <path
              d={path.line}
              fill="none"
              strokeWidth="2"
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
              className="stroke-primary"
            />
          </>
        ) : null}
      </svg>
      <div className="text-muted-foreground flex items-baseline justify-between gap-3 text-xs">
        <span className="min-w-0 truncate" title={scan.label ?? undefined}>
          {running && scan.label && scan.phase === "scanning"
            ? scan.label
            : running
              ? "Recent rate"
              : scan.status === "complete"
                ? "Whole run"
                : "Waiting for a worker"}
        </span>
        <span className="shrink-0 tabular-nums">
          {scan.status === "complete"
            ? "Done"
            : showEta
              ? `ETA ${formatEta(scan.etaMs)}`
              : running
                ? phaseText(scan.phase).replace(/^./, (c) => c.toUpperCase())
                : scan.status === "queued"
                  ? "Queued"
                  : "Stopped"}
        </span>
      </div>
    </div>
  );
}

function Discovered({ scan }: { scan: LiveScan }) {
  return (
    <div className="space-y-2">
      <h3 className="text-sm font-semibold">
        Discovered{" "}
        {scan.discovered.length ? (
          <span className="text-muted-foreground font-normal">
            · {scan.discovered.length}
          </span>
        ) : null}
      </h3>
      {scan.discovered.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          New and changed series pop in here as the scan finds them.
        </p>
      ) : (
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(84px,1fr))] gap-x-2.5 gap-y-3">
          {scan.discovered.map((d) => (
            <li key={d.seriesId} className="discover-pop min-w-0">
              {d.coverUrl ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  src={d.coverUrl}
                  alt=""
                  loading="lazy"
                  className="aspect-[2/3] w-full rounded-md object-cover shadow-md"
                />
              ) : (
                <div className="bg-muted text-muted-foreground grid aspect-[2/3] w-full place-items-center rounded-md p-1 text-center text-xs">
                  {d.name.slice(0, 18)}
                </div>
              )}
              {d.created ? (
                <span className="text-primary mt-1 block text-xs font-bold tracking-wide uppercase">
                  New
                </span>
              ) : null}
              <span className="block truncate text-xs" title={d.name}>
                {d.name}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const TONE: Record<LogTone, string> = {
  "": "",
  ok: "text-success",
  warn: "text-warning",
  error: "text-destructive",
  new: "text-primary",
};

function LogFeed({ scan }: { scan: LiveScan }) {
  const ref = React.useRef<HTMLDivElement>(null);
  const stick = React.useRef(true);
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [scan.log]);
  const clock = (t: number) =>
    new Date(t).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  return (
    <div className="space-y-2">
      <h3 className="text-sm font-semibold">Log</h3>
      {/* eslint-disable jsx-a11y/no-noninteractive-tabindex -- a scrollable log: keyboard users focus it to scroll */}
      <div
        ref={ref}
        role="log"
        aria-label="Scan log"
        tabIndex={0}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current =
            el.scrollTop + el.clientHeight >= el.scrollHeight - 30;
        }}
        className="bg-background/40 focus-visible:ring-ring h-48 overflow-y-auto rounded-lg border px-3 py-2 font-mono text-xs leading-relaxed focus-visible:ring-2 focus-visible:outline-none"
      >
        {scan.log.length === 0 ? (
          <p className="text-muted-foreground">Waiting for the scan…</p>
        ) : (
          scan.log.map((l, i) => (
            <div
              key={`${l.at}-${i}`}
              className="log-line-in truncate"
              title={l.text}
            >
              <time className="text-muted-foreground mr-2">{clock(l.at)}</time>
              <span className={TONE[l.tone]}>{l.text}</span>
            </div>
          ))
        )}
      </div>
      {/* eslint-enable jsx-a11y/no-noninteractive-tabindex */}
    </div>
  );
}
