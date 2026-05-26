"use client";

/**
 * `<DashboardTab>` — /admin/metadata default tab (M6).
 *
 * At-a-glance summary of the metadata-providers integration state.
 * Counts are computed server-side; quota gauges read the live Redis
 * token-bucket state.
 */

import { CheckCircle2, Loader2, Search, XCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useAdminMetadataDashboard } from "@/lib/api/queries";
import type { ProviderView } from "@/lib/api/types";

export function DashboardTab() {
  const q = useAdminMetadataDashboard();
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }
  if (!q.data) {
    return null;
  }
  const d = q.data;
  const matchedPct =
    d.series_total > 0
      ? Math.round((d.series_matched / d.series_total) * 100)
      : 0;
  return (
    <div className="space-y-4">
      <section className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard
          label="Series matched"
          value={`${d.series_matched.toLocaleString()} / ${d.series_total.toLocaleString()}`}
          accent={`${matchedPct}%`}
          icon={<CheckCircle2 className="h-4 w-4" />}
        />
        <StatCard
          label="Unmatched"
          value={d.series_unmatched.toLocaleString()}
          accent={d.series_unmatched > 0 ? "needs search" : "none"}
          icon={<XCircle className="h-4 w-4" />}
        />
        <StatCard
          label="Review queue"
          value={d.review_queue_count.toLocaleString()}
          accent="medium + low"
          icon={<Search className="h-4 w-4" />}
        />
        <StatCard
          label="Applies (7d)"
          value={d.applies_last_7_days.toLocaleString()}
          accent="audit log"
        />
      </section>

      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Provider quota</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          {d.providers.map((p) => (
            <ProviderQuotaRow key={p.id} provider={p} />
          ))}
        </CardContent>
      </Card>
    </div>
  );
}

function StatCard({
  label,
  value,
  accent,
  icon,
}: {
  label: string;
  value: string;
  accent?: string;
  icon?: React.ReactNode;
}) {
  return (
    <Card>
      <CardContent className="space-y-1 p-4">
        <div className="text-muted-foreground flex items-center gap-1.5 text-xs uppercase tracking-wide">
          {icon}
          {label}
        </div>
        <div className="text-2xl font-semibold">{value}</div>
        {accent && (
          <div className="text-muted-foreground text-xs">{accent}</div>
        )}
      </CardContent>
    </Card>
  );
}

function ProviderQuotaRow({ provider }: { provider: ProviderView }) {
  const enabled = provider.enabled;
  const quota = provider.quota;
  return (
    <div className="flex items-center justify-between gap-3 border-b py-2 text-sm last:border-0">
      <div>
        <div className="flex items-center gap-2">
          <span className="font-medium">{provider.label}</span>
          {enabled ? (
            <Badge variant="default" className="text-[10px]">
              ENABLED
            </Badge>
          ) : provider.configured ? (
            <Badge variant="outline" className="text-[10px]">
              DISABLED
            </Badge>
          ) : (
            <Badge variant="secondary" className="text-[10px]">
              NOT CONFIGURED
            </Badge>
          )}
        </div>
      </div>
      <div className="text-muted-foreground text-xs">
        {quota ? (
          <>
            {quota.remaining_hour != null && (
              <span>
                {quota.remaining_hour.toLocaleString()} /hr
                {quota.remaining_day != null
                  ? ` · ${quota.remaining_day.toLocaleString()} /day`
                  : ""}
              </span>
            )}
            {quota.seconds_until_reset != null && quota.seconds_until_reset > 0 && (
              <span> · resets in {formatSeconds(quota.seconds_until_reset)}</span>
            )}
          </>
        ) : (
          <span>—</span>
        )}
      </div>
    </div>
  );
}

function formatSeconds(s: number): string {
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}
