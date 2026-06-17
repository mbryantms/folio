"use client";

/**
 * `<DashboardTab>` — /admin/metadata default tab (M6).
 *
 * At-a-glance summary of the metadata-providers integration state.
 * Counts are computed server-side; quota gauges read the live Redis
 * token-bucket state.
 */

import {
  CheckCircle2,
  Fingerprint,
  ImageDown,
  Loader2,
  XCircle,
} from "lucide-react";
import Link from "next/link";
import { useState, type ReactNode } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { apiMutate } from "@/lib/api/mutations";
import {
  useAdminMetadataDashboard,
  useAdminMetadataMatchQuality,
  useAdminMetadataRecentApplies,
} from "@/lib/api/queries";
import { formatRelativeDate } from "@/lib/format";
import type {
  BackfillEnqueuedResp,
  MatchQualityWindow,
  ProviderView,
  RecentApplyRow,
} from "@/lib/api/types";

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
      <section className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
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

      <RecentAppliesCard />

      <MatchQualityCard />

      <CoverHashBackfillCard />

      <VariantCoverBackfillCard />
    </div>
  );
}

/**
 * "Recent applies" feed (audit B14). Surfaces the runs that actually wrote
 * metadata — crucially the **automatic** weekly-refresh applies, which emit
 * no audit-log row and otherwise run invisibly. A bounded summary; the full,
 * filterable history is the Runs tab.
 */
function RecentAppliesCard() {
  const q = useAdminMetadataRecentApplies(10);
  const rows = q.data?.applies ?? [];
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-2 pb-2">
        <CardTitle className="text-sm font-medium">Recent applies</CardTitle>
        <Link
          href="/admin/metadata?tab=runs"
          className="text-muted-foreground hover:text-foreground text-xs underline"
        >
          View all runs
        </Link>
      </CardHeader>
      <CardContent>
        {q.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 text-sm">
            <Loader2 className="h-3.5 w-3.5 animate-spin" /> Loading…
          </div>
        ) : rows.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            Nothing applied yet. Searches that write changes — manual or the
            weekly auto-refresh — show up here.
          </p>
        ) : (
          <ul className="divide-border/60 divide-y text-sm">
            {rows.map((r) => (
              <RecentApplyItem key={r.run_id} row={r} />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function RecentApplyItem({ row }: { row: RecentApplyRow }) {
  const when = formatRelativeDate(row.applied_at) ?? "";
  const sub = [
    row.providers.length > 0 ? row.providers.join(", ") : null,
    row.items_applied > 1 ? `${row.items_applied} items` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <li className="flex items-center justify-between gap-3 py-2">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          {row.series_slug ? (
            <Link
              href={`/series/${row.series_slug}`}
              className="truncate font-medium hover:underline"
            >
              {row.entity_label}
            </Link>
          ) : (
            <span className="truncate font-medium">{row.entity_label}</span>
          )}
          <Badge
            variant={row.automatic ? "secondary" : "outline"}
            className="shrink-0"
          >
            {row.automatic ? "Automatic" : "Manual"}
          </Badge>
        </div>
        {sub && <div className="text-muted-foreground text-xs">{sub}</div>}
      </div>
      <span className="text-muted-foreground shrink-0 text-xs">{when}</span>
    </li>
  );
}

/**
 * Trigger a background backfill drain (audit B17). The sweep used to run
 * synchronously in a request loop, holding the tab open while it decoded
 * hundreds of covers; it's now a queued apalis job. Clicking enqueues it
 * and returns immediately — the queue page shows it while pending and a
 * toast (from the central scan-events listener) reports the tally when the
 * drain finishes. Idempotent + safe to re-run.
 */
function BackfillCard({
  title,
  description,
  endpoint,
  icon,
  label,
}: {
  title: string;
  description: string;
  endpoint: string;
  icon: ReactNode;
  label: string;
}) {
  const [pending, setPending] = useState(false);

  const enqueue = async () => {
    setPending(true);
    try {
      const r = await apiMutate<BackfillEnqueuedResp>({
        path: endpoint,
        method: "POST",
      });
      if (r?.enqueued) {
        toast.info(
          "Backfill started — it runs in the background. You'll get a toast when it finishes; the Queue page shows it while it's pending.",
        );
      }
    } catch {
      toast.error("Could not start the backfill — see server logs.");
    } finally {
      setPending(false);
    }
  };

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">{title}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-muted-foreground text-sm">{description}</p>
        <Button onClick={enqueue} disabled={pending}>
          {pending ? (
            <>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Starting…
            </>
          ) : (
            <>
              {icon}
              {label}
            </>
          )}
        </Button>
      </CardContent>
    </Card>
  );
}

function CoverHashBackfillCard() {
  return (
    <BackfillCard
      title="Cover perceptual hashes"
      description="Compute perceptual hashes for covers scanned before pHash matching existed, so cover-similarity matching works on your existing libraries. Runs in the background as a queued job; safe to re-run — it only touches covers that still lack a hash."
      endpoint="/admin/metadata/phash-backfill"
      icon={<Fingerprint className="mr-2 h-4 w-4" />}
      label="Backfill cover hashes"
    />
  );
}

function VariantCoverBackfillCard() {
  return (
    <BackfillCard
      title="Variant covers"
      description="Re-download provider variant covers whose image file is missing on disk, or that are still kept as hotlinks. Runs in the background as a queued job; safe to re-run. Covers whose provider URL has expired can't be recovered here — re-apply metadata for those issues."
      endpoint="/admin/metadata/variant-cover-backfill"
      icon={<ImageDown className="mr-2 h-4 w-4" />}
      label="Re-download missing covers"
    />
  );
}

/**
 * Match-quality distribution (matching-accuracy-1.0 M0).
 *
 * Renders the rolling 7d + 28d bucket counts so the operator can see
 * whether the matcher is producing decisive single-good matches or
 * dropping everything into the review queue. Lands BEFORE the M2 / M4
 * matcher tuning so the trend has a real before/after baseline once
 * those ship.
 */
const OUTCOME_LABELS: Record<string, string> = {
  single_good: "Single strong match",
  multi_good: "Multiple strong matches",
  single_bad_cover: "One weak match",
  multi_bad_cover: "Multiple weak matches",
  no_match: "No matches",
};
const OUTCOME_ORDER = [
  "single_good",
  "multi_good",
  "single_bad_cover",
  "multi_bad_cover",
  "no_match",
];

function MatchQualityCard() {
  const q = useAdminMetadataMatchQuality();
  if (q.isLoading) {
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Match quality</CardTitle>
        </CardHeader>
        <CardContent className="text-muted-foreground flex items-center gap-2 text-sm">
          <Loader2 className="h-3.5 w-3.5 animate-spin" /> Loading…
        </CardContent>
      </Card>
    );
  }
  if (!q.data) return null;
  const { last_7d, last_28d, total_7d, total_28d } = q.data;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Match quality</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {total_7d === 0 && total_28d === 0 ? (
          <p className="text-muted-foreground text-sm">
            No search runs in the last 28 days — kick one off to start
            collecting baseline telemetry.
          </p>
        ) : (
          <div className="grid gap-4 sm:grid-cols-2">
            <MatchQualityWindowView
              title="Last 7 days"
              total={total_7d}
              rows={last_7d}
            />
            <MatchQualityWindowView
              title="Last 28 days"
              total={total_28d}
              rows={last_28d}
            />
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function MatchQualityWindowView({
  title,
  total,
  rows,
}: {
  title: string;
  total: number;
  rows: MatchQualityWindow[];
}) {
  // Server returns only buckets with count > 0; fill in zeros so the
  // operator can see "no_match: 0" instead of a missing row that's
  // easy to misread as "haven't measured".
  const byKind = new Map(rows.map((r) => [r.kind, r.count] as const));
  const display = OUTCOME_ORDER.map((kind) => ({
    kind,
    count: byKind.get(kind) ?? 0,
  }));

  return (
    <div className="space-y-2">
      <div className="text-muted-foreground flex items-center justify-between text-xs tracking-wide uppercase">
        <span>{title}</span>
        <span>{total.toLocaleString()} total</span>
      </div>
      <ul className="space-y-1 text-sm">
        {display.map((d) => {
          const pct = total > 0 ? Math.round((d.count / total) * 100) : 0;
          return (
            <li
              key={d.kind}
              className="flex items-center justify-between gap-3"
            >
              <span>{OUTCOME_LABELS[d.kind] ?? d.kind}</span>
              <span className="text-muted-foreground tabular-nums">
                {d.count.toLocaleString()}{" "}
                <span className="text-xs">({pct}%)</span>
              </span>
            </li>
          );
        })}
      </ul>
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
        <div className="text-muted-foreground flex items-center gap-1.5 text-xs tracking-wide uppercase">
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
    <div className="border-border flex items-center justify-between gap-3 border-b py-2 text-sm last:border-0">
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
            {quota.seconds_until_reset != null &&
              quota.seconds_until_reset > 0 && (
                <span>
                  {" "}
                  · resets in {formatSeconds(quota.seconds_until_reset)}
                </span>
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
