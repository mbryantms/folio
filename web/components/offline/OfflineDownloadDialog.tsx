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
} from "@/lib/pwa/downloads";
import { DOWNLOAD_TIERS, type DownloadTier } from "@/lib/pwa/offline-store";
import { useDownload, useStorageUsage } from "@/lib/pwa/use-downloads";

export type OfflineDownloadTarget =
  | { kind: "issue"; issue: IssueDetailView; seriesName?: string | null }
  | {
      kind: "series";
      series: { id: string; slug: string; name: string };
      issueCount?: number | null;
    };

const TIER_KEY = "folio:download-tier";
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
  const [pending, setPending] = useState(false);
  const usage = useStorageUsage();
  const existing = useDownload(target.kind === "issue" ? target.issue.id : "");

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
        Math.max(1, target.issueCount ?? 1);
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
      } else {
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
      : `Download ${target.series.name} for offline reading`;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{heading}</DialogTitle>
          <DialogDescription>
            Pages, thumbnails, and details are stored on this device so you can
            read without a connection. Progress you make offline syncs when you
            reconnect.
          </DialogDescription>
        </DialogHeader>

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
              {target.kind === "series" ? "about " : "≈ "}
              {formatBytes(estimate)}
            </span>
            {target.kind === "series" && target.issueCount ? (
              <span className="text-muted-foreground">
                {" "}
                for {target.issueCount}{" "}
                {target.issueCount === 1 ? "issue" : "issues"}
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
