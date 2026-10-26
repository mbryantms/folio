"use client";

import {
  BookOpen,
  HardDriveDownload,
  ListOrdered,
  Pause,
  Play,
  Trash2,
} from "lucide-react";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/folio/empty-state";
import { Progress } from "@/components/ui/progress";
import { formatBytes, formatIssueHeading } from "@/lib/format";
import { getDownloadManager, type DownloadManager } from "@/lib/pwa/downloads";
import {
  nextInOfflineList,
  offlineListRows,
  offlineListSummary,
} from "@/lib/pwa/offline-lists";
import {
  offlineCoverKey,
  OFFLINE_SHELL_PATH,
  type DownloadRecord,
  type OfflineList,
} from "@/lib/pwa/offline-store";
import {
  useDownloads,
  useOfflineLists,
  useStorageUsage,
} from "@/lib/pwa/use-downloads";

export function downloadLabel(record: DownloadRecord): string {
  return formatIssueHeading(
    { title: record.title, number: record.number },
    record.seriesName,
  );
}

/** Where a downloaded issue opens: the offline reader shell. With a
 *  list, the reader follows that list's order for Up Next. */
export function offlineReadHref(
  issueId: string,
  listId?: string | null,
): string {
  const qs = new URLSearchParams({ issue: issueId });
  if (listId) qs.set("list", listId);
  return `${OFFLINE_SHELL_PATH}?${qs}`;
}

export const listKindLabel = (list: Pick<OfflineList, "kind">) =>
  list.kind === "cbl" ? "Reading list" : "Collection";

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

type ReadHandler = (record: DownloadRecord, listId?: string | null) => void;

function DownloadRow({
  record: d,
  manager,
  onRead,
  onRemove,
  listId,
  position,
}: {
  record: DownloadRecord;
  manager: DownloadManager;
  onRead?: ReadHandler;
  onRemove: (record: DownloadRecord) => void;
  /** Rendered inside a list section: Read keeps the list context. */
  listId?: string | null;
  /** 1-based position in that list. */
  position?: number;
}) {
  const label = downloadLabel(d);
  const total = d.pageCount || d.pages.length;
  const inFlight = d.status === "downloading" || d.status === "queued";
  const finished = d.progress?.finished === true;
  return (
    <li className="flex items-center gap-3 py-3">
      {position != null ? (
        <span
          className="text-muted-foreground w-7 shrink-0 text-right text-xs tabular-nums"
          aria-label={`Position ${position}`}
        >
          {position}
        </span>
      ) : null}
      {/* eslint-disable-next-line @next/next/no-img-element -- stored cover, served offline by the worker */}
      <img
        src={offlineCoverKey(d.issueId)}
        alt=""
        loading="lazy"
        className="bg-muted h-16 w-11 shrink-0 rounded object-cover"
      />
      <div className="min-w-0 flex-1 space-y-1">
        <p className="truncate text-sm font-medium">
          {label}
          {finished ? (
            <span className="text-muted-foreground ml-2 text-xs font-normal">
              Read
            </span>
          ) : null}
        </p>
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
            <Button size="sm" onClick={() => onRead(d, listId)}>
              <BookOpen aria-hidden="true" />
              Read
            </Button>
          ) : (
            <Button size="sm" asChild>
              <Link href={offlineReadHref(d.issueId, listId)}>
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
              d.status === "error" ? `Retry ${label}` : `Resume ${label}`
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
          onClick={() => onRemove(d)}
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </div>
    </li>
  );
}

/** One offline reading list: its issues in reading order, kept apart
 *  from every other download, with "Continue" on the next unfinished
 *  downloaded issue and "Remove list" for the whole set. */
function OfflineListSection({
  list,
  downloads,
  manager,
  onRead,
  onRemove,
  onRemoveList,
}: {
  list: OfflineList;
  downloads: readonly DownloadRecord[];
  manager: DownloadManager;
  onRead?: ReadHandler;
  onRemove: (record: DownloadRecord) => void;
  onRemoveList: (list: OfflineList) => void;
}) {
  const rows = offlineListRows(list, downloads);
  const summary = offlineListSummary(list, downloads);
  const next = nextInOfflineList(list, downloads);
  const continueLabel = summary.finished > 0 ? "Continue" : "Start reading";
  const nextLabel = next ? downloadLabel(next.record) : null;
  return (
    <section
      aria-labelledby={`offline-list-${list.id}`}
      className="border-border bg-card rounded-lg border"
    >
      <header className="border-border flex flex-wrap items-start justify-between gap-3 border-b p-3">
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <ListOrdered
              aria-hidden="true"
              className="text-muted-foreground size-4 shrink-0"
            />
            <h3
              id={`offline-list-${list.id}`}
              className="truncate text-sm font-semibold"
            >
              {list.name}
            </h3>
            <Badge variant="secondary">{listKindLabel(list)}</Badge>
          </div>
          <p className="text-muted-foreground text-xs">
            {summary.downloaded} of {summary.total}{" "}
            {summary.total === 1 ? "issue" : "issues"} downloaded
            {summary.finished > 0 ? ` · ${summary.finished} read` : ""}
            {" · "}
            {formatBytes(summary.bytes)}
            {list.totalInList != null && list.totalInList > summary.total
              ? ` · ${list.totalInList} in the full list`
              : ""}
          </p>
        </div>
        <div className="flex shrink-0 gap-1">
          {next ? (
            onRead ? (
              <Button
                size="sm"
                onClick={() => onRead(next.record, list.id)}
                aria-label={`${continueLabel} ${list.name}: ${nextLabel}`}
              >
                <Play aria-hidden="true" />
                {continueLabel}
              </Button>
            ) : (
              <Button size="sm" asChild>
                <Link
                  href={offlineReadHref(next.issueId, list.id)}
                  aria-label={`${continueLabel} ${list.name}: ${nextLabel}`}
                >
                  <Play aria-hidden="true" />
                  {continueLabel}
                </Link>
              </Button>
            )
          ) : null}
          <Button
            size="sm"
            variant="ghost"
            aria-label={`Remove list ${list.name}`}
            onClick={() => onRemoveList(list)}
          >
            <Trash2 aria-hidden="true" />
            Remove list
          </Button>
        </div>
      </header>
      <ul role="list" className="divide-border divide-y px-3">
        {rows.map((row) =>
          row.record ? (
            <DownloadRow
              key={row.issueId}
              record={row.record}
              manager={manager}
              onRead={onRead}
              onRemove={onRemove}
              listId={list.id}
              position={row.position + 1}
            />
          ) : null,
        )}
      </ul>
    </section>
  );
}

type Confirm = DownloadRecord | OfflineList | "all";

const isList = (c: Confirm): c is OfflineList =>
  typeof c === "object" && "entries" in c;

/**
 * Downloads on this device with sizes, progress, pause/resume, and removal
 * (WP-4.6 eviction UI). Offline reading lists come first, each in its
 * reading order; everything downloaded on its own follows. Rendered in
 * Settings → Downloads and in the offline library (`/downloads`), where
 * `onRead` opens the offline reader in place.
 */
export function DownloadsList({
  manager = getDownloadManager(),
  onRead,
}: {
  manager?: DownloadManager;
  onRead?: ReadHandler;
}) {
  const downloads = useDownloads(manager);
  const lists = useOfflineLists(manager);
  const usage = useStorageUsage(manager);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const total = downloads.reduce((sum, d) => sum + d.bytes, 0);
  const listed = new Set(lists.flatMap((l) => l.entries.map((e) => e.issueId)));
  const others = downloads.filter((d) => !listed.has(d.issueId));

  const remove = async () => {
    const target = confirm;
    setConfirm(null);
    try {
      if (target === "all") {
        await manager.removeAll();
        toast.success("Removed all downloads from this device");
      } else if (target && isList(target)) {
        await manager.removeList(target.id);
        toast.success(`Removed ${target.name} from this device`);
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
              {lists.length > 0
                ? ` · ${lists.length} ${lists.length === 1 ? "list" : "lists"}`
                : ""}
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
          description="Use “Download for offline” in an issue’s, series’, reading list’s or collection’s actions menu to read it without a connection."
        />
      ) : (
        <>
          {lists.map((list) => (
            <OfflineListSection
              key={list.id}
              list={list}
              downloads={downloads}
              manager={manager}
              onRead={onRead}
              onRemove={setConfirm}
              onRemoveList={setConfirm}
            />
          ))}
          {others.length > 0 ? (
            <section aria-labelledby="offline-others">
              {lists.length > 0 ? (
                <h3
                  id="offline-others"
                  className="text-muted-foreground mb-1 text-xs font-semibold tracking-wide uppercase"
                >
                  Other downloads
                </h3>
              ) : null}
              <ul role="list" className="divide-border divide-y">
                {others.map((d) => (
                  <DownloadRow
                    key={d.key}
                    record={d}
                    manager={manager}
                    onRead={onRead}
                    onRemove={setConfirm}
                  />
                ))}
              </ul>
            </section>
          ) : null}
        </>
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
                : confirm && isList(confirm)
                  ? `Remove ${confirm.name} from this device?`
                  : "Remove this download?"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm === "all"
                ? "Every downloaded issue is deleted from this device. Your reading progress is kept on the server."
                : confirm && isList(confirm)
                  ? "Its issues are deleted from this device unless you also downloaded them on their own or through another list. Your reading progress is kept on the server."
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
