"use client";

import * as React from "react";

import {
  Activity,
  AlertTriangle,
  BookOpen,
  Clock,
  Database,
  FileStack,
  HardDrive,
  Library,
  ListChecks,
  Server,
  Users,
  XCircle,
} from "lucide-react";
import Link from "next/link";

import { StatCard } from "@/components/admin/StatCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { sparklinePoints } from "@/lib/activity";
import { cn } from "@/lib/utils";
import {
  useAdminLatestScanPerLibrary,
  useAdminOverview,
  useAdminScanRuns,
  useQueueDepth,
  useServerInfo,
} from "@/lib/api/queries";
import type {
  AdminOverviewView,
  CrossLibScanRunView,
  ServerInfoView,
} from "@/lib/api/types";

export function DashboardClient() {
  const overview = useAdminOverview();
  const queue = useQueueDepth({ intervalMs: 10_000 });
  const serverInfo = useServerInfo();

  return (
    <div className="space-y-6">
      <TotalsRow data={overview.data} loading={overview.isLoading} />

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <HealthCard data={overview.data} loading={overview.isLoading} />
        <ScansCard
          data={overview.data}
          queueTotal={queue.data?.total ?? 0}
          loading={overview.isLoading}
        />
        <ReadersCard data={overview.data} loading={overview.isLoading} />
      </div>

      {/* M2 of the findings plan: surface the per-library scan-run
       *  data on the dashboard so operators see "did anything just
       *  break" and "which libraries are stale" without drilling. */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <RecentScanFailuresCard />
        <LatestScanPerLibraryCard />
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <ReadsSparklineCard data={overview.data} loading={overview.isLoading} />
        <ServerHealthCard
          data={serverInfo.data}
          loading={serverInfo.isLoading}
        />
        <QuickActionsCard />
      </div>
    </div>
  );
}

function TotalsRow({
  data,
  loading,
}: {
  data?: AdminOverviewView;
  loading: boolean;
}) {
  if (loading || !data) {
    return (
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {[0, 1, 2, 3].map((i) => (
          <Skeleton key={i} className="h-28 w-full" />
        ))}
      </div>
    );
  }
  return (
    <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
      <StatCard
        label="Libraries"
        value={data.totals.libraries}
        hint="configured"
      />
      <StatCard
        label="Series"
        value={data.totals.series}
        hint="across all libraries"
      />
      <StatCard
        label="Issues"
        value={data.totals.issues.toLocaleString()}
        hint="active, on disk"
      />
      <StatCard label="Users" value={data.totals.users} hint="local + OIDC" />
    </div>
  );
}

function HealthCard({
  data,
  loading,
}: {
  data?: AdminOverviewView;
  loading: boolean;
}) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Open health issues
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Unresolved + undismissed across libraries.
          </p>
        </div>
        <AlertTriangle className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-3 pt-2">
        {loading || !data ? (
          <Skeleton className="h-16 w-full" />
        ) : (
          <ul className="space-y-1 text-sm">
            <SeverityRow
              label="Errors"
              count={data.open_health.error}
              variant="error"
              severityParam="error"
            />
            <SeverityRow
              label="Warnings"
              count={data.open_health.warning}
              variant="warning"
              severityParam="warning"
            />
            <SeverityRow
              label="Info"
              count={data.open_health.info}
              variant="info"
              severityParam="info"
            />
          </ul>
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/findings`}>Open findings</Link>
        </Button>
      </CardContent>
    </Card>
  );
}

function SeverityRow({
  label,
  count,
  variant,
  severityParam,
}: {
  label: string;
  count: number;
  variant: "error" | "warning" | "info";
  severityParam: string;
}) {
  const tone =
    variant === "error"
      ? "text-red-400"
      : variant === "warning"
        ? "text-amber-400"
        : "text-blue-400";
  // Each severity row is a deep link into the findings page with the
  // matching filter pre-applied — turns the dashboard's summary
  // numbers into a one-click drill-in instead of stranding the
  // operator at the count.
  return (
    <li>
      <Link
        href={`/admin/findings?tab=health&severity=${severityParam}`}
        className="hover:bg-muted/50 -mx-2 flex items-center justify-between rounded px-2 py-0.5 transition-colors"
      >
        <span className="text-muted-foreground">{label}</span>
        <span
          className={cn(
            "text-base font-semibold tabular-nums",
            count > 0 ? tone : "text-foreground/60",
          )}
        >
          {count}
        </span>
      </Link>
    </li>
  );
}

function ScansCard({
  data,
  queueTotal,
  loading,
}: {
  data?: AdminOverviewView;
  queueTotal: number;
  loading: boolean;
}) {
  // Pull the latest-scan-per-library snapshot so the card can show
  // "Last scan N ago" — answers the "is anything happening?" question
  // when the running/queue counts are both zero. Reuses the same
  // dashboard query the LatestScanPerLibraryCard already issues.
  const latest = useAdminLatestScanPerLibrary();
  const mostRecent = React.useMemo(() => {
    const rows = latest.data ?? [];
    if (rows.length === 0) return null;
    return rows.reduce<CrossLibScanRunView | null>((best, row) => {
      const t = Date.parse(row.started_at);
      if (Number.isNaN(t)) return best;
      if (!best) return row;
      return t > Date.parse(best.started_at) ? row : best;
    }, null);
  }, [latest.data]);

  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Scans
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            In flight + queued across libraries.
          </p>
        </div>
        <ListChecks className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-3 pt-2">
        {loading || !data ? (
          <Skeleton className="h-16 w-full" />
        ) : (
          <div className="grid grid-cols-2 gap-3 text-sm">
            <Stat
              label="Running"
              value={data.scans_in_flight}
              live={data.scans_in_flight > 0}
            />
            <Stat label="Queue" value={queueTotal} live={queueTotal > 0} />
          </div>
        )}
        {mostRecent ? (
          <p className="text-muted-foreground text-xs">
            Last scan: <span className="text-foreground">{mostRecent.library_name}</span> ·{" "}
            {formatRelative(mostRecent.started_at)}
          </p>
        ) : null}
        <div className="flex gap-2">
          <Button asChild variant="outline" size="sm" className="flex-1">
            <Link href={`/admin/libraries`}>Manage libraries</Link>
          </Button>
          <Button asChild variant="outline" size="sm" className="flex-1">
            <Link href={`/admin/findings?tab=scans`}>View runs</Link>
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function RecentScanFailuresCard() {
  // 7-day window so a long-quiet library still surfaces a stale
  // failure operator hasn't seen yet.
  const since = React.useMemo(() => {
    const d = new Date();
    d.setDate(d.getDate() - 7);
    return d.toISOString();
  }, []);
  const { data, isLoading } = useAdminScanRuns({
    state: "failed",
    since,
    limit: 5,
  });
  const items = data?.items ?? [];

  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Recent scan failures (7d)
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Anything in here means a library scan didn&apos;t finish cleanly.
          </p>
        </div>
        <XCircle className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-2 pt-2">
        {isLoading ? (
          <Skeleton className="h-16 w-full" />
        ) : items.length === 0 ? (
          <p className="text-muted-foreground py-4 text-center text-xs">
            All scans green for the last 7 days.
          </p>
        ) : (
          <ul className="space-y-1.5 text-sm">
            {items.map((row) => (
              <li
                key={row.id}
                className="border-border/60 border-b pb-1.5 last:border-b-0 last:pb-0"
              >
                <Link
                  href={`/admin/libraries/${row.library_slug}/history`}
                  className="hover:bg-muted/50 -mx-1 block rounded px-1 py-0.5 transition-colors"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate font-medium">
                      {row.library_name}
                    </span>
                    <span className="text-muted-foreground shrink-0 text-xs">
                      {formatRelative(row.started_at)}
                    </span>
                  </div>
                  {row.error ? (
                    <p className="text-muted-foreground truncate text-xs">
                      {row.error}
                    </p>
                  ) : null}
                </Link>
              </li>
            ))}
          </ul>
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/findings?tab=scans&state=failed`}>
            View all failures
          </Link>
        </Button>
      </CardContent>
    </Card>
  );
}

function LatestScanPerLibraryCard() {
  const { data, isLoading } = useAdminLatestScanPerLibrary();
  // Oldest-first order from the backend; show the top 5 so the
  // most-stale libraries float up — answers "what hasn't been
  // touched in months."
  const items = (data ?? []).slice(0, 5);

  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Oldest scans per library
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            The 5 libraries that haven&apos;t scanned in the longest.
          </p>
        </div>
        <Clock className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-2 pt-2">
        {isLoading ? (
          <Skeleton className="h-16 w-full" />
        ) : items.length === 0 ? (
          <p className="text-muted-foreground py-4 text-center text-xs">
            No scans recorded yet.
          </p>
        ) : (
          <ul className="space-y-1 text-sm">
            {items.map((row) => (
              <li key={row.id}>
                <Link
                  href={`/admin/libraries/${row.library_slug}/history`}
                  className="hover:bg-muted/50 -mx-1 flex items-center justify-between gap-2 rounded px-1 py-0.5 transition-colors"
                >
                  <span className="truncate">{row.library_name}</span>
                  <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                    {formatRelative(row.started_at)}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/findings?tab=scans`}>View all scan runs</Link>
        </Button>
      </CardContent>
    </Card>
  );
}

/**
 * Coarse relative-time label, kept inline rather than pulling in a
 * date-fns dependency for the dashboard alone. Reads like a status
 * line — "5m ago", "3h ago", "12d ago" — not a precise stamp.
 */
function formatRelative(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const ms = Date.now() - t;
  const s = Math.max(1, Math.round(ms / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86_400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86_400)}d ago`;
}

function ReadersCard({
  data,
  loading,
}: {
  data?: AdminOverviewView;
  loading: boolean;
}) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Readers
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Active = heartbeat in the last 5 minutes.
          </p>
        </div>
        <BookOpen className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-3 pt-2">
        {loading || !data ? (
          <Skeleton className="h-16 w-full" />
        ) : (
          <div className="grid grid-cols-2 gap-3 text-sm">
            <Stat
              label="Reading now"
              value={data.active_readers_now}
              live={data.active_readers_now > 0}
            />
            <Stat label="Sessions today" value={data.sessions_today} />
          </div>
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/stats`}>Open stats</Link>
        </Button>
      </CardContent>
    </Card>
  );
}

function Stat({
  label,
  value,
  live,
}: {
  label: string;
  value: number;
  live?: boolean;
}) {
  return (
    <div className="border-border bg-background rounded-md border p-3">
      <p className="text-muted-foreground flex items-center gap-1.5 text-xs tracking-wide uppercase">
        {label}
        {live ? (
          <span
            aria-hidden="true"
            className="bg-primary inline-block h-1.5 w-1.5 animate-pulse rounded-full"
          />
        ) : null}
      </p>
      <p className="text-foreground mt-1 text-xl font-semibold">{value}</p>
    </div>
  );
}

function ReadsSparklineCard({
  data,
  loading,
}: {
  data?: AdminOverviewView;
  loading: boolean;
}) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Reads — last 14 days
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Sessions per day across all users.
          </p>
        </div>
        <Activity className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-3 pt-2">
        {loading || !data ? (
          <Skeleton className="h-16 w-full" />
        ) : data.reads_per_day.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No reading recorded in the last 14 days.
          </p>
        ) : (
          <Sparkline
            series={data.reads_per_day.map((d) => d.sessions)}
            ariaLabel="Sessions per day, last 14 days"
          />
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/stats`}>Deep dive</Link>
        </Button>
      </CardContent>
    </Card>
  );
}

function Sparkline({
  series,
  ariaLabel,
}: {
  series: ReadonlyArray<number>;
  ariaLabel: string;
}) {
  const w = 240;
  const h = 56;
  const points = sparklinePoints(series, w, h);
  return (
    <svg
      role="img"
      aria-label={ariaLabel}
      viewBox={`0 0 ${w} ${h}`}
      width={w}
      height={h}
      style={{ maxWidth: "100%", height: "auto" }}
      className="text-primary block"
    >
      <polyline
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        points={points}
      />
    </svg>
  );
}

function ServerHealthCard({
  data,
  loading,
}: {
  data?: ServerInfoView;
  loading: boolean;
}) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Service status
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Live ping of the runtime dependencies.
          </p>
        </div>
        <Server className="text-muted-foreground h-4 w-4" />
      </CardHeader>
      <CardContent className="space-y-3 pt-2">
        {loading || !data ? (
          <Skeleton className="h-24 w-full" />
        ) : (
          <ul className="space-y-1.5 text-sm">
            <Pill
              icon={<Database className="h-3.5 w-3.5" />}
              label="Postgres"
              ok={data.postgres_ok}
            />
            <Pill
              icon={<HardDrive className="h-3.5 w-3.5" />}
              label="Redis"
              ok={data.redis_ok}
            />
            <Pill
              icon={<ListChecks className="h-3.5 w-3.5" />}
              label="Scheduler"
              ok={data.scheduler_running}
            />
            <li className="text-muted-foreground flex items-center justify-between text-xs">
              <span>Version</span>
              <span className="tabular-nums">
                {data.version}
                {data.build_sha !== "dev"
                  ? ` · ${data.build_sha.slice(0, 7)}`
                  : ""}
              </span>
            </li>
            <li className="text-muted-foreground flex items-center justify-between text-xs">
              <span>Uptime</span>
              <span className="tabular-nums">
                {formatUptime(data.uptime_secs)}
              </span>
            </li>
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function Pill({
  icon,
  label,
  ok,
}: {
  icon: React.ReactNode;
  label: string;
  ok: boolean;
}) {
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground flex items-center gap-2">
        {icon}
        {label}
      </span>
      <Badge
        variant="outline"
        className={
          ok
            ? "border-emerald-500/40 text-emerald-400"
            : "border-red-500/40 text-red-400"
        }
      >
        {ok ? "OK" : "Down"}
      </Badge>
    </li>
  );
}

function QuickActionsCard() {
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
        <div>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Quick actions
          </CardTitle>
          <p className="text-muted-foreground mt-1 text-xs">
            Common operator paths.
          </p>
        </div>
      </CardHeader>
      <CardContent className="space-y-2 pt-2">
        <Action
          icon={<Library className="h-3.5 w-3.5" />}
          href={`/admin/libraries`}
          label="Manage libraries"
        />
        <Action
          icon={<Users className="h-3.5 w-3.5" />}
          href={`/admin/users`}
          label="Manage users"
        />
        <Action
          icon={<FileStack className="h-3.5 w-3.5" />}
          href={`/admin/audit`}
          label="View audit log"
        />
        <Action
          icon={<Activity className="h-3.5 w-3.5" />}
          href={`/admin/stats`}
          label="Open stats"
        />
      </CardContent>
    </Card>
  );
}

function Action({
  icon,
  href,
  label,
}: {
  icon: React.ReactNode;
  href: string;
  label: string;
}) {
  return (
    <Button
      asChild
      variant="outline"
      size="sm"
      className="w-full justify-start"
    >
      <Link href={href} className="flex items-center gap-2">
        {icon}
        {label}
      </Link>
    </Button>
  );
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86_400) return `${Math.round(secs / 3600)}h`;
  return `${Math.round(secs / 86_400)}d`;
}
