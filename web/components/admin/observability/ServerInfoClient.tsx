"use client";

import {
  Database,
  ExternalLink,
  Github,
  HardDrive,
  ListChecks,
  Eye,
  ScanText,
  Sparkles,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useLatestRelease,
  useOcrModels,
  useServerInfo,
} from "@/lib/api/queries";
import type {
  LatestReleaseView,
  OcrModelView,
  ServerInfoView,
} from "@/lib/api/types";
import { cn } from "@/lib/utils";

export function ServerInfoClient() {
  const info = useServerInfo({ intervalMs: 15_000 });

  if (info.isLoading || !info.data) {
    return <Skeleton className="h-72 w-full" />;
  }
  if (info.error) {
    return (
      <p className="text-destructive text-sm">Failed to load server info.</p>
    );
  }
  const data = info.data;

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <BuildCard data={data} />
      <DependenciesCard data={data} />
      <RuntimeCard data={data} />
      <LinksCard />
      <OcrModelsCard />
    </div>
  );
}

function OcrModelsCard() {
  // Poll every 30 s — if any model is downloading the card refreshes
  // mid-fetch, giving the operator a near-live progress readout
  // without aggressively hitting the disk walker. Stable once
  // everything's `ready` (the response shape doesn't change), so the
  // extra requests are cheap.
  const models = useOcrModels();
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          OCR models
        </CardTitle>
      </CardHeader>
      <CardContent>
        {models.isLoading || !models.data ? (
          <Skeleton className="h-24 w-full" />
        ) : models.error ? (
          <p className="text-destructive text-sm">
            Failed to load OCR model status.
          </p>
        ) : (
          <ul className="space-y-2 text-sm">
            {models.data.models.map((m) => (
              <OcrModelRow key={m.id} model={m} />
            ))}
            <li className="border-border/40 mt-2 flex items-baseline justify-between border-t pt-2">
              <span className="text-muted-foreground inline-flex items-center gap-2">
                <ScanText className="h-3.5 w-3.5" />
                Total on disk
              </span>
              <span className="text-foreground font-mono tabular-nums">
                {formatBytes(models.data.total_bytes_on_disk)}
              </span>
            </li>
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

function OcrModelRow({ model }: { model: OcrModelView }) {
  const state = classifyModelState(model);
  return (
    <li className="flex items-baseline justify-between gap-3">
      <span
        className="text-muted-foreground min-w-0 truncate"
        title={`${model.purpose}\n${model.cache_dir}`}
      >
        {model.id}
      </span>
      <span className="text-foreground inline-flex items-center gap-2 font-mono tabular-nums">
        {model.present ? formatBytes(model.bytes_on_disk) : "—"}
        <Badge
          variant="outline"
          className={cn(
            state.kind === "ready" && "border-emerald-500/40 text-emerald-400",
            state.kind === "downloading" &&
              "border-amber-500/40 text-amber-300",
            state.kind === "missing" &&
              "border-muted-foreground/30 text-muted-foreground",
          )}
        >
          {state.label}
        </Badge>
      </span>
    </li>
  );
}

/** Three-state classification used by the model tile.
 *
 *  - `ready` — `bytes_on_disk` is at least 95% of
 *    `expected_bytes_approx`. We accept slight over-/undershoot
 *    because `expected_bytes_approx` is a hand-picked round number,
 *    not a sha-pinned download manifest.
 *  - `downloading` — at least one byte on disk but below the
 *    `ready` threshold. Operators see this between "first OCR
 *    request triggered the HF fetch" and "fetch completed".
 *  - `missing` — nothing on disk. First OCR call will fetch.
 *
 *  Exported for unit tests.
 */
export function classifyModelState(model: OcrModelView): {
  kind: "ready" | "downloading" | "missing";
  pct: number;
  label: string;
} {
  if (!model.present || model.bytes_on_disk === 0) {
    return { kind: "missing", pct: 0, label: "missing" };
  }
  const pct =
    model.expected_bytes_approx > 0
      ? Math.min(
          100,
          Math.round((model.bytes_on_disk / model.expected_bytes_approx) * 100),
        )
      : 100;
  if (pct >= 95) return { kind: "ready", pct, label: "ready" };
  return { kind: "downloading", pct, label: `downloading ${pct}%` };
}

/** Compact byte formatting — picks the largest unit that keeps the
 *  display under 4 characters. `1024^2 → "1.0 MB"`. */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "kB", "MB", "GB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return `${value < 10 && i > 0 ? value.toFixed(1) : Math.round(value)} ${units[i]}`;
}

function BuildCard({ data }: { data: ServerInfoView }) {
  const versionHref = releaseUrl(data.repo_url, data.version);
  const shaHref = commitUrl(data.repo_url, data.build_sha_full);
  const repoLabel = repoDisplay(data.repo_url);
  // Latest-release check runs once on mount; cache TTL on both ends
  // (server: 1 hr, client: useQuery `staleTime: 1 hr`) means
  // repeated /admin/server visits don't poll GitHub.
  const latest = useLatestRelease();
  const newer = isNewerRelease(data.version, latest.data ?? null);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Build
        </CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="space-y-2 text-sm">
          <Row
            label="Version"
            value={data.version}
            mono
            href={versionHref}
          />
          <Row
            label="Build SHA"
            value={
              data.build_sha === "unknown"
                ? "unknown"
                : data.build_sha.slice(0, 12)
            }
            mono
            href={shaHref}
          />
          {data.build_epoch !== null && (
            <Row
              label="Built"
              value={formatRelativeFromEpoch(data.build_epoch)}
              title={new Date(data.build_epoch * 1000).toLocaleString()}
            />
          )}
          <Row label="Uptime" value={formatUptime(data.uptime_secs)} mono />
          {data.repo_url && repoLabel && (
            <Row label="Repository" value={repoLabel} href={data.repo_url} />
          )}
        </dl>
        {newer && latest.data && (
          <div className="border-border/60 mt-4 flex items-center justify-between gap-3 rounded-md border border-dashed bg-amber-500/5 px-3 py-2 text-sm">
            <span className="text-amber-300 inline-flex items-center gap-1.5">
              <Sparkles className="h-3.5 w-3.5" aria-hidden />
              {latest.data.tag} available
            </span>
            <a
              href={latest.data.html_url}
              target="_blank"
              rel="noreferrer noopener"
              className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 underline-offset-4 hover:underline"
            >
              release notes
              <ExternalLink className="h-3 w-3" aria-hidden />
            </a>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * `true` when `latest` represents a strictly newer version than
 * `current`. Strategy: pull `vX.Y.Z` numeric components out of both,
 * compare lexicographically as integer tuples. Pre-release suffixes
 * (`-rc.1`, `-3-gabcd1234`, `-dirty`) are ignored on the **current**
 * side — a user running `v0.1.8-3-gabcd1234` is past v0.1.8, so an
 * upstream `v0.1.8` is NOT newer; only `v0.1.9+` counts. Pre-release
 * tags on the **upstream** side are skipped (we don't promote rc /
 * beta tags to the "available" banner). Exported for unit tests.
 */
export function isNewerRelease(
  current: string,
  latest: LatestReleaseView | null,
): boolean {
  if (!latest) return false;
  const a = parseSemverPrefix(current);
  const b = parseSemverPrefix(latest.tag);
  if (!a || !b) return false;
  // The current side might be `v0.1.8-3-gabcd1234` — we want to treat
  // that as STRICTLY past v0.1.8, so upstream v0.1.8 is NOT newer.
  // `currentHasExtensions` lets us require strict-greater when the
  // tuples are equal but current has past-tag commits.
  const currentHasExtensions = /^v\d+(?:\.\d+)*-/.test(current);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const av = a[i] ?? 0;
    const bv = b[i] ?? 0;
    if (bv > av) return true;
    if (bv < av) return false;
  }
  // Tuples equal: only "newer" if current carries no past-tag suffix
  // (meaning current is exactly at the tag, and an equal upstream is
  // NOT newer — we don't want false-positive banners on clean tags).
  return !currentHasExtensions && false;
}

/** Extract the `[major, minor, patch, …]` integer tuple from a `vX.Y.Z`
 *  prefix. Returns `null` for shapes we can't compare (e.g. `"dev"`,
 *  bare SHA, pre-release-only). Exported for unit tests. */
export function parseSemverPrefix(version: string): number[] | null {
  const m = /^v(\d+(?:\.\d+)*)/.exec(version);
  if (!m) return null;
  return m[1]!.split(".").map((n) => parseInt(n, 10));
}

/**
 * Build a GitHub-compatible release URL when the version looks like a
 * clean tag (`vX.Y.Z`). The `git describe` extensions
 * (`v0.1.8-3-gabcd1234`, `v0.1.8-dirty`) are NOT linked — there's no
 * release page that matches them, and the user would land on a 404.
 * Exported for unit tests.
 */
export function releaseUrl(
  repoUrl: string | null,
  version: string,
): string | undefined {
  if (!repoUrl) return undefined;
  if (!/^v\d+(?:\.\d+)*$/.test(version)) return undefined;
  return `${repoUrl}/releases/tag/${encodeURIComponent(version)}`;
}

/** GitHub-compatible commit URL. Exported for unit tests. */
export function commitUrl(
  repoUrl: string | null,
  shaFull: string,
): string | undefined {
  if (!repoUrl) return undefined;
  if (!shaFull || shaFull === "unknown") return undefined;
  // GitHub also accepts short SHAs, but using the full one keeps the
  // link stable across history rewrites or shallow clones.
  return `${repoUrl}/commit/${encodeURIComponent(shaFull)}`;
}

/** `"github.com/mbryantms/folio"` from `"https://github.com/mbryantms/folio"`.
 *  Exported for unit tests. */
export function repoDisplay(repoUrl: string | null): string | null {
  if (!repoUrl) return null;
  // Strip protocol for a compact "github.com/owner/repo" label.
  return repoUrl.replace(/^https?:\/\//, "");
}

/** "N minutes/hours/days/months/years ago" for relative-time rows.
 *  Exported for unit tests. Accepts an explicit `now` for determinism
 *  in tests; defaults to wall clock. */
export function formatRelativeFromEpoch(
  epochSecs: number,
  now: number = Date.now(),
): string {
  const ageMs = now - epochSecs * 1000;
  if (ageMs < 0) return "just now";
  const minutes = Math.floor(ageMs / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

function DependenciesCard({ data }: { data: ServerInfoView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Dependencies
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <Pill
            icon={<Database className="h-3.5 w-3.5" />}
            label="Postgres"
            ok={data.postgres_ok}
          />
          <Pill
            icon={<HardDrive className="h-3.5 w-3.5" />}
            label="Redis"
            ok={data.redis_ok}
          />
        </ul>
      </CardContent>
    </Card>
  );
}

function RuntimeCard({ data }: { data: ServerInfoView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Runtime
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <Pill
            icon={<ListChecks className="h-3.5 w-3.5" />}
            label="Cron scheduler"
            ok={data.scheduler_running}
          />
          <li className="flex items-center justify-between">
            <span className="text-muted-foreground flex items-center gap-2">
              <Eye className="h-3.5 w-3.5" />
              Library file-watchers
            </span>
            <span className="text-foreground font-mono tabular-nums">
              {data.watchers_enabled}
            </span>
          </li>
        </ul>
      </CardContent>
    </Card>
  );
}

function LinksCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Probes &amp; metrics
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <ProbeLink
            href="/healthz"
            label="/healthz"
            hint="liveness probe"
          />
          <ProbeLink
            href="/readyz"
            label="/readyz"
            hint="readiness probe"
          />
          <ProbeLink
            href="/metrics"
            label="/metrics"
            hint="Prometheus exporter"
          />
        </ul>
      </CardContent>
    </Card>
  );
}

function Row({
  label,
  value,
  mono,
  href,
  title,
}: {
  label: string;
  value: string;
  mono?: boolean;
  /** When set, the value renders as an `<a target="_blank">` link.
   *  Falls back to plain text otherwise. The link gets a `rel` to
   *  block referrer leakage to GitHub (or whatever forge). */
  href?: string;
  /** Optional tooltip (`title=`) for hover-revealed absolute time on
   *  relative-date rows. */
  title?: string;
}) {
  const valueClasses = cn("text-foreground", mono && "font-mono tabular-nums");
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={cn("min-w-0 truncate", valueClasses)} title={title}>
        {href ? (
          <a
            href={href}
            target="_blank"
            rel="noreferrer noopener"
            className="hover:text-primary inline-flex items-center gap-1 underline-offset-4 hover:underline"
          >
            {value}
            {label === "Repository" ? (
              <Github className="h-3 w-3" aria-hidden />
            ) : (
              <ExternalLink className="h-3 w-3" aria-hidden />
            )}
          </a>
        ) : (
          value
        )}
      </dd>
    </div>
  );
}

function Pill({
  icon,
  label,
  ok,
}: {
  icon: React.ReactNode;
  label: string;
  ok: boolean;
}) {
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground flex items-center gap-2">
        {icon}
        {label}
      </span>
      <Badge
        variant="outline"
        className={
          ok
            ? "border-emerald-500/40 text-emerald-400"
            : "border-red-500/40 text-red-400"
        }
      >
        {ok ? "OK" : "Down"}
      </Badge>
    </li>
  );
}

function ProbeLink({
  href,
  label,
  hint,
}: {
  href: string;
  label: string;
  hint: string;
}) {
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground">{hint}</span>
      <a
        href={href}
        target="_blank"
        rel="noreferrer"
        className="text-foreground flex items-center gap-1 font-mono underline-offset-4 hover:underline"
      >
        {label}
        <ExternalLink className="h-3 w-3" />
      </a>
    </li>
  );
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const minutes = Math.floor(secs / 60);
  if (minutes < 60) return `${minutes}m ${secs % 60}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}
