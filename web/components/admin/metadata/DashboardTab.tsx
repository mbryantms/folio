"use client";

/**
 * `<DashboardTab>` — /admin/metadata default tab (M6).
 *
 * At-a-glance summary of the metadata-providers integration state.
 * Counts are computed server-side; quota gauges read the live Redis
 * token-bucket state.
 */

import { CheckCircle2, Fingerprint, Loader2, Search, XCircle } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { apiMutate } from "@/lib/api/mutations";
import {
  useAdminMetadataDashboard,
  useAdminMetadataMatchQuality,
} from "@/lib/api/queries";
import type { MatchQualityWindow, ProviderView } from "@/lib/api/types";

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

      <MatchQualityCard />

      <CoverHashBackfillCard />
    </div>
  );
}

type BackfillOutcome = {
  considered: number;
  hashed: number;
  skipped: number;
  errored: number;
};

/**
 * One-click cover perceptual-hash backfill. Existing libraries scanned
 * before pHash matching have NULL cover hashes; this drains them so
 * cover-similarity matching works on the back-catalog.
 *
 * Runs the server endpoint in **small batches in a loop** rather than one
 * big call: decoding hundreds of covers in a single request can exceed a
 * reverse proxy's timeout (Cloudflare 524). Each `limit=25` batch returns
 * in seconds; the loop stops when a batch hashes nothing new. The hashing
 * itself is idempotent + resumable, so re-clicking is always safe.
 */
function CoverHashBackfillCard() {
  const [running, setRunning] = useState(false);
  const [hashed, setHashed] = useState(0);
  const [done, setDone] = useState(false);

  const run = async () => {
    setRunning(true);
    setDone(false);
    setHashed(0);
    let total = 0;
    let errored = 0;
    try {
      for (;;) {
        const r = await apiMutate<BackfillOutcome>({
          path: "/admin/metadata/phash-backfill?limit=25",
          method: "POST",
        });
        if (!r) break;
        total += r.hashed;
        errored += r.errored;
        setHashed(total);
        // No forward progress this batch → drained (or only
        // un-decodable covers remain). Stop.
        if (r.hashed === 0) break;
      }
      setDone(true);
      toast.success(
        `Backfilled ${total.toLocaleString()} cover hash${total === 1 ? "" : "es"}` +
          (errored > 0 ? ` · ${errored} could not be decoded` : ""),
      );
    } catch {
      toast.error("Cover-hash backfill failed — see server logs");
    } finally {
      setRunning(false);
    }
  };

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Cover perceptual hashes
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-muted-foreground text-sm">
          Compute perceptual hashes for covers scanned before pHash matching
          existed, so cover-similarity matching works on your existing
          libraries. Runs in small batches — keep this tab open until it
          finishes. Safe to re-run; it only touches covers that still lack a
          hash.
        </p>
        <div className="flex items-center gap-3">
          <Button onClick={run} disabled={running}>
            {running ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Backfilling… ({hashed.toLocaleString()})
              </>
            ) : (
              <>
                <Fingerprint className="mr-2 h-4 w-4" />
                Backfill cover hashes
              </>
            )}
          </Button>
          {done && !running && (
            <span className="text-muted-foreground text-sm">
              Done — {hashed.toLocaleString()} hashed.
            </span>
          )}
        </div>
      </CardContent>
    </Card>
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
      <div className="text-muted-foreground flex items-center justify-between text-xs uppercase tracking-wide">
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
