"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useUpdateSettings } from "@/lib/api/mutations";

const DURATION_RE = /^\d+[smhd]$/;

/** Access + refresh JWT TTL inputs. Server-side validation in
 *  `Config::parse_duration` accepts `\d+[smhd]` (e.g. `15m`, `24h`, `30d`)
 *  and cross-validates `refresh >= access`. We mirror the unit check
 *  client-side so the most common mistakes don't round-trip to a 400. */
export function TokensCard({
  initial,
}: {
  initial: { access_ttl: string; refresh_ttl: string };
}) {
  const [access, setAccess] = useState(initial.access_ttl);
  const [refresh, setRefresh] = useState(initial.refresh_ttl);
  const update = useUpdateSettings();

  const accessBad = access !== initial.access_ttl && !DURATION_RE.test(access);
  const refreshBad =
    refresh !== initial.refresh_ttl && !DURATION_RE.test(refresh);
  const dirty =
    access !== initial.access_ttl || refresh !== initial.refresh_ttl;

  async function onSave() {
    // `disabled={!dirty}` on the submit button makes the no-op path
    // unreachable, so we don't need the in-handler short-circuit.
    const patch: Record<string, unknown> = {};
    if (access !== initial.access_ttl) patch["auth.jwt.access_ttl"] = access;
    if (refresh !== initial.refresh_ttl) patch["auth.jwt.refresh_ttl"] = refresh;
    await update.mutateAsync(patch);
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Tokens
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="space-y-2">
            <Label htmlFor="access-ttl">Access TTL</Label>
            <Input
              id="access-ttl"
              value={access}
              onChange={(e) => setAccess(e.target.value)}
              placeholder="24h"
            />
            {accessBad && (
              <p className="text-xs text-red-400">
                Use a duration like <code>15m</code>, <code>24h</code>, or{" "}
                <code>30d</code>.
              </p>
            )}
            <p className="text-muted-foreground text-xs">
              How long an access cookie is valid before silent refresh kicks in.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="refresh-ttl">Refresh TTL</Label>
            <Input
              id="refresh-ttl"
              value={refresh}
              onChange={(e) => setRefresh(e.target.value)}
              placeholder="30d"
            />
            {refreshBad && (
              <p className="text-xs text-red-400">
                Use a duration like <code>15m</code>, <code>24h</code>, or{" "}
                <code>30d</code>.
              </p>
            )}
            <p className="text-muted-foreground text-xs">
              The &ldquo;stay signed in&rdquo; window. Must be ≥ access TTL —
              server-side validation rejects shorter values.
            </p>
          </div>
        </div>
        <div className="flex items-center justify-end gap-3">
          <span className="text-muted-foreground text-xs">
            New tokens use the new TTLs. Existing tokens keep their original
            expiry.
          </span>
          <Button
            onClick={onSave}
            disabled={!dirty || update.isPending || accessBad || refreshBad}
          >
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
