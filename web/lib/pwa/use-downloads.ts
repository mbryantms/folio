"use client";

import { useEffect, useState, useSyncExternalStore } from "react";

import {
  getDownloadManager,
  type DownloadManager,
  type StorageUsage,
} from "./downloads";
import type { DownloadRecord } from "./offline-store";

const EMPTY: readonly DownloadRecord[] = [];

/** Live list of this device's downloads (current account only). */
export function useDownloads(
  manager: DownloadManager = getDownloadManager(),
): readonly DownloadRecord[] {
  return useSyncExternalStore(manager.subscribe, manager.snapshot, () => EMPTY);
}

/** One issue's download record, if any. */
export function useDownload(
  issueId: string,
  manager: DownloadManager = getDownloadManager(),
): DownloadRecord | undefined {
  return useDownloads(manager).find((r) => r.issueId === issueId);
}

/** `navigator.storage` usage/quota; refreshed whenever downloads change. */
export function useStorageUsage(
  manager: DownloadManager = getDownloadManager(),
): StorageUsage | null {
  const downloads = useDownloads(manager);
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  // Byte totals move on every stored page; re-estimate on count/status
  // changes and completed bytes rather than per page.
  const signature = downloads
    .map(
      (d) =>
        `${d.issueId}:${d.status}:${d.status === "complete" ? d.bytes : ""}`,
    )
    .join("|");
  useEffect(() => {
    let live = true;
    void manager.usage().then((next) => {
      if (live) setUsage(next);
    });
    return () => {
      live = false;
    };
  }, [manager, signature]);
  return usage;
}
