"use client";

import Link from "next/link";
import { ArrowRight, CheckCircle2, Clock, Loader2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { useBackgroundWork } from "@/lib/api/queries";
import {
  otherWork,
  pct,
  phaseLabel,
  sortLibraries,
} from "@/lib/admin/background-work";
import { statusTone, statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";
import type {
  BackgroundWorkView,
  LibraryWorkView,
  MetadataBatchWorkView,
} from "@/lib/api/types";

const n = (v: number) => v.toLocaleString("en-US");

/**
 * The one place that shows everything in flight: per-library scan +
 * thumbnail + hashing state, every other queue, and unfinished metadata
 * batches. The header pill links here. Each row links onward to the surface
 * that owns the detail (a library's Live scan page, the queue page, …).
 */
export function BackgroundWorkClient() {
  const { data, isLoading, isError } = useBackgroundWork();

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading background work…
      </div>
    );
  }
  if (isError || !data) {
    return (
      <p className="text-destructive text-sm">
        Failed to load background work.
      </p>
    );
  }

  return (
    <div className="space-y-6">
      <Summary view={data} />
      <LibrariesCard libraries={sortLibraries(data.libraries)} />
      <OtherWorkCard view={data} />
    </div>
  );
}

function Summary({ view }: { view: BackgroundWorkView }) {
  const t = view.totals;
  return (
    <div className="space-y-3">
      <p
        className={cn(
          "flex items-center gap-2 text-sm font-medium",
          t.busy ? statusToneText("warning") : statusToneText("success"),
        )}
        role="status"
      >
        {t.busy ? (
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
        ) : (
          <CheckCircle2 className="h-4 w-4" aria-hidden="true" />
        )}
        {t.busy ? "Work in progress" : "Idle — nothing is running or queued"}
      </p>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile
          label="Scans"
          value={n(t.scans_running + t.scans_queued)}
          detail={`${n(t.scans_running)} running · ${n(t.scans_queued)} queued`}
        />
        <Tile
          label="Covers to generate"
          value={n(t.covers_remaining)}
          detail={
            t.hash_pending > 0
              ? `${n(t.hash_pending)} files still to hash`
              : "across all libraries"
          }
        />
        <Tile
          label="Jobs pending"
          value={n(t.jobs_outstanding)}
          detail={`${n(t.jobs_in_flight)} with workers`}
          href="/admin/queue"
        />
        <Tile
          label="Failed jobs"
          value={n(t.jobs_dead)}
          detail={t.jobs_dead > 0 ? "Review and retry" : "None"}
          href="/admin/queue?tab=failed"
          tone={t.jobs_dead > 0 ? "error" : undefined}
        />
      </div>
    </div>
  );
}

function Tile({
  label,
  value,
  detail,
  href,
  tone,
}: {
  label: string;
  value: string;
  detail: string;
  href?: string;
  tone?: "error";
}) {
  const body = (
    <Card
      className={cn(
        "h-full",
        href && "hover:bg-muted/40 transition-colors",
        tone === "error" && "border-destructive/40",
      )}
    >
      <CardHeader className="pb-1">
        <CardTitle className="text-muted-foreground text-xs font-medium">
          {label}
        </CardTitle>
      </CardHeader>
      <CardContent>
        <div
          className={cn(
            "text-2xl font-semibold tabular-nums",
            tone === "error" && "text-destructive",
          )}
        >
          {value}
        </div>
        <p className="text-muted-foreground mt-0.5 text-xs tabular-nums">
          {detail}
        </p>
      </CardContent>
    </Card>
  );
  return href ? (
    <Link
      href={href}
      className="focus-visible:ring-ring rounded-xl focus-visible:ring-2 focus-visible:outline-none"
    >
      {body}
    </Link>
  ) : (
    body
  );
}

function LibrariesCard({ libraries }: { libraries: LibraryWorkView[] }) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Libraries</CardTitle>
        <p className="text-muted-foreground text-xs">
          Scan, cover and hashing progress per library. Open a library for its
          live scan detail.
        </p>
      </CardHeader>
      <CardContent>
        {libraries.length === 0 ? (
          <p className="text-muted-foreground text-sm">No libraries yet.</p>
        ) : (
          <ul className="divide-border divide-y">
            {libraries.map((lib) => (
              <LibraryRow key={lib.id} lib={lib} />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function LibraryRow({ lib }: { lib: LibraryWorkView }) {
  const coversDone = Math.min(lib.covers_ready, lib.issues_total);
  const coverJobs = lib.cover_jobs_queued + lib.cover_jobs_running;
  const pageJobs = lib.page_jobs_queued + lib.page_jobs_running;
  return (
    <li>
      <Link
        href={`/admin/libraries/${lib.slug}/scan`}
        className={cn(
          "hover:bg-muted/40 focus-visible:ring-ring -mx-2 grid gap-x-6 gap-y-3 rounded-md px-2 py-3 transition-colors focus-visible:ring-2 focus-visible:outline-none md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)_minmax(0,1.4fr)_auto]",
          !lib.busy && "opacity-70",
        )}
        aria-label={`${lib.name}: open live scan`}
      >
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{lib.name}</div>
          <div className="text-muted-foreground text-xs tabular-nums">
            {n(lib.issues_total)} issues
          </div>
        </div>

        <ScanCell lib={lib} />

        <div className="min-w-0 space-y-1">
          <div className="flex items-baseline justify-between gap-2 text-xs">
            <span className="text-muted-foreground">Covers</span>
            <span className="tabular-nums">
              {n(coversDone)} / {n(lib.issues_total)}
            </span>
          </div>
          <Progress
            value={
              lib.issues_total === 0 ? 100 : pct(coversDone, lib.issues_total)
            }
            aria-label={`${lib.name} covers ready`}
          />
          <p className="text-muted-foreground truncate text-xs tabular-nums">
            {coverLine(lib, coverJobs, pageJobs)}
          </p>
        </div>

        <ArrowRight
          className="text-muted-foreground hidden h-4 w-4 self-center md:block"
          aria-hidden="true"
        />
      </Link>
    </li>
  );
}

function coverLine(
  lib: LibraryWorkView,
  coverJobs: number,
  pageJobs: number,
): string {
  const parts: string[] = [];
  if (lib.covers_remaining > 0) {
    parts.push(
      coverJobs > 0
        ? `${n(lib.cover_jobs_running)} running · ${n(lib.cover_jobs_queued)} queued`
        : `${n(lib.covers_remaining)} to do, none queued`,
    );
  } else {
    parts.push("Covers ready");
  }
  if (lib.covers_hash_only > 0) {
    parts.push(`${n(lib.covers_hash_only)} need only a cover hash`);
  }
  if (pageJobs > 0) parts.push(`${n(pageJobs)} page-thumbnail jobs`);
  if (lib.hash_pending > 0) parts.push(`${n(lib.hash_pending)} to hash`);
  if (lib.covers_errored > 0) parts.push(`${n(lib.covers_errored)} errored`);
  return parts.join(" · ");
}

function ScanCell({ lib }: { lib: LibraryWorkView }) {
  const scan = lib.scan;
  if (!scan) {
    return (
      <div className="text-muted-foreground min-w-0 self-center text-xs">
        {lib.scoped_scans > 0
          ? `${n(lib.scoped_scans)} series/issue ${lib.scoped_scans === 1 ? "scan" : "scans"} in flight`
          : "No scan running"}
      </div>
    );
  }
  const running = scan.state === "running";
  const total = scan.total ?? 0;
  const completed = scan.completed ?? 0;
  return (
    <div className="min-w-0 space-y-1">
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="flex min-w-0 items-center gap-1.5">
          <Badge
            variant="outline"
            className={cn(
              "gap-1 text-[10px] tracking-wider uppercase",
              statusTone(running ? "info" : "neutral"),
            )}
          >
            {running ? (
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
            ) : (
              <Clock className="h-3 w-3" aria-hidden="true" />
            )}
            {running ? "Scanning" : "Queued"}
          </Badge>
          {running ? (
            <span className="text-muted-foreground truncate">
              {phaseLabel(scan.phase)}
            </span>
          ) : null}
        </span>
        {running && total > 0 ? (
          <span className="tabular-nums">
            {n(completed)} / {n(total)}
          </span>
        ) : null}
      </div>
      {running ? (
        <Progress
          value={pct(completed, total)}
          aria-label={`${lib.name} scan progress`}
        />
      ) : null}
      <p className="text-muted-foreground truncate text-xs">
        {running
          ? (scan.current_label ?? "Working…")
          : "Waiting for a scan worker"}
        {scan.batch_id ? " · part of a Scan all" : ""}
        {lib.scoped_scans > 0 ? ` · +${n(lib.scoped_scans)} series/issue` : ""}
      </p>
    </div>
  );
}

function OtherWorkCard({ view }: { view: BackgroundWorkView }) {
  const rows = otherWork(view);
  const batches = view.metadata_batches;
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Other work</CardTitle>
        <p className="text-muted-foreground text-xs">
          Metadata, archive and maintenance jobs that are not tied to one
          library&apos;s scan.
        </p>
      </CardHeader>
      <CardContent className="space-y-4">
        {rows.length === 0 && batches.length === 0 ? (
          <p className="text-muted-foreground text-sm">Nothing queued.</p>
        ) : null}
        {rows.length > 0 ? (
          <ul className="divide-border divide-y">
            {rows.map((r) => (
              <li key={r.queue}>
                <Link
                  href={r.href}
                  className="hover:bg-muted/40 focus-visible:ring-ring -mx-2 flex items-center justify-between gap-4 rounded-md px-2 py-2 text-sm transition-colors focus-visible:ring-2 focus-visible:outline-none"
                >
                  <span className="truncate">{r.label}</span>
                  <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                    <span className="text-foreground font-medium">
                      {n(r.pending)}
                    </span>{" "}
                    pending
                    {r.inFlight > 0 ? ` · ${n(r.inFlight)} with workers` : ""}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        ) : null}
        {batches.length > 0 ? (
          <div className="space-y-2">
            <h3 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Metadata batches
            </h3>
            <ul className="space-y-3">
              {batches.map((b) => (
                <BatchRow key={b.id} batch={b} />
              ))}
            </ul>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

const BATCH_SCOPE: Record<string, string> = {
  series_issues: "Series issues",
  saved_view: "Saved view",
  library_refresh: "Library refresh",
};

function BatchRow({ batch }: { batch: MetadataBatchWorkView }) {
  const waiting = batch.status === "awaiting_quota";
  return (
    <li>
      <Link
        href="/admin/metadata?tab=review"
        className="hover:bg-muted/40 focus-visible:ring-ring -mx-2 block space-y-1 rounded-md px-2 py-1.5 transition-colors focus-visible:ring-2 focus-visible:outline-none"
      >
        <div className="flex items-baseline justify-between gap-2 text-xs">
          <span className="truncate text-sm">
            {BATCH_SCOPE[batch.scope] ?? batch.scope}
            {waiting ? (
              <span className={cn("ml-2 text-xs", statusToneText("warning"))}>
                waiting on provider quota
              </span>
            ) : null}
          </span>
          <span className="shrink-0 tabular-nums">
            {n(batch.items_finished)} / {n(batch.items_total)}
          </span>
        </div>
        <Progress
          value={pct(batch.items_finished, batch.items_total)}
          aria-label="Metadata batch progress"
        />
      </Link>
    </li>
  );
}
