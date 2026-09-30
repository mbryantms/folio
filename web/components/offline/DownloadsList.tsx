"use client";

import { BookOpen, HardDriveDownload, Pause, Play, Trash2 } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";

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
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { Progress } from "@/components/ui/progress";
import { formatBytes, formatIssueHeading } from "@/lib/format";
import { getDownloadManager, type DownloadManager } from "@/lib/pwa/downloads";
import {
  offlineCoverKey,
  OFFLINE_SHELL_PATH,
  type DownloadRecord,
} from "@/lib/pwa/offline-store";
import { useDownloads, useStorageUsage } from "@/lib/pwa/use-downloads";

export function downloadLabel(record: DownloadRecord): string {
  return formatIssueHeading(
    { title: record.title, number: record.number },
    record.seriesName,
  );
}

/** Where a downloaded issue opens: the offline reader shell. */
export function offlineReadHref(issueId: string): string {
  return `${OFFLINE_SHELL_PATH}?issue=${encodeURIComponent(issueId)}`;
}

function statusText(record: DownloadRecord): string {
  const total = record.pageCount || record.pages.length;
  switch (record.status) {
    case "complete":
      return record.missingThumbs > 0
        ? `Downloaded · ${formatBytes(record.bytes)} · ${record.missingThumbs} page previews unavailable`
        : `Downloaded · ${formatBytes(record.bytes)}`;
    case "downloading":
      return `Downloading · ${record.donePages} of ${total || "?"} pages · ${formatBytes(record.bytes)}`;
    case "queued":
      return "Waiting to download";
    case "paused":
      return `Paused · ${record.donePages} of ${total || "?"} pages`;
    case "error":
      return record.errorMessage ?? "Download failed";
  }
}

/**
 * Downloads on this device with sizes, progress, pause/resume, and removal
 * (WP-4.6 eviction UI). Rendered in Settings → Downloads and in the offline
 * library (`/downloads`), where `onRead` opens the offline reader in place.
 */
export function DownloadsList({
  manager = getDownloadManager(),
  onRead,
}: {
  manager?: DownloadManager;
  onRead?: (record: DownloadRecord) => void;
}) {
  const downloads = useDownloads(manager);
  const usage = useStorageUsage(manager);
  const [confirm, setConfirm] = useState<DownloadRecord | "all" | null>(null);
  const total = downloads.reduce((sum, d) => sum + d.bytes, 0);

  const remove = async () => {
    const target = confirm;
    setConfirm(null);
    try {
      if (target === "all") {
        await manager.removeAll();
        toast.success("Removed all downloads from this device");
      } else if (target) {
        await manager.remove(target.issueId);
        toast.success(`Removed ${downloadLabel(target)} from this device`);
      }
    } catch {
      toast.error("Could not remove the download. Try again.");
    }
  };

  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <div className="flex flex-wrap items-baseline justify-between gap-2 text-sm">
          <p>
            <span className="font-medium">{formatBytes(total)}</span>{" "}
            <span className="text-muted-foreground">
              in {downloads.length}{" "}
              {downloads.length === 1 ? "download" : "downloads"}
            </span>
          </p>
          {downloads.length > 0 ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => setConfirm("all")}
            >
              <Trash2 aria-hidden="true" />
              Remove all
            </Button>
          ) : null}
        </div>
        {usage?.quota != null && usage.usage != null ? (
          <>
            <Progress
              value={Math.min(100, (usage.usage / usage.quota) * 100)}
              aria-label="Storage used on this device"
            />
            <p className="text-muted-foreground text-xs">
              {formatBytes(usage.usage)} used of {formatBytes(usage.quota)}{" "}
              available to Folio on this device.{" "}
              {usage.persisted
                ? "Storage is persistent: the browser keeps downloads until you remove them."
                : "The browser may clear downloads when the device runs low on space."}
            </p>
          </>
        ) : null}
      </div>

      {downloads.length === 0 ? (
        <EmptyState
          size="sm"
          icon={HardDriveDownload}
          title="No downloads"
          description="Use “Download for offline” in an issue’s or series’ actions menu to read it without a connection."
        />
      ) : (
        <ul role="list" className="divide-border divide-y">
          {downloads.map((d) => {
            const label = downloadLabel(d);
            const total = d.pageCount || d.pages.length;
            const inFlight =
              d.status === "downloading" || d.status === "queued";
            return (
              <li key={d.key} className="flex items-center gap-3 py-3">
                {/* eslint-disable-next-line @next/next/no-img-element -- stored cover, served offline by the worker */}
                <img
                  src={offlineCoverKey(d.issueId)}
                  alt=""
                  loading="lazy"
                  className="bg-muted h-16 w-11 shrink-0 rounded object-cover"
                />
                <div className="min-w-0 flex-1 space-y-1">
                  <p className="truncate text-sm font-medium">{label}</p>
                  <p
                    className={
                      d.status === "error"
                        ? "text-destructive text-xs"
                        : "text-muted-foreground text-xs"
                    }
                  >
                    {statusText(d)}
                  </p>
                  {d.status !== "complete" && total > 0 ? (
                    <Progress
                      value={(d.donePages / total) * 100}
                      aria-label={`${label} download progress`}
                      className="h-1.5"
                    />
                  ) : null}
                </div>
                <div className="flex shrink-0 gap-1">
                  {d.status === "complete" ? (
                    onRead ? (
                      <Button size="sm" onClick={() => onRead(d)}>
                        <BookOpen aria-hidden="true" />
                        Read
                      </Button>
                    ) : (
                      <Button size="sm" asChild>
                        <Link href={offlineReadHref(d.issueId)}>
                          <BookOpen aria-hidden="true" />
                          Read
                        </Link>
                      </Button>
                    )
                  ) : inFlight ? (
                    <Button
                      size="icon"
                      variant="ghost"
                      aria-label={`Pause ${label}`}
                      onClick={() => void manager.pause(d.issueId)}
                    >
                      <Pause aria-hidden="true" />
                    </Button>
                  ) : (
                    <Button
                      size="icon"
                      variant="ghost"
                      aria-label={
                        d.status === "error"
                          ? `Retry ${label}`
                          : `Resume ${label}`
                      }
                      onClick={() => void manager.resume(d.issueId)}
                    >
                      <Play aria-hidden="true" />
                    </Button>
                  )}
                  <Button
                    size="icon"
                    variant="ghost"
                    aria-label={`Remove ${label}`}
                    onClick={() => setConfirm(d)}
                  >
                    <Trash2 aria-hidden="true" />
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <AlertDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirm === "all"
                ? "Remove all downloads?"
                : "Remove this download?"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm === "all"
                ? "Every downloaded issue is deleted from this device. Your reading progress is kept on the server."
                : confirm
                  ? `${downloadLabel(confirm)} is deleted from this device. Your reading progress is kept on the server.`
                  : null}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => void remove()}>
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
