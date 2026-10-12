"use client";

/**
 * `<ProvidersTab>` — /admin/metadata `?tab=providers` (M6).
 *
 * Per-provider connectivity + quota + "Test" button, with the
 * credential / enabled-toggle form inline on each card
 * (`ProviderConfigForm`). Operational tuning (cache TTLs, auto-apply
 * threshold, refresh cron) lives one tab over on `?tab=settings` —
 * there is no generic /admin/settings page.
 */

import {
  AlertTriangle,
  CheckCircle2,
  ExternalLink,
  Loader2,
  XCircle,
} from "lucide-react";

import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { useTestLocalDb, useTestMetadataProvider } from "@/lib/api/mutations";
import { useAdminMetadataProviders } from "@/lib/api/queries";
import type { LocalDbView, RequestBudget, ProviderView } from "@/lib/api/types";
import {
  LOW_BUDGET_FRACTION,
  budgetFraction,
  formatBudget,
  formatCountdown,
} from "@/lib/metadata/quota";
import { statusToneText } from "@/lib/ui/status-tone";

import {
  ProviderConfigForm,
  isConfigurableProvider,
} from "./ProviderConfigForm";

const PROVIDER_DOCS: Record<string, string> = {
  comicvine: "https://comicvine.gamespot.com/api/",
  metron: "https://metron.cloud/",
  gcd: "https://www.comics.org/api/",
};

export function ProvidersTab() {
  const q = useAdminMetadataProviders();
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }
  return (
    <div className="space-y-3">
      {(q.data?.providers ?? []).map((p) => (
        <ProviderCard key={p.id} provider={p} />
      ))}
      <p className="text-muted-foreground text-xs">
        Cache TTLs, the auto-apply threshold, and other operational tuning live
        in the <span className="font-medium">Settings</span> tab above. The
        cards here own the credentials + enable toggles.
      </p>
    </div>
  );
}

/**
 * Request budget bar (WP-2.9): what the upstream says is left in its
 * headline window (Metron's daily budget; ComicVine's local hourly
 * bucket; GCD's local daily bucket). Tone turns to warning under 20% — the same threshold the
 * search dialog uses for its note.
 */
function BudgetBar({
  budget,
  nowMs,
}: {
  budget: RequestBudget;
  nowMs: number;
}) {
  const fraction = budgetFraction(budget);
  const low = fraction < LOW_BUDGET_FRACTION;
  return (
    <div className="space-y-1" data-testid="provider-budget">
      <Progress
        value={Math.round(fraction * 100)}
        aria-label="Remaining request budget"
        className={low ? "[&>div]:bg-warning" : undefined}
      />
      <p
        className={`text-xs ${low ? statusToneText("warning") : "text-muted-foreground"}`}
      >
        {formatBudget(budget, nowMs)}
      </p>
    </div>
  );
}

function formatLastErrorAge(at: string, nowMs: number): string {
  const then = Date.parse(at);
  if (Number.isNaN(then)) return at;
  const seconds = Math.max(0, Math.round((nowMs - then) / 1000));
  if (seconds < 60) return "just now";
  return `${formatCountdown(seconds)} ago`;
}

/** One line per fact about a provider's local file: where it stands,
 *  the dump date (with the stale warning), counts, and a Test button
 *  that re-probes past the cache. Nothing here downloads anything. */
function LocalFileStatus({
  providerId,
  local,
}: {
  providerId: string;
  local: LocalDbView;
}) {
  const test = useTestLocalDb();
  const probe = local.probe ?? null;
  const counts = probe
    ? Object.entries(probe.counts)
        .map(([k, v]) => `${Number(v).toLocaleString()} ${k}`)
        .join(" · ")
    : null;
  return (
    <div
      className="border-border space-y-1 rounded border p-2 text-xs"
      data-testid="provider-local-file"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="flex items-center gap-2 font-medium">
          {local.label}
          {local.status === "ok" ? (
            <Badge variant="default" className="text-[10px]">
              {local.mode === "off"
                ? "FILE OK · MODE OFF"
                : local.mode.toUpperCase().replace("_", " ")}
            </Badge>
          ) : local.status === "unconfigured" ? (
            <Badge variant="secondary" className="text-[10px]">
              NO FILE
            </Badge>
          ) : (
            <Badge variant="destructive" className="text-[10px]">
              {local.status.toUpperCase()}
            </Badge>
          )}
        </span>
        <Button
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() => test.mutate({ id: providerId })}
          disabled={test.isPending || local.status === "unconfigured"}
        >
          {test.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Testing
            </>
          ) : (
            "Test file"
          )}
        </Button>
      </div>
      {local.path ? (
        <p
          className="text-muted-foreground truncate font-mono"
          title={local.path}
        >
          {local.path}
        </p>
      ) : (
        <p className="text-muted-foreground">
          Set a file path below to read this provider from disk.
        </p>
      )}
      {probe ? (
        <p className="text-muted-foreground">
          {probe.dump_date
            ? `Dump date ${probe.dump_date}`
            : "Dump date unknown"}
          {counts ? ` · ${counts}` : ""}
        </p>
      ) : null}
      {probe?.stale ? (
        <p className={`flex items-start gap-1 ${statusToneText("warning")}`}>
          <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
          <span>
            This file is more than 60 days old; releases newer than its dump
            date are not in it.
          </span>
        </p>
      ) : null}
      {probe?.missing_optional?.length ? (
        <p className="text-muted-foreground">
          Missing optional tables: {probe.missing_optional.join(", ")}
        </p>
      ) : null}
      {local.error ? (
        <p className="text-destructive flex items-start gap-1">
          <XCircle className="mt-0.5 h-3 w-3 shrink-0" /> {local.error}
        </p>
      ) : null}
      {test.error ? (
        <p className="text-destructive flex items-start gap-1">
          <XCircle className="mt-0.5 h-3 w-3 shrink-0" /> {test.error.message}
        </p>
      ) : null}
      {!test.error && test.data?.ok ? (
        <p className={statusToneText("success")}>
          <CheckCircle2 className="mr-1 inline h-3 w-3" /> File readable —{" "}
          {test.data.duration_ms}ms.
        </p>
      ) : null}
      {providerId === "gcd" && local.status === "ok" ? (
        <p className="text-muted-foreground">
          Data from the Grand Comics Database, CC BY-SA 4.0. Pages that show it
          carry a credit line.
        </p>
      ) : null}
    </div>
  );
}

function ProviderCard({ provider }: { provider: ProviderView }) {
  const test = useTestMetadataProvider();
  const onTest = () => test.mutate({ id: provider.id });
  const lastResult = test.data?.ok;
  // Sampled once when the card mounts (lazy initializer, so render stays
  // pure); the reset countdown is coarse (minutes/hours) and the query
  // refetch remounts the list on every refresh anyway.
  const [now] = React.useState(() => Date.now());
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          {provider.label}
          {provider.enabled ? (
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
        </CardTitle>
        <Button
          size="sm"
          variant="outline"
          onClick={onTest}
          // Testable once a credential exists — the natural setup order
          // is paste key → Test → enable, so gating on `enabled` forced
          // admins to enable a possibly-broken credential first.
          disabled={test.isPending || !provider.configured}
        >
          {test.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Testing
            </>
          ) : (
            "Test"
          )}
        </Button>
      </CardHeader>
      <CardContent className="space-y-2 text-sm">
        <div className="text-muted-foreground flex items-center gap-3">
          {provider.quota ? (
            <>
              {provider.quota.remaining_hour != null && (
                <span>
                  {provider.quota.remaining_hour.toLocaleString()} /hr
                </span>
              )}
              {provider.quota.remaining_day != null && (
                <span>
                  · {provider.quota.remaining_day.toLocaleString()} /day
                </span>
              )}
            </>
          ) : (
            <span>No quota data</span>
          )}
        </div>
        {provider.budget && <BudgetBar budget={provider.budget} nowMs={now} />}
        {provider.last_error && (
          <div
            className="text-destructive flex items-start gap-1 text-xs"
            data-testid="provider-last-error"
          >
            <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
            <span>
              Last error ({formatLastErrorAge(provider.last_error.at, now)}):{" "}
              {provider.last_error.message}
            </span>
          </div>
        )}
        {test.error && (
          <div className="text-destructive flex items-center gap-1 text-xs">
            <XCircle className="h-3 w-3" /> {test.error.message}
          </div>
        )}
        {!test.error && lastResult === true && (
          <div className={`text-xs ${statusToneText("success")}`}>
            <CheckCircle2 className="mr-1 inline h-3 w-3" /> Live —{" "}
            {test.data?.duration_ms}ms round-trip.
          </div>
        )}
        {provider.local ? (
          <LocalFileStatus providerId={provider.id} local={provider.local} />
        ) : null}
        {PROVIDER_DOCS[provider.id] && (
          <a
            href={PROVIDER_DOCS[provider.id]}
            target="_blank"
            rel="noreferrer"
            className="text-muted-foreground inline-flex items-center gap-1 text-xs hover:underline"
          >
            Provider docs <ExternalLink className="h-3 w-3" />
          </a>
        )}
        {isConfigurableProvider(provider.id) && (
          <ProviderConfigForm provider={provider.id} />
        )}
      </CardContent>
    </Card>
  );
}
