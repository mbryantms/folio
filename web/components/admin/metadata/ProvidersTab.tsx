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
import { useTestMetadataProvider } from "@/lib/api/mutations";
import { useAdminMetadataProviders } from "@/lib/api/queries";
import type { RequestBudget, ProviderView } from "@/lib/api/types";
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
