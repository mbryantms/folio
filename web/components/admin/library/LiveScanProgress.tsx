"use client";

import * as React from "react";
import Link from "next/link";
import {
  Activity,
  AlertCircle,
  CheckCircle2,
  FileArchive,
  HeartPulse,
  Loader2,
  Pause,
  Play,
  RotateCw,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  useClearQueue,
  useDeleteAllThumbnails,
  useForceRecreateThumbnails,
  useGenerateMissingThumbnails,
  useGeneratePageMapThumbnails,
  useTriggerScan,
} from "@/lib/api/mutations";
import {
  useHealthIssues,
  useLibrary,
  useScanRuns,
  useThumbnailsSettings,
  useThumbnailsStatus,
} from "@/lib/api/queries";
import { useScanEvents } from "@/lib/api/scan-events";
import type { HealthIssueView, ScanEvent, ScanRunView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { ScanModeMenu } from "./ScanModeMenu";

type Status =
  | "idle"
  | "queued"
  | "running"
  | "thumbnailing"
  | "completed"
  | "failed";
type ProgressEvent = Extract<ScanEvent, { type: "scan.progress" }>;
type HealthItem = {
  kind: string;
  severity: string;
  path: string | null;
  scanId: string;
};

type LiveState = {
  scanId: string | null;
  status: Status;
  progress: ProgressEvent | null;
  recentSeries: string[];
  health: HealthItem[];
  severityCounts: { error: number; warning: number; info: number };
};

type LiveAction =
  | { type: "hydrate"; run: ScanRunView | undefined }
  | { type: "events"; events: ScanEvent[] }
  | { type: "healthRows"; scanId: string; rows: HealthIssueView[] };

const emptyCounts = { error: 0, warning: 0, info: 0 };

function initialState(run?: ScanRunView): LiveState {
  return {
    scanId: run?.id ?? null,
    status: statusFromRun(run),
    progress: progressFromRun(run),
    recentSeries: [],
    health: [],
    severityCounts: { ...emptyCounts },
  };
}

export function liveScanReducer(
  state: LiveState,
  action: LiveAction,
): LiveState {
  switch (action.type) {
    case "hydrate": {
      const run = action.run;
      if (!run) return state.scanId ? state : initialState(undefined);
      if (
        state.scanId &&
        state.scanId !== run.id &&
        state.status === "running"
      ) {
        return state;
      }
      if (state.scanId === run.id && state.progress) {
        return { ...state, status: statusFromRun(run) };
      }
      return initialState(run);
    }
    case "events":
      return action.events.reduce(reduceEvent, state);
    case "healthRows": {
      if (!state.scanId || state.scanId !== action.scanId) return state;
      return mergeHealthItems(
        state,
        action.rows
          .filter((row) => row.scan_id === action.scanId)
          .filter((row) => !row.resolved_at && !row.dismissed_at)
          .map((row) => ({
            kind: row.kind,
            severity: row.severity,
            path: payloadPath(row.payload),
            scanId: action.scanId,
          })),
      );
    }
    default:
      return state;
  }
}

function reduceEvent(state: LiveState, evt: ScanEvent): LiveState {
  switch (evt.type) {
    case "scan.started":
      return {
        scanId: evt.scan_id,
        status: "running",
        progress: null,
        recentSeries: [],
        health: [],
        severityCounts: { ...emptyCounts },
      };
    case "scan.progress":
      if (state.scanId && state.scanId !== evt.scan_id) return state;
      return {
        ...state,
        scanId: evt.scan_id,
        status: evt.phase === "complete" ? "completed" : "running",
        progress: evt,
        recentSeries:
          evt.phase === "scanning" &&
          evt.current_label &&
          evt.current_label !== "Scanning files"
            ? appendRecentSeries(state.recentSeries, evt.current_label)
            : state.recentSeries,
      };
    case "scan.series_updated": {
      return {
        ...state,
        recentSeries: appendRecentSeries(state.recentSeries, evt.name),
      };
    }
    case "scan.health_issue": {
      if (state.scanId && state.scanId !== evt.scan_id) return state;
      return mergeHealthItems(
        {
          ...state,
          scanId: evt.scan_id,
        },
        [
          {
            kind: evt.kind,
            severity: evt.severity,
            path: evt.path,
            scanId: evt.scan_id,
          },
        ],
      );
    }
    case "scan.completed":
      if (state.scanId && state.scanId !== evt.scan_id) return state;
      return {
        ...state,
        scanId: evt.scan_id,
        status: "completed",
        progress: state.progress
          ? {
              ...state.progress,
              phase: "complete",
              completed: state.progress.total,
            }
          : state.progress,
      };
    case "scan.failed":
      if (state.scanId && state.scanId !== evt.scan_id) return state;
      return { ...state, scanId: evt.scan_id, status: "failed" };
    default:
      return state;
  }
}

export function LiveScanProgress({ libraryId }: { libraryId: string }) {
  const library = useLibrary(libraryId);
  const runs = useScanRuns(libraryId);
  const healthIssues = useHealthIssues(libraryId);
  const thumbnailSettings = useThumbnailsSettings(libraryId);
  const thumbnailStatus = useThumbnailsStatus(libraryId, { intervalMs: 2_000 });
  const trigger = useTriggerScan(libraryId);
  const eventLibraryId = library.data?.id ?? "__loading-library-id__";
  const { status: wsStatus, events } = useScanEvents({
    libraryId: eventLibraryId,
    maxBuffer: 500,
  });
  const lastRun = runs.data?.[0];
  const [state, dispatch] = React.useReducer(
    liveScanReducer,
    lastRun,
    initialState,
  );
  const consumedKeys = React.useRef<Set<string>>(new Set());
  const thumbnailRefreshKeys = React.useRef<Set<string>>(new Set());

  React.useEffect(() => {
    dispatch({ type: "hydrate", run: lastRun });
  }, [lastRun]);

  React.useEffect(() => {
    const fresh: ScanEvent[] = [];
    for (const evt of events) {
      if (evt.type === "scan.started") consumedKeys.current.clear();
      const key = eventKey(evt);
      if (consumedKeys.current.has(key)) continue;
      consumedKeys.current.add(key);
      fresh.push(evt);
    }
    if (fresh.length > 0) {
      if (consumedKeys.current.size > 1000)
        consumedKeys.current = new Set([...consumedKeys.current].slice(-500));
      dispatch({ type: "events", events: fresh });
    }
  }, [events]);

  React.useEffect(() => {
    if (!state.scanId || !healthIssues.data?.length) return;
    dispatch({
      type: "healthRows",
      scanId: state.scanId,
      rows: healthIssues.data,
    });
  }, [healthIssues.data, state.scanId]);

  React.useEffect(() => {
    let shouldRefetch = false;
    for (const evt of events) {
      const key = eventKey(evt);
      if (thumbnailRefreshKeys.current.has(key)) continue;
      if (
        evt.type === "thumbs.started" ||
        evt.type === "thumbs.completed" ||
        evt.type === "thumbs.failed" ||
        evt.type === "scan.completed" ||
        (evt.type === "scan.progress" && evt.phase === "enqueueing_thumbnails")
      ) {
        thumbnailRefreshKeys.current.add(key);
        shouldRefetch = true;
      }
    }
    if (shouldRefetch) void thumbnailStatus.refetch();
    if (thumbnailRefreshKeys.current.size > 1000) {
      thumbnailRefreshKeys.current = new Set(
        [...thumbnailRefreshKeys.current].slice(-500),
      );
    }
  }, [events, thumbnailStatus]);

  const progress =
    state.status === "completed" && state.progress
      ? {
          ...state.progress,
          phase: "complete",
          unit:
            state.progress.unit === "planning" ? "work" : state.progress.unit,
          completed: state.progress.total,
        }
      : state.progress;
  const determinate = Boolean(
    state.status === "completed" ||
      (progress && progress.unit !== "planning" && progress.total > 0),
  );
  const pct = determinate
    ? state.status === "completed" && !progress
      ? 100
      : Math.min(
          100,
          Math.round(
            ((progress?.completed ?? 0) / (progress?.total ?? 1)) * 100,
          ),
        )
    : 0;
  const metricValues = metricsFromProgress(progress);
  const thumbnailData = thumbnailStatus.data;
  const thumbnailsEnabled = thumbnailSettings.data?.enabled ?? true;
  const coversPending = Boolean(
    thumbnailsEnabled &&
      thumbnailData &&
      thumbnailData.total > 0 &&
      thumbnailData.cover_generated < thumbnailData.total,
  );
  const thumbnailQueueActive = (thumbnailData?.in_flight ?? 0) > 0;
  const displayStatus: Status =
    state.status === "completed" && (coversPending || thumbnailQueueActive)
      ? "thumbnailing"
      : state.status;
  const displayPct =
    displayStatus === "thumbnailing" && thumbnailData && thumbnailData.total > 0
      ? Math.round((thumbnailData.cover_generated / thumbnailData.total) * 100)
      : pct;
  const displayRight =
    displayStatus === "thumbnailing" && thumbnailData
      ? thumbnailData.total > 0
        ? `${thumbnailData.cover_generated} / ${thumbnailData.total} covers`
        : `${thumbnailData.in_flight} queued`
      : state.status === "completed" && !progress
        ? "Complete"
        : determinate && progress
          ? `${progress.completed} / ${progress.total}`
          : "Planning";
  const displayLabel =
    displayStatus === "thumbnailing"
      ? coversPending
        ? "Generating cover thumbnails"
        : "Draining thumbnail queue"
      : (progress?.current_label ?? scanLabel(state.status));

  return (
    <div className="space-y-6">
      <Card>
        <CardContent className="flex flex-wrap items-center gap-4 p-5">
          <StatusPill status={displayStatus} />
          <div className="text-muted-foreground ml-auto flex items-center gap-2 text-xs">
            <span
              className={cn(
                "h-2 w-2 rounded-full",
                wsStatus === "open" ? "bg-emerald-400" : "bg-yellow-500",
              )}
            />
            WebSocket {wsStatus}
          </div>
          <ScanModeMenu
            disabled={state.status === "queued"}
            isPending={trigger.isPending}
            isRunning={state.status === "running"}
            onScan={(mode) => trigger.mutate({ mode })}
          />
        </CardContent>
      </Card>

      <Card>
        <CardContent className="space-y-4 p-5">
          <div className="flex flex-wrap items-baseline justify-between gap-3">
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-foreground text-sm font-semibold tracking-tight">
                  {phaseLabel(progress?.phase, displayStatus)}
                </h3>
                {progress?.kind && displayStatus !== "thumbnailing" ? (
                  <Badge variant="secondary" className="rounded-md capitalize">
                    {progress.kind}
                  </Badge>
                ) : null}
              </div>
              <p className="text-muted-foreground mt-1 text-xs">
                {displayLabel}
              </p>
            </div>
            <span className="text-muted-foreground text-xs tabular-nums">
              {displayRight}
            </span>
          </div>
          {determinate || displayStatus === "idle" ? (
            <Progress value={displayPct} />
          ) : (
            <div className="bg-muted h-2 w-full overflow-hidden rounded-full">
              <div className="bg-primary h-full w-1/3 animate-pulse rounded-full" />
            </div>
          )}
          <PhaseChips phase={progress?.phase} status={displayStatus} />
          <ScanRuntimeStrip progress={progress} />
        </CardContent>
      </Card>

      <dl className="grid grid-cols-2 gap-3 text-xs md:grid-cols-4">
        <Metric
          icon={Activity}
          label="Series scanned"
          value={metricValues.series}
        />
        <Metric
          icon={FileArchive}
          label="Files seen"
          value={metricValues.seen}
        />
        <Metric icon={CheckCircle2} label="Added" value={metricValues.added} />
        <Metric icon={RotateCw} label="Updated" value={metricValues.updated} />
        <Metric icon={Pause} label="Unchanged" value={metricValues.unchanged} />
        <Metric
          icon={AlertCircle}
          label="Removed"
          value={metricValues.removed}
        />
        <Metric
          icon={FileArchive}
          label="Duplicates"
          value={metricValues.duplicates}
        />
        <Metric
          icon={HeartPulse}
          label="Health issues"
          value={metricValues.health}
        />
      </dl>

      <Tabs defaultValue="activity" className="space-y-4">
        <TabsList className="flex w-full justify-start overflow-x-auto md:w-fit">
          <TabsTrigger value="activity">Activity</TabsTrigger>
          <TabsTrigger value="health">Health</TabsTrigger>
          <TabsTrigger value="thumbnails">Thumbnails</TabsTrigger>
          <TabsTrigger value="events">Events</TabsTrigger>
        </TabsList>
        <TabsContent value="activity">
          <RecentActivityCard recentSeries={state.recentSeries} />
        </TabsContent>
        <TabsContent value="health">
          <HealthSummaryCard
            libraryId={libraryId}
            health={state.health}
            severityCounts={state.severityCounts}
          />
        </TabsContent>
        <TabsContent value="thumbnails">
          <ThumbnailWorkPanel
            libraryId={libraryId}
            liveEvents={events}
            wsStatus={wsStatus}
          />
        </TabsContent>
        <TabsContent value="events">
          <RawEventsCard events={events} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function ScanRuntimeStrip({ progress }: { progress: ProgressEvent | null }) {
  if (!progress) return null;
  const rows = [
    ["Elapsed", formatDuration(progress.elapsed_ms)],
    ["Phase", formatDuration(progress.phase_elapsed_ms)],
    ["Files/sec", formatRate(progress.files_per_sec)],
    ["Bytes/sec", formatBytesRate(progress.bytes_per_sec)],
    [
      "Workers",
      progress.active_workers ? String(progress.active_workers) : "—",
    ],
    ["Skipped folders", String(progress.skipped_folders ?? 0)],
    ["ETA", formatDuration(progress.eta_ms)],
  ];
  return (
    <dl className="border-border grid gap-3 border-t pt-4 text-xs sm:grid-cols-2 lg:grid-cols-7">
      {rows.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <dt className="text-muted-foreground truncate text-[10px] font-medium tracking-widest uppercase">
            {label}
          </dt>
          <dd className="text-foreground truncate font-medium tabular-nums">
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function RecentActivityCard({ recentSeries }: { recentSeries: string[] }) {
  const latest = [...recentSeries].reverse().slice(0, 5);
  return (
    <Card>
      <CardContent className="space-y-3 p-5">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
            Recent activity
          </h3>
          <span className="text-muted-foreground text-xs">
            Latest {latest.length}
          </span>
        </div>
        {latest.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No series activity yet.
          </p>
        ) : (
          <ul className="divide-border divide-y font-mono text-xs">
            {latest.map((name, i) => (
              <li
                key={`${name}-${i}`}
                className="text-muted-foreground truncate py-2 first:pt-0 last:pb-0"
              >
                {name}
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function HealthSummaryCard({
  libraryId,
  health,
  severityCounts,
}: {
  libraryId: string;
  health: HealthItem[];
  severityCounts: LiveState["severityCounts"];
}) {
  const examples = health.slice(0, 5);
  return (
    <Card>
      <CardContent className="space-y-4 p-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
              Health
            </h3>
            <Link
              href={`/admin/libraries/${libraryId}/health`}
              className="text-primary text-xs font-medium hover:underline"
            >
              Open details
            </Link>
          </div>
          <div className="flex flex-wrap justify-end gap-1">
            <SeverityBadge
              label="Error"
              value={severityCounts.error}
              tone="error"
            />
            <SeverityBadge
              label="Warn"
              value={severityCounts.warning}
              tone="warning"
            />
            <SeverityBadge
              label="Info"
              value={severityCounts.info}
              tone="info"
            />
          </div>
        </div>
        {examples.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No health issues raised in this scan.
          </p>
        ) : (
          <ul className="space-y-1.5 text-xs">
            {examples.map((h, i) => (
              <li
                key={`${h.scanId}-${h.kind}-${h.path ?? ""}-${i}`}
                className="flex items-start gap-2"
              >
                <Badge
                  variant={h.severity === "error" ? "destructive" : "secondary"}
                  className="shrink-0 uppercase"
                >
                  {h.severity}
                </Badge>
                <Link
                  href={`/admin/libraries/${libraryId}/health`}
                  className="text-muted-foreground hover:text-foreground min-w-0 font-mono"
                >
                  <span className="text-foreground">{h.kind}</span>
                  {h.path ? ` - ${h.path}` : ""}
                </Link>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function RawEventsCard({ events }: { events: ScanEvent[] }) {
  const recent = [...events].slice(-25).reverse();
  return (
    <Card>
      <CardContent className="space-y-3 p-5">
        <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
          Developer event stream
        </h3>
        <ScrollArea className="border-border bg-background/40 h-72 rounded-md border p-3">
          {recent.length === 0 ? (
            <p className="text-muted-foreground text-xs">No events buffered.</p>
          ) : (
            <pre className="text-muted-foreground text-[11px] leading-relaxed whitespace-pre-wrap">
              {recent.map((event) => JSON.stringify(event)).join("\n")}
            </pre>
          )}
        </ScrollArea>
      </CardContent>
    </Card>
  );
}

function ThumbnailWorkPanel({
  libraryId,
  liveEvents,
  wsStatus,
}: {
  libraryId: string;
  liveEvents: ScanEvent[];
  wsStatus: string;
}) {
  const settings = useThumbnailsSettings(libraryId);
  const status = useThumbnailsStatus(libraryId);
  const generateMissing = useGenerateMissingThumbnails(libraryId);
  const generatePageMap = useGeneratePageMapThumbnails(libraryId);
  const forceRecreate = useForceRecreateThumbnails(libraryId);
  const deleteAll = useDeleteAllThumbnails(libraryId);
  const clearQueue = useClearQueue();
  const [confirmRecreate, setConfirmRecreate] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  const [confirmClearQueue, setConfirmClearQueue] = React.useState(false);

  const thumbEvents = React.useMemo(
    () =>
      liveEvents
        .filter((event) => event.type.startsWith("thumbs."))
        .slice(-25)
        .reverse(),
    [liveEvents],
  );

  if (settings.isLoading || status.isLoading) {
    return <Skeleton className="h-64 w-full" />;
  }
  if (settings.error || !settings.data || status.error || !status.data) {
    return (
      <Card>
        <CardContent className="text-destructive p-4 text-sm">
          Failed to load thumbnail work status.
        </CardContent>
      </Card>
    );
  }

  const d = status.data;
  const enabled = settings.data.enabled;
  const format = settings.data.format;
  const percent =
    d.total === 0 ? 100 : Math.round((d.cover_generated / d.total) * 100);
  const coverActive = enabled && d.total > 0 && d.cover_generated < d.total;
  const coversReady = d.total === 0 || d.cover_generated >= d.total;
  const pageTotal = d.page_total ?? 0;
  const pageGenerated = d.page_map_generated ?? 0;
  const pageMissing =
    d.page_map_missing ?? Math.max(0, pageTotal - pageGenerated);
  const pagePercent =
    pageTotal === 0 ? 100 : Math.round((pageGenerated / pageTotal) * 100);
  const pageActive =
    enabled && coversReady && pageTotal > 0 && pageGenerated < pageTotal;
  const anyMutating =
    generateMissing.isPending ||
    generatePageMap.isPending ||
    forceRecreate.isPending ||
    deleteAll.isPending ||
    clearQueue.isPending;

  return (
    <Card>
      <CardContent className="space-y-5 p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h3 className="text-foreground text-sm font-semibold tracking-tight">
              Cover and page thumbnail work
            </h3>
            <p className="text-muted-foreground mt-1 text-xs">
              Covers are scan-triggered first; page thumbnails are optional
              queue work after covers are ready.
            </p>
          </div>
          <Badge
            variant={enabled ? "secondary" : "outline"}
            className="rounded-md"
          >
            {enabled ? `Enabled · ${format.toUpperCase()}` : "Disabled"}
          </Badge>
        </div>

        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            disabled={!enabled || anyMutating || d.total === 0}
            onClick={() => generateMissing.mutate()}
          >
            {generateMissing.isPending ? "Enqueueing..." : "Generate missing"}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!enabled || anyMutating || d.total === 0 || !coversReady}
            onClick={() => generatePageMap.mutate()}
            title={
              coversReady
                ? "Queue page thumbnail jobs"
                : "Cover thumbnails must finish before page thumbnails"
            }
          >
            {generatePageMap.isPending
              ? "Queueing..."
              : "Queue page thumbnails"}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!enabled || anyMutating || d.total === 0}
            onClick={() => setConfirmRecreate(true)}
          >
            {forceRecreate.isPending ? "Enqueueing..." : "Force recreate all"}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={clearQueue.isPending || d.in_flight === 0}
            onClick={() => setConfirmClearQueue(true)}
          >
            {clearQueue.isPending ? "Clearing..." : "Clear thumbnail queue"}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={anyMutating || d.total === 0}
            onClick={() => setConfirmDelete(true)}
            className="text-destructive hover:text-destructive"
          >
            {deleteAll.isPending ? "Deleting..." : "Delete all thumbnails"}
          </Button>
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          <div
            className={cn(
              "flex min-h-32 flex-col justify-between rounded-md border p-4",
              coverActive
                ? "border-primary/40 bg-primary/5"
                : "border-border bg-background/50",
            )}
          >
            <div className="flex items-baseline justify-between gap-3">
              <div>
                <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
                  Cover readiness
                </p>
                <p className="text-muted-foreground text-xs">
                  Scan-triggered thumbnail work completes covers before optional
                  page thumbnails.
                </p>
              </div>
              <span className="text-muted-foreground text-sm tabular-nums">
                {percent}%
              </span>
            </div>
            <div className="space-y-3">
              <span className="text-foreground text-2xl font-semibold tabular-nums">
                {d.cover_generated}
                <span className="text-muted-foreground text-base">
                  {" "}
                  / {d.total}
                </span>
              </span>
              <Progress value={percent} />
            </div>
          </div>

          <div
            className={cn(
              "flex min-h-32 flex-col justify-between rounded-md border p-4",
              pageActive
                ? "border-primary/40 bg-primary/5"
                : "border-border bg-background/50",
            )}
          >
            <div className="flex items-baseline justify-between gap-3">
              <div>
                <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
                  Page thumbnail readiness
                </p>
                <p className="text-muted-foreground text-xs">
                  Reader page thumbnails are counted from generated strip files
                  after covers are ready.
                </p>
              </div>
              <span className="text-muted-foreground text-sm tabular-nums">
                {pagePercent}%
              </span>
            </div>
            <div className="space-y-3">
              <span className="text-foreground text-2xl font-semibold tabular-nums">
                {pageGenerated}
                <span className="text-muted-foreground text-base">
                  {" "}
                  / {pageTotal}
                </span>
              </span>
              <Progress value={pagePercent} />
              <p className="text-muted-foreground text-xs">
                {!coversReady && pageTotal > 0
                  ? "Waiting for covers to finish before page thumbnail work."
                  : pageTotal === 0
                    ? "No page count data available yet."
                    : pageMissing > 0
                      ? `${pageMissing} page thumbnails remaining.`
                      : "Page thumbnails ready."}
              </p>
            </div>
          </div>
        </div>

        <dl className="grid grid-cols-2 gap-3 text-xs sm:grid-cols-4">
          <SmallStat label="Missing covers" value={d.cover_missing} />
          <SmallStat label="Queued covers" value={d.cover_queued} />
          <SmallStat label="Missing pages" value={pageMissing} />
          <SmallStat label="Queued pages" value={d.page_map_queued} />
          <SmallStat
            label="Errored jobs"
            value={d.cover_failed}
            tone={d.cover_failed > 0 ? "destructive" : "default"}
          />
          <SmallStat
            label="Queue depth"
            value={d.in_flight}
            tone={d.in_flight > 0 ? "primary" : "default"}
          />
        </dl>

        <div className="space-y-1">
          <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
            Recent cover and page thumbnail events
          </p>
          {thumbEvents.length === 0 ? (
            <p className="text-muted-foreground text-xs">
              {d.in_flight > 0
                ? `Waiting for thumbnail events (${wsStatus})...`
                : "No recent thumbnail events."}
            </p>
          ) : (
            <ul className="border-border bg-background/50 divide-border divide-y rounded-md border">
              {thumbEvents.map((event, index) => (
                <li
                  key={`${event.type}-${"issue_id" in event ? event.issue_id : ""}-${index}`}
                  className="flex items-center gap-3 px-3 py-1.5 text-xs"
                >
                  <ThumbEventDot type={event.type} />
                  <ThumbKindBadge event={event} />
                  <span className="text-muted-foreground font-mono">
                    {thumbLabelFor(event)}
                  </span>
                  {"issue_id" in event ? (
                    <span className="text-muted-foreground/70 ml-auto truncate font-mono">
                      {event.issue_id.slice(0, 8)}...
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </div>
      </CardContent>

      <AlertDialog open={confirmRecreate} onOpenChange={setConfirmRecreate}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Recreate all thumbnails?</AlertDialogTitle>
            <AlertDialogDescription>
              Wipes the {d.total} cover and strip thumbnails for this library
              and enqueues fresh jobs in {format.toUpperCase()}.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={forceRecreate.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                forceRecreate.mutate(undefined, {
                  onSettled: () => setConfirmRecreate(false),
                })
              }
              disabled={forceRecreate.isPending}
            >
              {forceRecreate.isPending ? "Enqueueing..." : "Recreate"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete all thumbnails?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes every cover and strip thumbnail on disk for this library
              and clears the database state. New thumbnails will not be
              generated until you click Generate missing or run a scan
              {!enabled ? " after re-enabling generation" : ""}.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteAll.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                deleteAll.mutate(undefined, {
                  onSettled: () => setConfirmDelete(false),
                })
              }
              disabled={deleteAll.isPending}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteAll.isPending ? "Deleting..." : "Delete all"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={confirmClearQueue} onOpenChange={setConfirmClearQueue}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Clear thumbnail queue?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes pending cover and page-map thumbnail jobs. A thumbnail
              already being generated may still finish.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={clearQueue.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                clearQueue.mutate(
                  { target: "thumbnails" },
                  { onSettled: () => setConfirmClearQueue(false) },
                )
              }
              disabled={clearQueue.isPending}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {clearQueue.isPending ? "Clearing..." : "Clear queue"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function statusFromRun(run?: ScanRunView): Status {
  if (!run) return "idle";
  if (run.state === "running") return "running";
  if (run.state === "queued") return "queued";
  if (run.state === "complete" || run.state === "completed") return "completed";
  if (run.state === "failed") return "failed";
  return "idle";
}

function appendRecentSeries(current: string[], name: string): string[] {
  const next = current.filter((item) => item !== name);
  next.push(name);
  return next.slice(-80);
}

function mergeHealthItems(state: LiveState, items: HealthItem[]): LiveState {
  if (items.length === 0) return state;
  const existing = new Set(
    state.health.map(
      (h) => `${h.scanId}:${h.kind}:${h.severity}:${h.path ?? ""}`,
    ),
  );
  const fresh = items.filter((item) => {
    const key = `${item.scanId}:${item.kind}:${item.severity}:${item.path ?? ""}`;
    if (existing.has(key)) return false;
    existing.add(key);
    return true;
  });
  if (fresh.length === 0) return state;

  const health = [...fresh, ...state.health].slice(0, 100);
  const severityCounts = { ...state.severityCounts };
  for (const item of fresh) {
    const severity = item.severity as keyof LiveState["severityCounts"];
    if (severity in severityCounts) severityCounts[severity] += 1;
  }
  return { ...state, health, severityCounts };
}

function payloadPath(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const obj = payload as Record<string, unknown>;
  const data =
    obj.data && typeof obj.data === "object"
      ? (obj.data as Record<string, unknown>)
      : obj;
  for (const key of ["path", "file_path", "folder"]) {
    const value = data[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  const pathA = data.path_a;
  const pathB = data.path_b;
  if (typeof pathA === "string" && typeof pathB === "string") {
    return `${pathA} | ${pathB}`;
  }
  return null;
}

function progressFromRun(run?: ScanRunView): ProgressEvent | null {
  const stats = run?.stats;
  if (!run || !stats || typeof stats !== "object" || !("progress" in stats))
    return null;
  const progress = (stats as { progress?: unknown }).progress;
  if (!progress || typeof progress !== "object") return null;
  const p = progress as Record<string, unknown>;
  if (typeof p.completed !== "number" || typeof p.total !== "number")
    return null;
  return {
    type: "scan.progress",
    library_id: "",
    scan_id: run.id,
    kind: typeof p.kind === "string" ? p.kind : run.kind,
    phase: typeof p.phase === "string" ? p.phase : "scanning",
    unit: typeof p.unit === "string" ? p.unit : "work",
    completed: p.completed,
    total: p.total,
    current_label: typeof p.current_label === "string" ? p.current_label : null,
    files_seen: numberStat(stats, "files_seen"),
    files_added: numberStat(stats, "files_added"),
    files_updated: numberStat(stats, "files_updated"),
    files_unchanged: numberStat(stats, "files_unchanged"),
    files_skipped: numberStat(stats, "files_skipped"),
    files_duplicate: numberStat(stats, "files_duplicate"),
    issues_removed: numberStat(stats, "issues_removed"),
    health_issues: typeof p.health_issues === "number" ? p.health_issues : 0,
    series_scanned: typeof p.series_scanned === "number" ? p.series_scanned : 0,
    series_total: typeof p.series_total === "number" ? p.series_total : 0,
    series_skipped_unchanged:
      typeof p.series_skipped_unchanged === "number"
        ? p.series_skipped_unchanged
        : 0,
    files_total: typeof p.files_total === "number" ? p.files_total : 0,
    root_files: typeof p.root_files === "number" ? p.root_files : 0,
    empty_folders: typeof p.empty_folders === "number" ? p.empty_folders : 0,
  };
}

function numberStat(stats: unknown, key: string): number {
  if (!stats || typeof stats !== "object") return 0;
  const value = (stats as Record<string, unknown>)[key];
  return typeof value === "number" ? value : 0;
}

function metricsFromProgress(progress: ProgressEvent | null) {
  return {
    series: progress
      ? `${progress.series_scanned}/${progress.series_total}`
      : "0/0",
    seen: progress?.files_seen ?? 0,
    added: progress?.files_added ?? 0,
    updated: progress?.files_updated ?? 0,
    unchanged: progress?.files_unchanged ?? 0,
    removed: progress?.issues_removed ?? 0,
    duplicates: progress?.files_duplicate ?? 0,
    health: progress?.health_issues ?? 0,
  };
}

function formatDuration(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms)) return "—";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60_000).toFixed(1)}m`;
}

function formatRate(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return "—";
  return value >= 100 ? value.toFixed(0) : value.toFixed(1);
}

function formatBytesRate(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return "—";
  if (value < 1024) return `${value.toFixed(0)} B/s`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB/s`;
  if (value < 1024 * 1024 * 1024) {
    return `${(value / 1024 / 1024).toFixed(1)} MiB/s`;
  }
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} GiB/s`;
}

function eventKey(evt: ScanEvent): string {
  switch (evt.type) {
    case "scan.progress":
      return `${evt.type}:${evt.scan_id}:${evt.phase}:${evt.completed}:${evt.total}:${evt.current_label ?? ""}`;
    case "scan.started":
    case "scan.completed":
    case "scan.failed":
      return `${evt.type}:${evt.scan_id}`;
    case "scan.series_updated":
      return `${evt.type}:${evt.library_id}:${evt.series_id}:${evt.name}`;
    case "scan.health_issue":
      return `${evt.type}:${evt.scan_id}:${evt.kind}:${evt.severity}:${evt.path ?? ""}`;
    case "thumbs.started":
    case "thumbs.completed":
    case "thumbs.failed":
      return `${evt.type}:${evt.library_id}:${evt.issue_id}`;
    default:
      return JSON.stringify(evt);
  }
}

function phaseLabel(phase: string | undefined, status: Status): string {
  if (status === "failed") return "Failed";
  if (status === "thumbnailing") return "Generating thumbnails";
  if (status === "completed" || phase === "complete") return "Complete";
  switch (phase) {
    case "planning":
      return "Planning scan";
    case "planning_complete":
      return "Plan ready";
    case "scanning":
      return "Scanning files";
    case "reconciling":
      return "Reconciling";
    case "reconciled":
      return "Reconciled";
    case "enqueueing_thumbnails":
      return "Enqueueing thumbnails";
    default:
      return status === "running" ? "Waiting for progress" : "No active scan";
  }
}

function scanLabel(status: Status): string {
  if (status === "running") return "Planning scan";
  if (status === "thumbnailing") return "Generating thumbnails";
  if (status === "completed") return "Last scan completed";
  if (status === "failed") return "Last scan failed";
  if (status === "queued") return "Scan queued";
  return "Idle";
}

function PhaseChips({
  phase,
  status,
}: {
  phase: string | undefined;
  status: Status;
}) {
  const active = status === "completed" ? "complete" : phase;
  const activePhase =
    status === "thumbnailing" ? "enqueueing_thumbnails" : active;
  const phases = [
    ["planning", "Planning"],
    ["scanning", "Scanning"],
    ["reconciling", "Reconciling"],
    ["enqueueing_thumbnails", "Thumbnails"],
    ["complete", "Complete"],
  ] as const;
  return (
    <div className="flex flex-wrap gap-2">
      {phases.map(([key, label]) => (
        <Badge
          key={key}
          variant={
            activePhase === key ||
            (key === "scanning" && activePhase === "planning_complete")
              ? "default"
              : "secondary"
          }
          className="rounded-md"
        >
          {label}
        </Badge>
      ))}
    </div>
  );
}

function Metric({
  icon: Icon,
  label,
  value,
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  value: number | string;
}) {
  return (
    <Card>
      <CardContent className="flex items-center gap-3 p-4">
        <Icon className="text-muted-foreground h-4 w-4 shrink-0" />
        <div className="min-w-0">
          <dt className="text-muted-foreground truncate text-[10px] font-medium tracking-widest uppercase">
            {label}
          </dt>
          <dd className="text-foreground text-base font-semibold tabular-nums">
            {value}
          </dd>
        </div>
      </CardContent>
    </Card>
  );
}

function SeverityBadge({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "error" | "warning" | "info";
}) {
  return (
    <span
      className={cn(
        "rounded-md border px-1.5 py-0.5 text-[10px] font-semibold tabular-nums",
        tone === "error" && "border-destructive/40 text-destructive",
        tone === "warning" && "border-amber-700/40 text-amber-500",
        tone === "info" && "border-border text-muted-foreground",
      )}
    >
      {label} {value}
    </span>
  );
}

function SmallStat({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: number;
  tone?: "default" | "primary" | "destructive";
}) {
  return (
    <div>
      <dt className="text-muted-foreground text-[10px] font-medium tracking-widest uppercase">
        {label}
      </dt>
      <dd
        className={cn(
          "text-base font-semibold tabular-nums",
          tone === "primary" && "text-primary",
          tone === "destructive" && "text-destructive",
        )}
      >
        {value}
      </dd>
    </div>
  );
}

function ThumbEventDot({ type }: { type: ScanEvent["type"] }) {
  const tone =
    type === "thumbs.completed"
      ? "bg-primary"
      : type === "thumbs.failed"
        ? "bg-destructive"
        : "bg-amber-500";
  return <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", tone)} />;
}

function ThumbKindBadge({ event }: { event: ScanEvent }) {
  if (!("kind" in event)) return null;
  const isPage = event.kind === "page_map";
  const isBoth = event.kind === "cover_page_map";
  return (
    <Badge
      variant="secondary"
      className={cn(
        "shrink-0 rounded-md px-1.5 py-0 text-[10px] uppercase",
        isBoth
          ? "text-amber-300"
          : isPage
            ? "text-sky-300"
            : "text-emerald-300",
      )}
    >
      {isBoth ? "Both" : isPage ? "Page" : "Cover"}
    </Badge>
  );
}

function thumbLabelFor(event: ScanEvent): string {
  const kind =
    "kind" in event && event.kind === "page_map"
      ? "Page thumbnail"
      : "kind" in event && event.kind === "cover_page_map"
        ? "Cover and page thumbnails"
        : "kind" in event && event.kind === "cover"
          ? "Cover thumbnail"
          : "Thumbnail";
  switch (event.type) {
    case "thumbs.started":
      return `${kind} started`;
    case "thumbs.completed":
      return event.kind === "page_map"
        ? `Page thumbnails completed (${event.pages} page${event.pages === 1 ? "" : "s"})`
        : event.kind === "cover_page_map"
          ? `Cover and page thumbnails completed (${event.pages} image${event.pages === 1 ? "" : "s"})`
          : `${kind} completed`;
    case "thumbs.failed":
      return `${kind} failed: ${event.error}`;
    default:
      return event.type;
  }
}

function StatusPill({ status }: { status: Status }) {
  const tone = (() => {
    switch (status) {
      case "running":
      case "thumbnailing":
        return "border-emerald-700/60 bg-emerald-950/40 text-emerald-200";
      case "queued":
        return "border-amber-700/60 bg-amber-950/40 text-amber-200";
      case "completed":
        return "border-emerald-700/60 bg-emerald-950/40 text-emerald-200";
      case "failed":
        return "border-destructive/60 bg-destructive/10 text-destructive";
      default:
        return "border-border bg-muted/40 text-muted-foreground";
    }
  })();
  const Icon = (() => {
    switch (status) {
      case "running":
      case "queued":
      case "thumbnailing":
        return Loader2;
      case "completed":
        return CheckCircle2;
      case "failed":
        return AlertCircle;
      default:
        return Pause;
    }
  })();
  return (
    <span
      className={cn(
        "inline-flex items-center gap-2 rounded-full border px-3 py-1 text-xs font-semibold tracking-wider uppercase",
        tone,
      )}
    >
      <Icon
        className={cn(
          "h-3.5 w-3.5",
          status === "running" || status === "queued" ? "animate-spin" : "",
          status === "thumbnailing" ? "animate-spin" : "",
        )}
      />
      {status}
      {status === "idle" ? <Play className="h-3 w-3 opacity-60" /> : null}
    </span>
  );
}
