"use client";

/**
 * `<SettingsTab>` for `/admin/metadata` (metadata-providers-1.0 M7
 * follow-up — added 2026-05-26 after user pushed back on the silent
 * scope-skip that left the plan's "Settings tab" gap unaddressed).
 *
 * Surfaces the operationally-toggleable `metadata.*` keys:
 *   - Weekly refresh enable + cron + recently-published window
 *   - Stale-after-days threshold (drives /libraries/{slug}/metadata/refresh?scope=stale)
 *
 * Writes go through the same `PATCH /admin/settings` endpoint the
 * generic settings surface uses; the cron-string flip needs a server
 * restart (tokio_cron_scheduler API limitation), the boolean flip is
 * live (the handler re-reads `Config.metadata_weekly_refresh_enabled`
 * on every cron fire).
 */

import { Loader2, RotateCw } from "lucide-react";
import * as React from "react";

import { CronInput } from "@/components/admin/library/CronInput";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { useUpdateSettings } from "@/lib/api/mutations";
import { useAdminSettings } from "@/lib/api/queries";
import { statusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

type RefreshSettings = {
  enabled: boolean;
  cron: string;
  windowDays: number;
  staleAfterDays: number;
  autoApplyThreshold: number;
  matchMediumThreshold: number;
};

function readInitial(values: Record<string, unknown>): RefreshSettings {
  const bool = (k: string, dflt: boolean) =>
    typeof values[k] === "boolean" ? (values[k] as boolean) : dflt;
  const str = (k: string, dflt: string) =>
    typeof values[k] === "string" && (values[k] as string).length > 0
      ? (values[k] as string)
      : dflt;
  const num = (k: string, dflt: number) => {
    const v = values[k];
    if (typeof v === "number" && Number.isFinite(v)) return v;
    if (typeof v === "string") {
      const parsed = Number(v);
      return Number.isFinite(parsed) ? parsed : dflt;
    }
    return dflt;
  };
  return {
    enabled: bool("metadata.weekly_refresh_enabled", false),
    cron: str("metadata.weekly_refresh_cron", "0 0 4 * * 0"),
    windowDays: num("metadata.weekly_refresh_window_days", 14),
    staleAfterDays: num("metadata.stale_after_days", 180),
    autoApplyThreshold: num("metadata.auto_apply_threshold", 80),
    matchMediumThreshold: num("metadata.match_medium_threshold", 60),
  };
}

export function SettingsTab() {
  const settings = useAdminSettings();
  const update = useUpdateSettings();

  if (settings.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading settings…
      </div>
    );
  }
  if (!settings.data) return null;
  const byKey: Record<string, unknown> = {};
  for (const row of settings.data.values) {
    byKey[row.key] = row.value;
  }
  const initial = readInitial(byKey);
  const formKey = `${initial.enabled ? "1" : "0"}-${initial.cron}-${initial.windowDays}-${initial.staleAfterDays}-${initial.autoApplyThreshold}-${initial.matchMediumThreshold}`;

  return (
    <SettingsForm
      key={formKey}
      initial={initial}
      isPending={update.isPending}
      onSubmit={async (patch) => {
        if (Object.keys(patch).length === 0) return;
        try {
          await update.mutateAsync(patch);
        } catch {
          // useApiMutation already toasts on error.
        }
      }}
    />
  );
}

function SettingsForm({
  initial,
  isPending,
  onSubmit,
}: {
  initial: RefreshSettings;
  isPending: boolean;
  onSubmit: (patch: Record<string, unknown>) => Promise<void>;
}) {
  const [enabled, setEnabled] = React.useState(initial.enabled);
  const [cron, setCron] = React.useState(initial.cron);
  const [windowDays, setWindowDays] = React.useState(String(initial.windowDays));
  const [staleAfterDays, setStaleAfterDays] = React.useState(
    String(initial.staleAfterDays),
  );
  const [autoApply, setAutoApply] = React.useState(
    String(initial.autoApplyThreshold),
  );
  const [matchMedium, setMatchMedium] = React.useState(
    String(initial.matchMediumThreshold),
  );

  const dirty =
    enabled !== initial.enabled ||
    cron.trim() !== initial.cron ||
    Number(windowDays) !== initial.windowDays ||
    Number(staleAfterDays) !== initial.staleAfterDays ||
    Number(autoApply) !== initial.autoApplyThreshold ||
    Number(matchMedium) !== initial.matchMediumThreshold;

  const handle = (e: React.FormEvent) => {
    e.preventDefault();
    const patch: Record<string, unknown> = {};
    if (enabled !== initial.enabled) {
      patch["metadata.weekly_refresh_enabled"] = enabled;
    }
    const trimmedCron = cron.trim();
    if (trimmedCron !== initial.cron && trimmedCron !== "") {
      patch["metadata.weekly_refresh_cron"] = trimmedCron;
    }
    const w = Number(windowDays);
    if (Number.isFinite(w) && w >= 0 && w !== initial.windowDays) {
      patch["metadata.weekly_refresh_window_days"] = w;
    }
    const s = Number(staleAfterDays);
    if (Number.isFinite(s) && s >= 0 && s !== initial.staleAfterDays) {
      patch["metadata.stale_after_days"] = s;
    }
    const a = Number(autoApply);
    if (
      Number.isFinite(a)
      && a >= 0
      && a <= 100
      && a !== initial.autoApplyThreshold
    ) {
      patch["metadata.auto_apply_threshold"] = a;
    }
    const m = Number(matchMedium);
    if (
      Number.isFinite(m)
      && m >= 0
      && m <= 100
      && m !== initial.matchMediumThreshold
    ) {
      patch["metadata.match_medium_threshold"] = m;
    }
    void onSubmit(patch);
  };

  return (
    <form onSubmit={handle} className="max-w-2xl space-y-6">
      <section className="space-y-3">
        <header className="space-y-1">
          <h3 className="text-base font-semibold">Weekly refresh</h3>
          <p className="text-muted-foreground text-xs">
            Off by default. When enabled, a background cron walks every
            library on the schedule below and re-fetches metadata for
            recently-published series (Mylar pattern) plus any series
            that crosses the staleness threshold. Burns provider quota —
            opt in only when you&rsquo;ve verified your CV / Metron
            allowances will absorb the weekly load.
          </p>
        </header>

        <div className="flex items-center gap-3 pt-1">
          <Switch
            id="weekly-enabled"
            checked={enabled}
            onCheckedChange={setEnabled}
          />
          <Label htmlFor="weekly-enabled" className="cursor-pointer text-sm">
            Enable weekly refresh
          </Label>
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="weekly-cron">Cron expression</Label>
          <CronInput
            id="weekly-cron"
            value={cron}
            onChange={setCron}
            placeholder="0 0 4 * * 0"
          />
          <p className="text-muted-foreground text-[11px]">
            6-field format (sec min hour day month weekday). Default
            <code className="mx-1">0 0 4 * * 0</code>= Sunday 04:00 UTC.
          </p>
          {cron.trim() !== initial.cron ? (
            <p
              className={cn(
                "flex items-start gap-2 rounded-md border px-3 py-2 text-[11px]",
                statusTone("warning"),
              )}
            >
              <RotateCw className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>
                Schedule changes take effect on the next server restart. The
                enable toggle above applies live.
              </span>
            </p>
          ) : null}
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="weekly-window">
            Recently-published window (days)
          </Label>
          <Input
            id="weekly-window"
            type="number"
            min={0}
            value={windowDays}
            onChange={(e) => setWindowDays(e.target.value)}
            className="w-32"
          />
          <p className="text-muted-foreground text-[11px]">
            Series whose latest issue was added within this window get
            re-fetched on every weekly run regardless of staleness.
            Default 14.
          </p>
        </div>
      </section>

      <section className="space-y-3 border-t border-border/40 pt-5">
        <header className="space-y-1">
          <h3 className="text-base font-semibold">Staleness</h3>
          <p className="text-muted-foreground text-xs">
            Drives both the weekly cron&rsquo;s older-than-window
            branch and the
            <code className="mx-1">scope=stale</code>
            option on
            <code className="mx-1">
              POST /libraries/{`{slug}`}/metadata/refresh
            </code>
            .
          </p>
        </header>
        <div className="grid gap-1.5">
          <Label htmlFor="stale-days">Stale after (days)</Label>
          <Input
            id="stale-days"
            type="number"
            min={0}
            value={staleAfterDays}
            onChange={(e) => setStaleAfterDays(e.target.value)}
            className="w-32"
          />
          <p className="text-muted-foreground text-[11px]">
            A series is &ldquo;stale&rdquo; when
            <code className="mx-1">last_metadata_sync_at</code>
            is null or older than this many days. Default 180.
          </p>
        </div>
      </section>

      <section className="space-y-3 border-t border-border/40 pt-5">
        <header className="space-y-1">
          <h3 className="text-base font-semibold">Match thresholds</h3>
          <p className="text-muted-foreground text-xs">
            Score (0&ndash;100) cutoffs the matcher uses to bucket
            candidates. The HIGH threshold defaults to 80 so legitimate
            text-only matches reach &ldquo;Strong match&rdquo; status;
            tighten it once you&rsquo;ve verified the matcher is
            consistent on your library, or loosen if you&rsquo;re
            getting too many candidates dumped to review.
          </p>
        </header>
        <div className="grid gap-1.5">
          <Label htmlFor="auto-apply-threshold">
            Auto-apply / HIGH threshold
          </Label>
          <Input
            id="auto-apply-threshold"
            type="number"
            min={0}
            max={100}
            value={autoApply}
            onChange={(e) => setAutoApply(e.target.value)}
            className="w-32"
          />
          <p className="text-muted-foreground text-[11px]">
            Candidates scoring at or above this land in the HIGH bucket
            (one-click apply, surfaced as &ldquo;Strong match&rdquo;).
            Default 80. ComicTagger&rsquo;s reference value is 90 — go
            higher if you want stricter, lower if matches keep landing
            in Medium.
          </p>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="match-medium-threshold">
            MEDIUM threshold
          </Label>
          <Input
            id="match-medium-threshold"
            type="number"
            min={0}
            max={100}
            value={matchMedium}
            onChange={(e) => setMatchMedium(e.target.value)}
            className="w-32"
          />
          <p className="text-muted-foreground text-[11px]">
            Candidates between this and the HIGH threshold land in the
            MEDIUM bucket (visible in the review queue). Below this is
            LOW (hidden by default). Default 60.
          </p>
        </div>
      </section>

      <div className="flex items-center gap-2 border-t border-border/40 pt-4">
        <Button type="submit" disabled={!dirty || isPending}>
          {isPending ? (
            <>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Saving
            </>
          ) : (
            "Save"
          )}
        </Button>
      </div>
    </form>
  );
}
