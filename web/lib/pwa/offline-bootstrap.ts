import { toast } from "sonner";

import { formatIssueHeading } from "@/lib/format";

import { getDownloadManager, trackOutboxProgress } from "./downloads";
import { getOutbox } from "./outbox";

/**
 * Signed-in start-up for offline downloads (WP-4.6), loaded lazily by
 * `OfflineBootstrap`: scope the download manager to the account (purging
 * any other account's downloads on this device), resume interrupted
 * downloads, keep downloaded issues' resume positions in step with the
 * progress outbox, and toast completions / failures. Returns a cleanup.
 */
export function startOfflineDownloads(userId: string): () => void {
  const manager = getDownloadManager();
  let stopped = false;
  let offProgress: (() => void) | undefined;
  const offEvents = manager.onEvent((event) => {
    const label = formatIssueHeading(
      { title: event.record.title, number: event.record.number },
      event.record.seriesName,
    );
    if (event.type === "complete")
      toast.success(`${label} is ready to read offline`, {
        id: "offline-download",
      });
    else
      toast.error(
        `${label}: ${event.record.errorMessage ?? "download failed"}`,
        { id: "offline-download-error" },
      );
  });
  void manager
    .setAccount(userId)
    .then(() => {
      if (!stopped) offProgress = trackOutboxProgress(manager, getOutbox());
    })
    .catch(() => {
      /* Offline storage unavailable: downloads stay off. */
    });
  return () => {
    stopped = true;
    offEvents();
    offProgress?.();
  };
}
