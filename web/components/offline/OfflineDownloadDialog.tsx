"use client";

import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { useMe } from "@/lib/api/queries";
import type { IssueDetailView, PageInfo } from "@/lib/api/types";
import { formatBytes } from "@/lib/format";
import {
  defaultDownloadTier,
  estimateIssueBytes,
  getDownloadManager,
  readerSnapshot,
  type ListDownloadScope,
  type OfflineListSource,
} from "@/lib/pwa/downloads";
import { DOWNLOAD_TIERS, type DownloadTier } from "@/lib/pwa/offline-store";
import { useDownload, useStorageUsage } from "@/lib/pwa/use-downloads";

export type OfflineDownloadTarget =
  | { kind: "issue"; issue: IssueDetailView; seriesName?: string | null }
  | {
      kind: "series";
      series: { id: string; slug: string; name: string };
      issueCount?: number | null;
    }
  | {
      kind: "list";
      list: OfflineListSource;
      /** Unfinished issues left in the list, for the size estimate. */
      remaining?: number | null;
    };

const TIER_KEY = "folio:download-tier";
const SCOPE_KEY = "folio:download-list-scope";

type ScopeKey = "next1" | "next5" | "next10" | "rest";
const SCOPES: {
  key: ScopeKey;
  label: string;
  hint: string;
  scope: ListDownloadScope;
}[] = [
  {
    key: "next1",
    label: "Next issue",
    hint: "The issue you are up to",
    scope: { count: 1 },
  },
  {
    key: "next5",
    label: "Next 5 issues",
    hint: "From where you are in the list",
    scope: { count: 5 },
  },
  {
    key: "next10",
    label: "Next 10 issues",
    hint: "From where you are in the list",
    scope: { count: 10 },
  },
  {
    key: "rest",
    label: "Rest of the list",
    hint: "Every issue you haven't finished yet",
    scope: "rest",
  },
];
const DEFAULT_SCOPE: ScopeKey = "next5";

function initialScope(): ScopeKey {
  try {
    const saved = localStorage.getItem(SCOPE_KEY);
    return SCOPES.some((s) => s.key === saved)
      ? (saved as ScopeKey)
      : DEFAULT_SCOPE;
  } catch {
    return DEFAULT_SCOPE;
  }
}

/** Issues a list download would take, for the estimate. */
export function listScopeCount(
  scope: ListDownloadScope,
  remaining: number | null | undefined,
): number {
  const left = remaining ?? null;
  if (scope === "rest") return Math.max(1, left ?? 10);
  return left == null ? scope.count : Math.max(1, Math.min(scope.count, left));
}
/** Page count assumed per issue when a series estimate has nothing better. */
const TYPICAL_ISSUE_PAGES = 24;

const TIER_LABEL: Record<string, { label: string; hint: string }> = {
  "720": { label: "Small", hint: "720 px wide — phones, least storage" },
  "1080": { label: "Medium", hint: "1080 px wide — most tablets" },
  "1600": {
    label: "Large",
    hint: "1600 px wide — large or high-density screens",
  },
  original: { label: "Original", hint: "Full resolution, best for zooming" },
};

function initialTier(): DownloadTier {
  try {
    const saved = localStorage.getItem(TIER_KEY);
    const match = DOWNLOAD_TIERS.find((t) => String(t) === saved);
    if (match) return match;
  } catch {
    /* storage disabled */
  }
  if (typeof window === "undefined") return 1080;
  return defaultDownloadTier(window.screen, window.devicePixelRatio || 1);
}

/**
 * "Download for offline" (WP-4.6): pick a page-size tier, see the estimated
 * size against the browser's storage quota, and queue the issue (or every
 * issue of a series). The download itself runs in the background download
 * manager; progress, pause and removal live in Settings → Downloads.
 */
export function OfflineDownloadDialog({
  open,
  onOpenChange,
  target,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  target: OfflineDownloadTarget;
}) {
  const me = useMe();
  const [tier, setTier] = useState<DownloadTier>(initialTier);
  const [scopeKey, setScopeKey] = useState<ScopeKey>(initialScope);
  const scope = SCOPES.find((s) => s.key === scopeKey)?.scope ?? { count: 5 };
  const [pending, setPending] = useState(false);
  const usage = useStorageUsage();
  const existing = useDownload(target.kind === "issue" ? target.issue.id : "");

  const estimateCount =
    target.kind === "issue"
      ? 1
      : target.kind === "series"
        ? Math.max(1, target.issueCount ?? 1)
        : listScopeCount(scope, target.remaining);
  const estimate =
    target.kind === "issue"
      ? estimateIssueBytes(
          (target.issue.pages as PageInfo[] | null | undefined) ?? [],
          tier,
          {
            pageCount: target.issue.page_count,
            fileSize: target.issue.file_size,
          },
        )
      : estimateIssueBytes([], tier, { pageCount: TYPICAL_ISSUE_PAGES }) *
        estimateCount;
  const free =
    usage?.quota != null && usage.usage != null
      ? Math.max(0, usage.quota - usage.usage)
      : null;
  const tight = free != null && estimate > free;

  const start = async () => {
    setPending(true);
    try {
      try {
        localStorage.setItem(TIER_KEY, String(tier));
      } catch {
        /* storage disabled */
      }
      const manager = getDownloadManager();
      const account = me.data?.id;
      if (account && manager.account() !== account)
        await manager.setAccount(account);
      const reader = readerSnapshot(me.data);
      if (target.kind === "issue") {
        await manager.downloadIssue(target.issue, tier, {
          reader,
          seriesName: target.seriesName ?? null,
        });
        toast.message("Downloading for offline reading", {
          id: "offline-download",
        });
      } else if (target.kind === "series") {
        const queued = await manager.downloadSeries(target.series, tier, {
          reader,
        });
        if (queued === 0)
          toast.info("Every issue in this series is already downloaded");
        else
          toast.message(
            `Downloading ${queued} ${queued === 1 ? "issue" : "issues"} for offline reading`,
            { id: "offline-download" },
          );
      } else {
        try {
          localStorage.setItem(SCOPE_KEY, scopeKey);
        } catch {
          /* storage disabled */
        }
        const { selected, queued } = await manager.downloadList(
          target.list,
          scope,
          tier,
          { reader },
        );
        if (selected === 0)
          toast.info("Nothing left to download: you've finished this list");
        else if (queued === 0)
          toast.info(
            `The next ${selected === 1 ? "issue" : `${selected} issues`} of ${target.list.name} ${selected === 1 ? "is" : "are"} already downloaded`,
          );
        else
          toast.message(
            `Downloading ${queued} ${queued === 1 ? "issue" : "issues"} from ${target.list.name} for offline reading`,
            { id: "offline-download" },
          );
      }
      onOpenChange(false);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : "Could not start the download",
      );
    } finally {
      setPending(false);
    }
  };

  const heading =
    target.kind === "issue"
      ? "Download issue for offline reading"
      : target.kind === "series"
        ? `Download ${target.series.name} for offline reading`
        : `Download ${target.list.name} for offline reading`;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{heading}</DialogTitle>
          <DialogDescription>
            Pages, thumbnails, and details are stored on this device so you can
            read without a connection. Progress you make offline syncs when you
            reconnect.
            {target.kind === "list"
              ? " Issues are taken from where you are in the list, in reading order, and kept together in Downloads."
              : null}
          </DialogDescription>
        </DialogHeader>

        {target.kind === "list" ? (
          <fieldset className="space-y-2">
            <legend className="mb-2 text-sm font-medium">How much</legend>
            <RadioGroup
              value={scopeKey}
              onValueChange={(value) => {
                if (SCOPES.some((s) => s.key === value))
                  setScopeKey(value as ScopeKey);
              }}
            >
              {SCOPES.map((s) => {
                const id = `download-scope-${s.key}`;
                return (
                  <div key={id} className="flex items-start gap-3">
                    <RadioGroupItem id={id} value={s.key} className="mt-1" />
                    <Label htmlFor={id} className="grid gap-0.5 font-normal">
                      <span className="font-medium">{s.label}</span>
                      <span className="text-muted-foreground text-xs">
                        {s.hint}
                      </span>
                    </Label>
                  </div>
                );
              })}
            </RadioGroup>
          </fieldset>
        ) : null}

        {existing?.status === "complete" ? (
          <p className="text-muted-foreground text-sm" role="status">
            Already downloaded ({formatBytes(existing.bytes)}). Choosing a
            different page size downloads it again.
          </p>
        ) : existing && existing.status !== "error" ? (
          <p className="text-muted-foreground text-sm" role="status">
            Download in progress: {existing.donePages} of{" "}
            {existing.pageCount || "?"} pages.
          </p>
        ) : null}

        <fieldset className="space-y-2">
          <legend className="mb-2 text-sm font-medium">Page size</legend>
          <RadioGroup
            value={String(tier)}
            onValueChange={(value) => {
              const next = DOWNLOAD_TIERS.find((t) => String(t) === value);
              if (next) setTier(next);
            }}
          >
            {DOWNLOAD_TIERS.map((t) => {
              const id = `download-tier-${t}`;
              const meta = TIER_LABEL[String(t)]!;
              return (
                <div key={id} className="flex items-start gap-3">
                  <RadioGroupItem id={id} value={String(t)} className="mt-1" />
                  <Label htmlFor={id} className="grid gap-0.5 font-normal">
                    <span className="font-medium">{meta.label}</span>
                    <span className="text-muted-foreground text-xs">
                      {meta.hint}
                    </span>
                  </Label>
                </div>
              );
            })}
          </RadioGroup>
        </fieldset>

        <div className="space-y-2 text-sm">
          <p>
            Estimated size:{" "}
            <span className="font-medium">
              {target.kind === "issue" ? "≈ " : "about "}
              {formatBytes(estimate)}
            </span>
            {target.kind === "series" && target.issueCount ? (
              <span className="text-muted-foreground">
                {" "}
                for {target.issueCount}{" "}
                {target.issueCount === 1 ? "issue" : "issues"}
              </span>
            ) : target.kind === "list" ? (
              <span className="text-muted-foreground">
                {" "}
                for{" "}
                {scope === "rest" && target.remaining == null ? "about " : ""}
                {estimateCount} {estimateCount === 1 ? "issue" : "issues"}
              </span>
            ) : null}
          </p>
          {usage?.quota != null && usage.usage != null ? (
            <div className="space-y-1">
              <Progress
                value={Math.min(100, (usage.usage / usage.quota) * 100)}
                aria-label="Storage used on this device"
              />
              <p className="text-muted-foreground text-xs">
                {formatBytes(usage.usage)} used of {formatBytes(usage.quota)}{" "}
                available to Folio on this device.
              </p>
            </div>
          ) : (
            <p className="text-muted-foreground text-xs">
              This browser does not report its storage quota.
            </p>
          )}
          {tight ? (
            <p className="text-destructive text-xs" role="alert">
              This may not fit. Remove downloads or choose a smaller page size.
            </p>
          ) : null}
        </div>

        <DialogFooter className="gap-2 sm:justify-between">
          <Button variant="ghost" asChild>
            <Link href="/settings/downloads">Manage downloads</Link>
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button onClick={() => void start()} disabled={pending || !me.data}>
              Download
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
