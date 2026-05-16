"use client";

import {
  Database,
  ExternalLink,
  HardDrive,
  ListChecks,
  Eye,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useServerInfo } from "@/lib/api/queries";
import type { ServerInfoView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

export function ServerInfoClient() {
  const info = useServerInfo({ intervalMs: 15_000 });

  if (info.isLoading || !info.data) {
    return <Skeleton className="h-72 w-full" />;
  }
  if (info.error) {
    return (
      <p className="text-destructive text-sm">Failed to load server info.</p>
    );
  }
  const data = info.data;

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <BuildCard data={data} />
      <DependenciesCard data={data} />
      <RuntimeCard data={data} />
      <LinksCard />
    </div>
  );
}

function BuildCard({ data }: { data: ServerInfoView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Build
        </CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="space-y-2 text-sm">
          <Row label="Version" value={data.version} mono />
          <Row
            label="Build SHA"
            value={
              data.build_sha === "dev"
                ? "dev (untagged)"
                : data.build_sha.slice(0, 12)
            }
            mono
          />
          <Row label="Uptime" value={formatUptime(data.uptime_secs)} mono />
        </dl>
      </CardContent>
    </Card>
  );
}

function DependenciesCard({ data }: { data: ServerInfoView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Dependencies
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
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
        </ul>
      </CardContent>
    </Card>
  );
}

function RuntimeCard({ data }: { data: ServerInfoView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Runtime
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <Pill
            icon={<ListChecks className="h-3.5 w-3.5" />}
            label="Cron scheduler"
            ok={data.scheduler_running}
          />
          <li className="flex items-center justify-between">
            <span className="text-muted-foreground flex items-center gap-2">
              <Eye className="h-3.5 w-3.5" />
              Library file-watchers
            </span>
            <span className="text-foreground font-mono tabular-nums">
              {data.watchers_enabled}
            </span>
          </li>
        </ul>
      </CardContent>
    </Card>
  );
}

function LinksCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Probes &amp; metrics
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <ProbeLink
            href="/healthz"
            label="/healthz"
            hint="liveness probe"
          />
          <ProbeLink
            href="/readyz"
            label="/readyz"
            hint="readiness probe"
          />
          <ProbeLink
            href="/metrics"
            label="/metrics"
            hint="Prometheus exporter"
          />
        </ul>
      </CardContent>
    </Card>
  );
}

function Row({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={cn("text-foreground", mono && "font-mono tabular-nums")}>
        {value}
      </dd>
    </div>
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

function ProbeLink({
  href,
  label,
  hint,
}: {
  href: string;
  label: string;
  hint: string;
}) {
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground">{hint}</span>
      <a
        href={href}
        target="_blank"
        rel="noreferrer"
        className="text-foreground flex items-center gap-1 font-mono underline-offset-4 hover:underline"
      >
        {label}
        <ExternalLink className="h-3 w-3" />
      </a>
    </li>
  );
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const minutes = Math.floor(secs / 60);
  if (minutes < 60) return `${minutes}m ${secs % 60}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}
