"use client";

/**
 * The Mylar card on `/admin/upcoming` (upcoming-releases 1.0).
 *
 * Folio never calls Mylar. A script on the Mylar host pushes a read-only
 * snapshot here, authenticated by the ingest token this card creates.
 * So the card has a switch, a token, and what the last snapshot said —
 * and nothing that acts on Mylar.
 */

import { AlertTriangle, KeyRound, Loader2 } from "lucide-react";
import * as React from "react";

import { CopyButton } from "@/components/folio/copy-button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  useCreateMylarToken,
  useRevokeMylarToken,
  useUpdateSettings,
} from "@/lib/api/mutations";
import type { MylarStatusView } from "@/lib/api/types";
import { formatRelativeDate } from "@/lib/format";

/** Bounds of `integrations.mylar.stale_after_minutes` (server-validated). */
export const STALE_MINUTES_MIN = 30;
export const STALE_MINUTES_MAX = 7 * 24 * 60;

export function MylarCard({
  mylar,
  upcomingEnabled,
}: {
  mylar: MylarStatusView;
  /** `upcoming.enabled`: snapshots are refused while it is off. */
  upcomingEnabled: boolean;
}) {
  return (
    <Card className="lg:col-span-2">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          Mylar
          {stateBadge(mylar, upcomingEnabled)}
        </CardTitle>
      </CardHeader>
      <CardContent className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <div className="space-y-5">
          <p className="text-muted-foreground text-sm">
            Folio can show what Mylar is tracking: this week&rsquo;s pull list,
            the watchlist and the download queue. It is view-only. A small
            script on the Mylar host pushes a snapshot here; Folio never
            contacts Mylar and cannot change anything in it.
          </p>
          <MylarSettingsForm
            key={`${mylar.enabled ? 1 : 0}-${mylar.stale_after_minutes}`}
            mylar={mylar}
          />
          <TokenSection mylar={mylar} />
        </div>
        <SnapshotStatus mylar={mylar} upcomingEnabled={upcomingEnabled} />
      </CardContent>
    </Card>
  );
}

function stateBadge(mylar: MylarStatusView, upcomingEnabled: boolean) {
  if (!mylar.enabled || !upcomingEnabled) {
    return (
      <Badge variant="outline" className="text-[10px]">
        OFF
      </Badge>
    );
  }
  if (!mylar.token) {
    return (
      <Badge variant="secondary" className="text-[10px]">
        NO TOKEN
      </Badge>
    );
  }
  const freshness = mylar.snapshot?.freshness;
  if (!freshness) {
    return (
      <Badge variant="secondary" className="text-[10px]">
        WAITING FOR A SNAPSHOT
      </Badge>
    );
  }
  if (freshness !== "fresh") {
    return (
      <Badge variant="destructive" className="text-[10px]">
        STALE
      </Badge>
    );
  }
  return (
    <Badge variant="default" className="text-[10px]">
      RECEIVING
    </Badge>
  );
}

function MylarSettingsForm({ mylar }: { mylar: MylarStatusView }) {
  const update = useUpdateSettings();
  const [enabled, setEnabled] = React.useState(mylar.enabled);
  const [stale, setStale] = React.useState(String(mylar.stale_after_minutes));
  const staleNum = Number(stale);
  const staleValid =
    Number.isInteger(staleNum) &&
    staleNum >= STALE_MINUTES_MIN &&
    staleNum <= STALE_MINUTES_MAX;
  const dirty =
    enabled !== mylar.enabled || staleNum !== mylar.stale_after_minutes;

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const patch: Record<string, unknown> = {};
    if (enabled !== mylar.enabled) {
      patch["integrations.mylar.enabled"] = enabled;
    }
    if (staleValid && staleNum !== mylar.stale_after_minutes) {
      patch["integrations.mylar.stale_after_minutes"] = staleNum;
    }
    if (Object.keys(patch).length === 0) return;
    try {
      await update.mutateAsync(patch);
    } catch {
      // useApiMutation already toasts on error.
    }
  };

  return (
    <form onSubmit={onSubmit} className="space-y-4">
      <div className="flex items-start gap-3">
        <Switch
          id="mylar-enabled"
          checked={enabled}
          onCheckedChange={setEnabled}
          className="mt-0.5"
        />
        <div className="grid gap-1">
          <Label htmlFor="mylar-enabled" className="cursor-pointer">
            Accept Mylar snapshots
          </Label>
          <p className="text-muted-foreground text-xs">
            While this is off, a push is refused and stored Mylar data is not
            shown.
          </p>
        </div>
      </div>
      <div className="grid gap-1.5">
        <Label htmlFor="mylar-stale-minutes">
          Show as stale after (minutes)
        </Label>
        <Input
          id="mylar-stale-minutes"
          type="number"
          inputMode="numeric"
          min={STALE_MINUTES_MIN}
          max={STALE_MINUTES_MAX}
          value={stale}
          onChange={(e) => setStale(e.target.value)}
          aria-invalid={!staleValid}
          aria-describedby="mylar-stale-minutes-help"
          className="w-28"
        />
        <p
          id="mylar-stale-minutes-help"
          className="text-muted-foreground text-xs"
        >
          {STALE_MINUTES_MIN} to {STALE_MINUTES_MAX.toLocaleString()}. The
          script sends at least one snapshot an hour, so 180 means three were
          missed.
        </p>
      </div>
      <Button
        type="submit"
        disabled={!dirty || !staleValid || update.isPending}
      >
        {update.isPending ? (
          <>
            <Loader2 className="mr-2 size-4 animate-spin" />
            Saving
          </>
        ) : (
          "Save"
        )}
      </Button>
    </form>
  );
}

function TokenSection({ mylar }: { mylar: MylarStatusView }) {
  const create = useCreateMylarToken();
  const revoke = useRevokeMylarToken();
  const [issued, setIssued] = React.useState<string | null>(null);
  const [confirmReplace, setConfirmReplace] = React.useState(false);
  const [confirmRevoke, setConfirmRevoke] = React.useState(false);
  const [purge, setPurge] = React.useState(false);
  const token = mylar.token ?? null;

  const generate = () =>
    create.mutate(undefined, {
      onSuccess: (data) => {
        if (data?.token) setIssued(data.token);
      },
    });

  return (
    <section className="border-border/40 space-y-2 border-t pt-4">
      <h3 className="flex items-center gap-2 text-sm font-medium">
        <KeyRound className="size-4" aria-hidden /> Ingest token
      </h3>
      {token ? (
        <p className="text-muted-foreground text-xs">
          Created {formatRelativeDate(token.created_at) ?? "earlier"}.{" "}
          {token.last_used_at
            ? `Last used ${formatRelativeDate(token.last_used_at)}${
                token.last_used_ip ? ` from ${token.last_used_ip}` : ""
              }.`
            : "Not used yet."}
        </p>
      ) : (
        <p className="text-muted-foreground text-xs">
          No token yet. Generate one and put it in the script&rsquo;s
          environment file on the Mylar host.
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="shrink-0 whitespace-nowrap"
          disabled={create.isPending}
          onClick={() => (token ? setConfirmReplace(true) : generate())}
        >
          {create.isPending ? (
            <Loader2 className="mr-1 size-3 animate-spin" />
          ) : null}
          {token ? "Replace token" : "Generate token"}
        </Button>
        {token ? (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="text-destructive shrink-0 whitespace-nowrap"
            disabled={revoke.isPending}
            onClick={() => {
              setPurge(false);
              setConfirmRevoke(true);
            }}
          >
            Revoke
          </Button>
        ) : null}
      </div>

      <AlertDialog open={confirmReplace} onOpenChange={setConfirmReplace}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Replace the ingest token?</AlertDialogTitle>
            <AlertDialogDescription>
              The current token stops working at once. Pushes from the Mylar
              host are refused until you put the new token in its environment
              file.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={generate}>Replace</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={confirmRevoke} onOpenChange={setConfirmRevoke}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke the ingest token?</AlertDialogTitle>
            <AlertDialogDescription>
              Pushes from the Mylar host are refused from now on. Nothing
              changes on the Mylar host itself.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="flex items-start gap-2">
            <Checkbox
              id="mylar-revoke-purge"
              checked={purge}
              onCheckedChange={(v) => setPurge(v === true)}
              className="mt-0.5"
            />
            <Label htmlFor="mylar-revoke-purge" className="cursor-pointer">
              Also delete the stored snapshot
            </Label>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => revoke.mutate({ purge })}>
              Revoke
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog
        open={issued !== null}
        onOpenChange={(open) => {
          if (!open) setIssued(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Your Mylar ingest token</DialogTitle>
            <DialogDescription>
              Copy it now. It is shown this once; Folio keeps only a fingerprint
              of it. It can do one thing: deliver a Mylar snapshot to this
              server.
            </DialogDescription>
          </DialogHeader>
          <div className="border-border bg-background flex items-start gap-2 rounded-md border p-2">
            <code
              className="text-foreground flex-1 font-mono text-xs break-all"
              data-testid="mylar-token"
            >
              {issued}
            </code>
            <CopyButton
              value={issued ?? ""}
              label="Copy"
              className="shrink-0"
            />
          </div>
          <p className="text-muted-foreground text-xs">
            On the Mylar host, set <code>FOLIO_TOKEN</code> in{" "}
            <code>/etc/folio-mylar-push.env</code>. The install steps are in{" "}
            <code>integrations/mylar/README.md</code>.
          </p>
          <DialogFooter>
            <Button type="button" onClick={() => setIssued(null)}>
              Done
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function SnapshotStatus({
  mylar,
  upcomingEnabled,
}: {
  mylar: MylarStatusView;
  upcomingEnabled: boolean;
}) {
  const snap = mylar.snapshot ?? null;
  const queue = snap
    ? Object.entries(snap.queue_counts)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(
          ([status, n]) =>
            `${Number(n).toLocaleString()} ${status.toLowerCase()}`,
        )
        .join(" · ")
    : "";
  return (
    <div className="space-y-3 text-sm">
      <h3 className="text-sm font-medium">Last snapshot</h3>
      {mylar.enabled && !upcomingEnabled ? (
        <Alert>
          <AlertTriangle className="size-4" aria-hidden />
          <AlertTitle>Upcoming releases is off</AlertTitle>
          <AlertDescription>
            Snapshots are refused until &ldquo;Show upcoming releases&rdquo; is
            on too.
          </AlertDescription>
        </Alert>
      ) : null}
      {!snap ? (
        <p className="text-muted-foreground text-xs">
          Nothing received yet. After the script&rsquo;s first run this shows
          when the snapshot arrived and what it held.
        </p>
      ) : (
        <>
          {snap.freshness !== "fresh" ? (
            <Alert variant="destructive">
              <AlertTriangle className="size-4" aria-hidden />
              <AlertTitle>
                {snap.freshness === "expired"
                  ? "No snapshot for over a week"
                  : "The snapshot is stale"}
              </AlertTitle>
              <AlertDescription>
                The last one arrived{" "}
                {formatRelativeDate(snap.received_at) ?? "a while ago"}. Check
                the timer on the Mylar host:{" "}
                <code>systemctl status folio-mylar-push.service</code>.
                {snap.freshness === "expired"
                  ? " Readers no longer see Mylar information."
                  : ""}
              </AlertDescription>
            </Alert>
          ) : null}
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
            <dt className="text-muted-foreground">Received</dt>
            <dd>{formatRelativeDate(snap.received_at) ?? snap.received_at}</dd>
            <dt className="text-muted-foreground">Pull list</dt>
            <dd className="tabular-nums">
              {snap.pull_count.toLocaleString()} lines
            </dd>
            <dt className="text-muted-foreground">Watchlist</dt>
            <dd className="tabular-nums">
              {snap.watch_count.toLocaleString()} series
              {snap.annuals_included ? ", annuals included" : ""}
            </dd>
            <dt className="text-muted-foreground">Wanted</dt>
            <dd className="tabular-nums">
              {snap.wanted_count.toLocaleString()} issues
            </dd>
            <dt className="text-muted-foreground">Download queue</dt>
            <dd className="tabular-nums">{queue || "empty"}</dd>
          </dl>
        </>
      )}
    </div>
  );
}
