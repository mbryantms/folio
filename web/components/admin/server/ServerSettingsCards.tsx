"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { useUpdateSettings } from "@/lib/api/mutations";
import { useAdminSettings } from "@/lib/api/queries";

/** Hardening + diagnostics cards on /admin/server. M4 of the
 *  runtime-config-admin plan. The Tokens (JWT TTLs) card lives under
 *  /admin/auth alongside the other identity knobs. */
export function ServerSettingsCards() {
  const settings = useAdminSettings();
  if (settings.isLoading) return <Skeleton className="h-48 w-full" />;
  if (settings.error || !settings.data) {
    return (
      <p className="text-destructive text-sm">
        Failed to load server settings.
      </p>
    );
  }

  const rows = settings.data.values;
  const asBool = (k: string, fallback: boolean) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "boolean" ? r.value : fallback;
  };
  const asString = (k: string, fallback: string) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : fallback;
  };

  const asUint = (k: string, fallback: number) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "number" ? r.value : fallback;
  };

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <HardeningCard initial={asBool("auth.rate_limit_enabled", true)} />
        <DiagnosticsCard
          initial={asString("observability.log_level", "info")}
        />
      </div>
      <CompatibilityCard initial={asString("compat.opds_panels_mode", "off")} />
      <CachingCard initial={asUint("cache.zip_lru_capacity", 64)} />
      <WorkersCard
        initial={{
          scan_count: asUint("workers.scan_count", 4),
          post_scan_count: asUint("workers.post_scan_count", 2),
          scan_batch_size: asUint("workers.scan_batch_size", 100),
          scan_hash_buffer_kb: asUint("workers.scan_hash_buffer_kb", 1024),
          archive_work_parallel: asUint("workers.archive_work_parallel", 4),
          thumb_inline_parallel: asUint("workers.thumb_inline_parallel", 8),
        }}
      />
    </div>
  );
}

/** progress-writeback-2.0 M4: OPDS client compatibility mode toggle.
 *  Default off (Folio identity preserved). When `komga`, the OPDS feed
 *  presents as Komga so Panels (iOS) and Tachiyomi-class clients sync
 *  reading progress via the Komga REST API they're hardcoded against.
 *  Live — no restart needed. */
function CompatibilityCard({ initial }: { initial: string }) {
  const [mode, setMode] = useState<"off" | "komga">(
    initial === "komga" ? "komga" : "off",
  );
  const update = useUpdateSettings();
  const dirty = mode !== initial;

  async function onSave() {
    await update.mutateAsync({ "compat.opds_panels_mode": mode });
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          OPDS client compatibility
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant={mode === "off" ? "default" : "outline"}
              size="sm"
              onClick={() => setMode("off")}
            >
              Off (Folio identity)
            </Button>
            <Button
              type="button"
              variant={mode === "komga" ? "default" : "outline"}
              size="sm"
              onClick={() => setMode("komga")}
            >
              Komga compatibility
            </Button>
          </div>
          <p className="text-muted-foreground text-xs">
            Komga mode makes Folio&apos;s OPDS feed present as Komga so apps
            that hardcode Komga support (Panels on iOS/macOS, Tachiyomi / Mihon
            / Yokai on Android) can sync reading progress back to the server.
            The feed&apos;s <code>&lt;author&gt;</code> element will display
            &ldquo;Komga&rdquo; while this is on — harmless for normal OPDS
            clients. Spec-clean alternative (OPDS Progression 1.0) is also
            active regardless of this flag, but no client implements it yet.
          </p>
          {mode === "komga" && (
            <div className="border-border bg-background/40 rounded-md border p-3 text-xs">
              <p className="text-foreground font-medium">
                Panels iOS / Android — extra setup
              </p>
              <p className="text-muted-foreground mt-1">
                Panels detects Komga via the OPDS fingerprint but does NOT
                propagate its OPDS username/password to the Komga REST writer
                that handles progress sync. Operators on Panels-class clients
                need three things:
              </p>
              <ol className="text-muted-foreground mt-2 list-decimal space-y-1 pl-5">
                <li>
                  In the client, set the OPDS source URL to{" "}
                  <code className="bg-secondary/40 rounded px-1 py-0.5">
                    /opds/v1.2/catalog
                  </code>{" "}
                  (Komga&apos;s canonical entry — the catalog alias is
                  registered regardless of this toggle).
                </li>
                <li>
                  Issue an app password with the{" "}
                  <span className="font-medium">read + write progress</span>{" "}
                  scope from{" "}
                  <a
                    href="/settings/api-tokens"
                    className="text-primary underline-offset-2 hover:underline"
                  >
                    Settings → API tokens
                  </a>
                  . The issued-password dialog now shows a pre-computed{" "}
                  <code className="bg-secondary/40 rounded px-1 py-0.5">
                    Basic …
                  </code>{" "}
                  header value — copy that.
                </li>
                <li>
                  In Panels&apos; source settings, paste the{" "}
                  <code className="bg-secondary/40 rounded px-1 py-0.5">
                    Basic …
                  </code>{" "}
                  value into the{" "}
                  <span className="font-medium">Custom headers</span> field with
                  key{" "}
                  <code className="bg-secondary/40 rounded px-1 py-0.5">
                    Authorization
                  </code>
                  .
                </li>
              </ol>
              <p className="text-muted-foreground mt-2">
                Without step 3 every progress PATCH is rejected by Folio&apos;s
                CSRF / auth gate and silently 401/403 — Panels won&apos;t
                surface the failure in its UI.
              </p>
            </div>
          )}
        </div>
        <div className="flex justify-end">
          <Button onClick={onSave} disabled={!dirty || update.isPending}>
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function HardeningCard({ initial }: { initial: boolean }) {
  const [enabled, setEnabled] = useState(initial);
  const update = useUpdateSettings();
  const dirty = enabled !== initial;

  async function onSave() {
    await update.mutateAsync({ "auth.rate_limit_enabled": enabled });
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Hardening
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center justify-between">
          <div>
            <Label className="text-sm">Rate limiting</Label>
            <p className="text-muted-foreground text-xs">
              Toggles the failed-auth Redis lockout (10 fails/min/IP → 15-min
              lockout). Per-route governor buckets stay installed regardless.
            </p>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>
        <div className="flex justify-end">
          <Button onClick={onSave} disabled={!dirty || update.isPending}>
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function RestartHint() {
  return (
    <p className="text-muted-foreground text-xs">
      <span className="text-amber-300">Restart required</span> — values are read
      at boot to size the worker pool / cache. Save now to pre-load for the next
      restart.
    </p>
  );
}

function CachingCard({ initial }: { initial: number }) {
  const [capacity, setCapacity] = useState(String(initial));
  const update = useUpdateSettings();
  const bad =
    capacity !== String(initial) &&
    (!/^\d+$/.test(capacity) ||
      Number(capacity) < 1 ||
      Number(capacity) > 4096);
  const dirty = Number(capacity) !== initial;

  async function onSave() {
    await update.mutateAsync({ "cache.zip_lru_capacity": Number(capacity) });
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Caching
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="space-y-2">
          <Label htmlFor="zip-lru-capacity">
            ZIP LRU capacity (open file descriptors)
          </Label>
          <Input
            id="zip-lru-capacity"
            inputMode="numeric"
            value={capacity}
            onChange={(e) => setCapacity(e.target.value)}
          />
          {bad && <p className="text-xs text-red-400">Must be in [1, 4096].</p>}
          <RestartHint />
        </div>
        <div className="flex justify-end">
          <Button onClick={onSave} disabled={!dirty || update.isPending || bad}>
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function WorkersCard({
  initial,
}: {
  initial: {
    scan_count: number;
    post_scan_count: number;
    scan_batch_size: number;
    scan_hash_buffer_kb: number;
    archive_work_parallel: number;
    thumb_inline_parallel: number;
  };
}) {
  const [state, setState] = useState({
    scan_count: String(initial.scan_count),
    post_scan_count: String(initial.post_scan_count),
    scan_batch_size: String(initial.scan_batch_size),
    scan_hash_buffer_kb: String(initial.scan_hash_buffer_kb),
    archive_work_parallel: String(initial.archive_work_parallel),
    thumb_inline_parallel: String(initial.thumb_inline_parallel),
  });
  const update = useUpdateSettings();

  function checkRange(value: string, min: number, max: number) {
    if (!/^\d+$/.test(value)) return false;
    const n = Number(value);
    return n >= min && n <= max;
  }

  const valid = {
    scan_count: checkRange(state.scan_count, 1, 64),
    post_scan_count: checkRange(state.post_scan_count, 1, 64),
    scan_batch_size: checkRange(state.scan_batch_size, 1, 10000),
    scan_hash_buffer_kb: checkRange(state.scan_hash_buffer_kb, 64, 65536),
    archive_work_parallel: checkRange(state.archive_work_parallel, 1, 64),
    thumb_inline_parallel: checkRange(state.thumb_inline_parallel, 1, 64),
  };
  const allValid = Object.values(valid).every(Boolean);
  const dirty =
    Number(state.scan_count) !== initial.scan_count ||
    Number(state.post_scan_count) !== initial.post_scan_count ||
    Number(state.scan_batch_size) !== initial.scan_batch_size ||
    Number(state.scan_hash_buffer_kb) !== initial.scan_hash_buffer_kb ||
    Number(state.archive_work_parallel) !== initial.archive_work_parallel ||
    Number(state.thumb_inline_parallel) !== initial.thumb_inline_parallel;

  async function onSave() {
    const patch: Record<string, number> = {};
    if (Number(state.scan_count) !== initial.scan_count)
      patch["workers.scan_count"] = Number(state.scan_count);
    if (Number(state.post_scan_count) !== initial.post_scan_count)
      patch["workers.post_scan_count"] = Number(state.post_scan_count);
    if (Number(state.scan_batch_size) !== initial.scan_batch_size)
      patch["workers.scan_batch_size"] = Number(state.scan_batch_size);
    if (Number(state.scan_hash_buffer_kb) !== initial.scan_hash_buffer_kb)
      patch["workers.scan_hash_buffer_kb"] = Number(state.scan_hash_buffer_kb);
    if (Number(state.archive_work_parallel) !== initial.archive_work_parallel)
      patch["workers.archive_work_parallel"] = Number(
        state.archive_work_parallel,
      );
    if (Number(state.thumb_inline_parallel) !== initial.thumb_inline_parallel)
      patch["workers.thumb_inline_parallel"] = Number(
        state.thumb_inline_parallel,
      );
    await update.mutateAsync(patch);
  }

  type Field = keyof typeof state;
  const inputs: Array<{
    field: Field;
    label: string;
    hint: string;
    range: string;
  }> = [
    {
      field: "scan_count",
      label: "Scan workers",
      hint: "Parallel library scan jobs.",
      range: "[1, 64]",
    },
    {
      field: "post_scan_count",
      label: "Post-scan workers",
      hint: "Parallel thumbnail / search jobs after a scan.",
      range: "[1, 64]",
    },
    {
      field: "scan_batch_size",
      label: "Scan batch size",
      hint: "Issues per DB transaction during scan.",
      range: "[1, 10000]",
    },
    {
      field: "scan_hash_buffer_kb",
      label: "Hash buffer (KB)",
      hint: "BLAKE3 streaming buffer per worker.",
      range: "[64, 65536]",
    },
    {
      field: "archive_work_parallel",
      label: "Archive work parallel",
      hint: "Global cap on blocking archive I/O.",
      range: "[1, 64]",
    },
    {
      field: "thumb_inline_parallel",
      label: "Thumb inline parallel",
      hint: "Cap on on-demand thumbnail generation.",
      range: "[1, 64]",
    },
  ];

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Workers
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {inputs.map(({ field, label, hint, range }) => (
            <div key={field} className="space-y-2">
              <Label htmlFor={`workers-${field}`}>{label}</Label>
              <Input
                id={`workers-${field}`}
                inputMode="numeric"
                value={state[field]}
                onChange={(e) =>
                  setState((s) => ({ ...s, [field]: e.target.value }))
                }
              />
              {!valid[field] && (
                <p className="text-xs text-red-400">Range: {range}</p>
              )}
              <p className="text-muted-foreground text-xs">{hint}</p>
            </div>
          ))}
        </div>
        <RestartHint />
        <div className="flex justify-end">
          <Button
            onClick={onSave}
            disabled={!dirty || update.isPending || !allValid}
          >
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

const LOG_LEVELS = ["trace", "debug", "info", "warn", "error"] as const;

function DiagnosticsCard({ initial }: { initial: string }) {
  const [level, setLevel] = useState(initial);
  const update = useUpdateSettings();
  const dirty = level !== initial;

  async function onSave() {
    await update.mutateAsync({ "observability.log_level": level });
  }

  // EnvFilter accepts both bare levels (e.g. "debug") and module-scoped
  // directives (e.g. "info,server::auth=debug"). The dropdown sets the
  // simple level; advanced operators can type a custom directive.
  const isStandard = LOG_LEVELS.includes(level as (typeof LOG_LEVELS)[number]);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Diagnostics
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="log-level">Log level</Label>
          <div className="flex flex-wrap gap-2">
            {LOG_LEVELS.map((l) => (
              <Button
                key={l}
                type="button"
                variant={level === l ? "default" : "outline"}
                size="sm"
                onClick={() => setLevel(l)}
              >
                {l}
              </Button>
            ))}
          </div>
          <Label className="text-muted-foreground mt-3 block text-xs">
            Or a custom EnvFilter directive
          </Label>
          <Input
            id="log-level"
            value={isStandard ? "" : level}
            onChange={(e) => setLevel(e.target.value)}
            placeholder="e.g. info,server::auth=debug"
          />
          <p className="text-muted-foreground text-xs">
            Live-reloaded on save — no restart needed. Invalid directives return
            400 before the swap.
          </p>
        </div>
        <div className="flex justify-end">
          <Button onClick={onSave} disabled={!dirty || update.isPending}>
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
