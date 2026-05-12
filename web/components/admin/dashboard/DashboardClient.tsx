"use client";

import {
  Activity,
  AlertTriangle,
  BookOpen,
  Database,
  FileStack,
  HardDrive,
  Library,
  ListChecks,
  Server,
  Users,
} from "lucide-react";
import Link from "next/link";

import { StatCard } from "@/components/admin/StatCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { sparklinePoints } from "@/lib/activity";
import {
  useAdminOverview,
  useQueueDepth,
  useServerInfo,
} from "@/lib/api/queries";
import type { AdminOverviewView, ServerInfoView } from "@/lib/api/types";

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
        hint="discoverable trees"
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
          <ul className="space-y-2 text-sm">
            <SeverityRow
              label="Errors"
              count={data.open_health.error}
              variant="error"
            />
            <SeverityRow
              label="Warnings"
              count={data.open_health.warning}
              variant="warning"
            />
            <SeverityRow
              label="Info"
              count={data.open_health.info}
              variant="info"
            />
          </ul>
        )}
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/libraries`}>View by library</Link>
        </Button>
      </CardContent>
    </Card>
  );
}

function SeverityRow({
  label,
  count,
  variant,
}: {
  label: string;
  count: number;
  variant: "error" | "warning" | "info";
}) {
  const tone =
    variant === "error"
      ? "text-red-400"
      : variant === "warning"
        ? "text-amber-400"
        : "text-blue-400";
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground">{label}</span>
      <span
        className={`text-base font-semibold tabular-nums ${count > 0 ? tone : "text-foreground/60"}`}
      >
        {count}
      </span>
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
        <Button asChild variant="outline" size="sm" className="w-full">
          <Link href={`/admin/libraries`}>Trigger a scan</Link>
        </Button>
      </CardContent>
    </Card>
  );
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
