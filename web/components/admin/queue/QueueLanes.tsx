"use client";

import * as React from "react";
import Link from "next/link";
import {
  Check,
  Clock,
  FolderSearch,
  Hourglass,
  Library,
  RotateCcw,
  X,
} from "lucide-react";

import { QueueLanesSkeleton } from "@/components/skeletons/AdminShapes";
import { Button } from "@/components/ui/button";
import {
  buildLanes,
  formatDuration,
  QUEUE_FILTERS,
  type QueueFilter,
} from "@/lib/admin/queue-lanes";
import { useQueueJobs } from "@/lib/api/queries";
import type { JobCard, JobTarget } from "@/lib/api/types";
import { adminLibraryUrl, issueUrl, seriesUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

/** Cards shown per lane; the rest are summarised as "+N more". */
const SHOW_QUEUED = 8;
const SHOW_RUNNING = 12;
const SHOW_DONE = 12;
const GLIDE_MS = 550;
const POP_MS = 450;
/** Cap on cards animated per update, so a busy refetch stays cheap. */
const MAX_ANIMATED = 48;
const EASE = "cubic-bezier(0.22, 1, 0.36, 1)";

const noop = () => () => {};

/**
 * Queue lanes (folio-flair phase 5, F2): the Queue page's Overview. Every
 * queue's jobs merged into Queued → Running → Done, with job cards that
 * glide between lanes as they move (FLIP, keyed by task id) and pop in
 * when new. Data: `GET /admin/queue/jobs`, refetched on each coalesced
 * `queue.activity` event (the admin shell's event socket invalidates it)
 * and polled every 10s as a fallback.
 */
export function QueueLanes({ onShowFailed }: { onShowFailed: () => void }) {
  // Render nothing data-driven until hydrated, so the server HTML and the
  // first client render agree (queue queries can already be warm).
  const hydrated = React.useSyncExternalStore(
    noop,
    () => true,
    () => false,
  );
  const [filter, setFilter] = React.useState<QueueFilter>("all");
  const q = useQueueJobs(null, { limit: 20 });
  const lanes = React.useMemo(
    () => buildLanes(q.data, filter),
    [q.data, filter],
  );
  const reduced = useReducedMotion();
  const board = React.useRef<HTMLDivElement | null>(null);
  useFlip(board, `${q.dataUpdatedAt}:${filter}`, reduced || !hydrated);
  const now = useNow(lanes.running.length > 0);

  if (!hydrated || q.isLoading) return <QueueLanesSkeleton />;
  if (!q.data) {
    return <p className="text-destructive text-sm">Failed to load jobs.</p>;
  }
  const { counts } = lanes;
  const queuedTotal = counts.waiting + counts.scheduled;
  const queued = lanes.queued.slice(0, SHOW_QUEUED);
  const running = lanes.running.slice(0, SHOW_RUNNING);
  const done = lanes.done.slice(0, SHOW_DONE);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm tabular-nums">
          <b className="text-foreground font-semibold">
            {queuedTotal.toLocaleString()}
          </b>{" "}
          queued
          {counts.scheduled > 0
            ? ` (${counts.scheduled.toLocaleString()} retrying)`
            : ""}{" "}
          ·{" "}
          <b className="text-foreground font-semibold">
            {counts.running.toLocaleString()}
          </b>{" "}
          running ·{" "}
          <b className="text-foreground font-semibold">
            {counts.done.toLocaleString()}
          </b>{" "}
          finished recently
        </p>
        <div
          role="group"
          aria-label="Filter queues"
          className="flex flex-wrap gap-1.5"
        >
          {QUEUE_FILTERS.map((f) => (
            <Button
              key={f.key}
              type="button"
              size="sm"
              variant={filter === f.key ? "secondary" : "ghost"}
              aria-pressed={filter === f.key}
              onClick={() => setFilter(f.key)}
              className={cn(
                "h-8 gap-1.5 rounded-full px-3",
                filter === f.key && "ring-border ring-1",
              )}
            >
              {f.label}
              <span className="text-muted-foreground text-xs tabular-nums">
                {lanes.byFilter[f.key].toLocaleString()}
              </span>
            </Button>
          ))}
        </div>
      </div>

      <div
        ref={board}
        className="relative grid grid-cols-1 items-start gap-3 lg:grid-cols-3"
      >
        <Lane
          title="Queued"
          count={queuedTotal}
          empty="Nothing waiting."
          footer={
            queuedTotal > queued.length
              ? `+${(queuedTotal - queued.length).toLocaleString()} more waiting`
              : null
          }
        >
          {queued.map((j) => (
            <JobCardRow key={j.task_id} job={j} now={now} />
          ))}
        </Lane>
        <Lane
          title="Running"
          count={counts.running}
          empty="Idle. Workers pick jobs up as they arrive."
          className="max-lg:order-first"
          footer={
            counts.running > running.length
              ? `+${(counts.running - running.length).toLocaleString()} more running`
              : null
          }
        >
          {running.map((j) => (
            <JobCardRow key={j.task_id} job={j} now={now} reduced={reduced} />
          ))}
        </Lane>
        <Lane
          title="Done"
          count={counts.done}
          empty="Nothing finished since the server started."
          footer={
            counts.failed > 0 ? (
              <span className="flex flex-wrap items-center gap-x-2">
                <span className="text-destructive">
                  {counts.failed} failed recently
                </span>
                <button
                  type="button"
                  onClick={onShowFailed}
                  className="text-foreground underline-offset-4 hover:underline"
                >
                  See failed jobs
                </button>
              </span>
            ) : null
          }
        >
          {done.map((j) => (
            <JobCardRow key={j.task_id} job={j} now={now} />
          ))}
        </Lane>
      </div>
    </div>
  );
}

function Lane({
  title,
  count,
  empty,
  footer,
  className,
  children,
}: {
  title: string;
  count: number;
  empty: string;
  footer: React.ReactNode;
  className?: string;
  children: React.ReactNode[];
}) {
  const id = React.useId();
  return (
    <section
      aria-labelledby={id}
      className={cn("bg-muted/30 min-w-0 rounded-xl border p-3", className)}
    >
      <h3
        id={id}
        className="mb-2.5 flex items-baseline justify-between gap-2 px-0.5 text-sm font-semibold"
      >
        {title}
        <span className="text-muted-foreground text-xs font-medium tabular-nums">
          {count.toLocaleString()}
        </span>
      </h3>
      {children.length === 0 ? (
        <p className="text-muted-foreground px-0.5 py-6 text-center text-sm">
          {empty}
        </p>
      ) : (
        <ul className="space-y-2">{children}</ul>
      )}
      {footer ? (
        <div className="text-muted-foreground mt-2.5 px-0.5 text-xs tabular-nums">
          {footer}
        </div>
      ) : null}
    </section>
  );
}

function targetHref(t: JobTarget): string | null {
  if (t.kind === "issue" && t.series_slug && t.issue_slug)
    return issueUrl(t.series_slug, t.issue_slug);
  if (t.kind === "series" && t.series_slug) return seriesUrl(t.series_slug);
  if (t.kind === "library" && t.library) return adminLibraryUrl(t.library.slug);
  return null;
}

function JobCardRow({
  job,
  now,
  reduced,
}: {
  job: JobCard;
  now: number;
  reduced?: boolean;
}) {
  const t = job.target ?? null;
  const href = t ? targetHref(t) : null;
  const failed = job.state === "done" && job.ok === false;
  const title = t?.title ?? job.queue.replace(/_/g, " ");
  return (
    <li
      data-flip={job.task_id}
      className={cn(
        "bg-card flex min-w-0 gap-3 rounded-lg border p-2.5 shadow-xs",
        failed && "border-destructive/50",
      )}
    >
      <Thumb target={t} />
      <div className="min-w-0 flex-1">
        <p className="text-muted-foreground flex items-center justify-between gap-2 text-xs">
          <span className="truncate">{job.label}</span>
          <StateMark job={job} />
        </p>
        <p className="mt-0.5 truncate text-sm font-medium" title={title}>
          {href ? (
            <Link href={href} className="hover:underline">
              {title}
            </Link>
          ) : (
            title
          )}
        </p>
        <p className="text-muted-foreground mt-0.5 truncate text-xs tabular-nums">
          {meta(job, now)}
        </p>
        {job.state === "running" ? (
          <div
            className="bg-muted mt-2 h-1 overflow-hidden rounded-full"
            aria-hidden
          >
            <div
              className={cn(
                "bg-primary h-full w-1/3 rounded-full",
                job.started_at && !reduced
                  ? "queue-indeterminate"
                  : "opacity-40",
              )}
            />
          </div>
        ) : null}
        {failed && job.error ? (
          <p
            className="text-destructive mt-1 line-clamp-2 text-xs break-words"
            title={job.error}
          >
            {job.error}
          </p>
        ) : null}
      </div>
    </li>
  );
}

function meta(job: JobCard, now: number): string {
  const lib = job.target?.library?.name;
  const parts: string[] = [];
  if (lib && job.target?.kind !== "library") parts.push(lib);
  switch (job.state) {
    case "waiting":
      parts.push("waiting");
      break;
    case "scheduled": {
      const at = job.next_run_at ? Date.parse(job.next_run_at) : null;
      parts.push(
        `retry${job.attempt ? ` ${job.attempt + 1}` : ""}${
          at ? ` in ${formatDuration(Math.max(0, at - now))}` : ""
        }`,
      );
      break;
    }
    case "running":
      parts.push(
        job.started_at
          ? `running ${formatDuration(Math.max(0, now - Date.parse(job.started_at)))}`
          : "claimed, starting",
      );
      break;
    case "done":
      parts.push(
        `${job.ok === false ? "failed" : "done"} in ${formatDuration(job.duration_ms)}`,
      );
      break;
  }
  return parts.join(" · ");
}

function StateMark({ job }: { job: JobCard }) {
  if (job.state === "done")
    return job.ok === false ? (
      <X aria-label="Failed" className="text-destructive size-3.5 shrink-0" />
    ) : (
      <Check
        aria-label="Succeeded"
        className="text-success size-3.5 shrink-0"
      />
    );
  if (job.state === "scheduled")
    return (
      <RotateCcw aria-label="Retry scheduled" className="size-3.5 shrink-0" />
    );
  if (job.state === "waiting")
    return <Clock aria-label="Waiting" className="size-3.5 shrink-0" />;
  return null;
}

function Thumb({ target }: { target: JobTarget | null }) {
  if (target?.cover_url)
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={target.cover_url}
        alt=""
        loading="lazy"
        className="bg-muted h-[54px] w-9 shrink-0 rounded object-cover"
      />
    );
  const Icon =
    target?.kind === "library"
      ? Library
      : target?.kind === "series"
        ? FolderSearch
        : Hourglass;
  return (
    <div
      aria-hidden
      className="bg-muted text-muted-foreground grid h-[54px] w-9 shrink-0 place-items-center rounded"
    >
      <Icon className="size-4" />
    </div>
  );
}

/** Wall clock that ticks each second while `active` (running elapsed). */
function useNow(active: boolean): number {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (!active) return;
    // First tick right away (in a callback, not the effect body), then
    // once a second.
    const first = window.setTimeout(() => setNow(Date.now()), 0);
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(id);
    };
  }, [active]);
  return now;
}

/** Layout position relative to `root`, ignoring in-flight transforms. */
function offsetIn(el: HTMLElement, root: HTMLElement) {
  let x = 0;
  let y = 0;
  let n: HTMLElement | null = el;
  while (n && n !== root) {
    x += n.offsetLeft;
    y += n.offsetTop;
    n = n.offsetParent as HTMLElement | null;
  }
  return { x, y };
}

/**
 * FLIP across lanes. Cards are keyed by task id, so a job that moves lanes
 * is a new DOM node; positions are remembered by key instead, and the new
 * node glides from where the old one was. Cards without a previous spot
 * pop in. Measured with offsets (not getBoundingClientRect) so a card
 * mid-glide doesn't poison the next measurement. Animates only when
 * `version` changes (new data or filter), never on the per-second tick.
 */
function useFlip(
  root: React.RefObject<HTMLDivElement | null>,
  version: string,
  disabled: boolean,
) {
  const spots = React.useRef(new Map<string, { x: number; y: number }>());
  const seen = React.useRef<string | null>(null);
  React.useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    const nodes = [...el.querySelectorAll<HTMLElement>("[data-flip]")];
    const next = new Map<string, { x: number; y: number }>();
    for (const n of nodes) next.set(n.dataset.flip!, offsetIn(n, el));
    const changed = seen.current !== null && seen.current !== version;
    if (changed && !disabled && spots.current.size > 0) {
      let budget = MAX_ANIMATED;
      for (const n of nodes) {
        if (budget <= 0) break;
        const key = n.dataset.flip!;
        const cur = next.get(key)!;
        const prev = spots.current.get(key);
        if (prev) {
          const dx = prev.x - cur.x;
          const dy = prev.y - cur.y;
          if (Math.abs(dx) + Math.abs(dy) < 1) continue;
          budget--;
          n.animate(
            [
              { transform: `translate(${dx}px, ${dy}px)` },
              { transform: "none" },
            ],
            { duration: GLIDE_MS, easing: EASE },
          );
        } else {
          budget--;
          n.animate(
            [
              { opacity: 0, transform: "scale(0.9) translateY(6px)" },
              { opacity: 1, transform: "none" },
            ],
            { duration: POP_MS, easing: EASE },
          );
        }
      }
    }
    spots.current = next;
    if (el.isConnected && nodes.length) seen.current = version;
  });
}
