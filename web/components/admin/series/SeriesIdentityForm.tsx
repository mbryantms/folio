"use client";

import * as React from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { useSeries } from "@/lib/api/queries";
import { useTriggerSeriesScan, useUpdateSeries } from "@/lib/api/mutations";

export function SeriesIdentityForm({ id }: { id: string }) {
  const series = useSeries(id);
  const update = useUpdateSeries(id);
  const rescan = useTriggerSeriesScan(id, series.data?.library_id);
  // The match_key isn't in SeriesView yet; this form treats it as set-or-clear
  // only. Future server change will surface the current override and a reset
  // effect can land then.
  const [matchKey, setMatchKey] = React.useState("");

  if (series.isLoading) return <Skeleton className="h-48 w-full" />;
  if (series.error || !series.data) {
    return <p className="text-destructive text-sm">Series not found.</p>;
  }

  return (
    <div className="space-y-6">
      <header>
        <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
          Series
        </p>
        <h1 className="text-2xl font-semibold tracking-tight">
          {series.data.name}
        </h1>
        <p className="text-muted-foreground text-xs">
          Library: <span className="font-mono">{series.data.library_id}</span>
        </p>
      </header>
      <Card>
        <CardContent className="space-y-4 p-5">
          <div className="space-y-2">
            <Label htmlFor="match-key">Identity override (match_key)</Label>
            <p className="text-muted-foreground text-xs">
              Sticky identifier the scanner will not overwrite. Leave empty to
              clear.
            </p>
            <div className="flex gap-2">
              <Input
                id="match-key"
                value={matchKey}
                onChange={(e) => setMatchKey(e.target.value)}
                placeholder="custom-identity-key"
                className="font-mono"
              />
              <Button
                onClick={() =>
                  update.mutate({
                    match_key: matchKey.trim() === "" ? null : matchKey.trim(),
                  })
                }
                disabled={update.isPending}
              >
                Save
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardContent className="flex items-center justify-between gap-4 p-5">
          <div className="space-y-1">
            <p className="text-foreground font-medium">Scan series</p>
            <p className="text-muted-foreground text-xs">
              Targeted rescan of just this series&apos;s folder. Faster than
              Scan library.
            </p>
          </div>
          <Button onClick={() => rescan.mutate()} disabled={rescan.isPending}>
            {rescan.isPending ? "Triggering…" : "Scan series"}
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}
