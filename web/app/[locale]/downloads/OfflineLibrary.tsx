"use client";

import { CloudOff, HardDriveDownload } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

import { OutboxReplayer } from "@/components/OutboxReplayer";
import {
  DownloadsList,
  offlineReadHref,
} from "@/components/offline/DownloadsList";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { useMe } from "@/lib/api/queries";
import { getDownloadManager, trackOutboxProgress } from "@/lib/pwa/downloads";
import {
  OFFLINE_SHELL_PATH,
  type DownloadRecord,
} from "@/lib/pwa/offline-store";
import { getOutbox } from "@/lib/pwa/outbox";
import { useDownloads } from "@/lib/pwa/use-downloads";

import { OfflineReader } from "./OfflineReader";

const READ_PATH = /^\/read\/([^/]+)\/([^/]+)\/?$/;

function subscribeOnline(cb: () => void) {
  window.addEventListener("online", cb);
  window.addEventListener("offline", cb);
  return () => {
    window.removeEventListener("online", cb);
    window.removeEventListener("offline", cb);
  };
}

/** The download a URL asks for: `?issue=<id>`, or `?from=/read/<s>/<i>`
 *  (the worker's redirect for an offline reader navigation). */
export function selectedDownload(
  downloads: readonly DownloadRecord[],
  params: { issue: string | null; from: string | null },
): { requested: boolean; record: DownloadRecord | undefined } {
  if (params.issue)
    return {
      requested: true,
      record: downloads.find((d) => d.issueId === params.issue),
    };
  const match = params.from ? READ_PATH.exec(params.from) : null;
  if (!match) return { requested: false, record: undefined };
  const [series, issue] = [
    decodeURIComponent(match[1]!),
    decodeURIComponent(match[2]!),
  ];
  return {
    requested: true,
    record: downloads.find(
      (d) => d.seriesSlug === series && d.issueSlug === issue,
    ),
  };
}

/**
 * Offline library + reader shell (WP-4.6, offline plan step 2). This page
 * renders entirely on the client from IndexedDB, so the service worker can
 * serve a stored, credential-less copy of it with no network: it lists the
 * device's downloads and opens a complete one in the regular reader. It
 * reads only the downloads of the account that owns this device's offline
 * data; with no session (offline, expired) it still replays queued
 * progress for that account once the server is reachable again.
 */
export function OfflineLibrary() {
  const params = useSearchParams();
  const manager = getDownloadManager();
  const me = useMe({ enabled: false });
  const signedIn = !!me.data;
  const [ready, setReady] = useState(false);
  const [owner, setOwner] = useState<string | null>(null);
  const [intact, setIntact] = useState<Record<string, boolean>>({});
  const downloads = useDownloads(manager);
  const online = useSyncExternalStore(
    subscribeOnline,
    () => navigator.onLine,
    () => true,
  );

  useEffect(() => {
    let live = true;
    void manager.hydrate().then(() => {
      if (!live) return;
      setOwner(manager.account());
      setReady(true);
    });
    return () => {
      live = false;
    };
  }, [manager]);

  // Signed-in pages track progress from `OfflineBootstrap`; the public
  // shell does it itself, scoped to the device owner.
  useEffect(() => {
    if (signedIn || !owner) return;
    const outbox = getOutbox();
    outbox.setAccount(owner);
    return trackOutboxProgress(manager, outbox);
  }, [manager, owner, signedIn]);

  const { requested, record } = selectedDownload(downloads, {
    issue: params.get("issue"),
    from: params.get("from"),
  });
  const readable = record?.status === "complete";

  // Integrity check before advertising a download as readable offline.
  const recordId = readable ? record.issueId : null;
  useEffect(() => {
    if (!recordId) return;
    let live = true;
    void manager.verify(recordId).then((ok) => {
      if (live) setIntact((m) => ({ ...m, [recordId]: ok }));
    });
    return () => {
      live = false;
    };
  }, [manager, recordId]);

  const open = useCallback((d: DownloadRecord) => {
    window.history.pushState(null, "", offlineReadHref(d.issueId));
  }, []);
  const back = useCallback(() => {
    window.history.pushState(null, "", OFFLINE_SHELL_PATH);
  }, []);

  const replayer =
    !signedIn && owner ? <OutboxReplayer userId={owner} /> : null;

  if (readable && intact[record.issueId]) {
    return (
      <>
        {replayer}
        <OfflineReader key={record.issueId} record={record} manager={manager} />
      </>
    );
  }

  return (
    <main className="mx-auto w-full max-w-3xl space-y-6 px-4 py-8 pb-[max(2rem,var(--safe-bottom))]">
      {replayer}
      <header className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div>
          <h1 className="text-foreground text-2xl font-semibold tracking-tight">
            Downloads
          </h1>
          <p className="text-muted-foreground mt-1 text-sm">
            Issues stored on this device. They open without a connection;
            progress syncs when you are back online.
          </p>
        </div>
        {online ? (
          <Button variant="outline" asChild>
            <a href="/">Open library</a>
          </Button>
        ) : null}
      </header>

      {!online ? (
        <div
          role="status"
          className="border-border bg-card flex items-start gap-3 rounded-lg border p-4 text-sm"
        >
          <CloudOff className="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <p>
            You&rsquo;re offline. Downloaded issues are available below; the
            rest of your library returns when you reconnect.
          </p>
        </div>
      ) : null}

      {requested && ready && !(readable && intact[record.issueId] !== false) ? (
        <EmptyState
          size="sm"
          icon={HardDriveDownload}
          title="This issue isn’t available offline"
          description={
            record && record.status !== "complete"
              ? "Its download hasn’t finished. Resume it below while online."
              : readable
                ? "Some of its pages are missing from this device. Download it again while online."
                : "Download it for offline reading from its page while you are online."
          }
          action={
            <Button variant="outline" onClick={back}>
              Show downloads
            </Button>
          }
        />
      ) : null}

      {ready && !owner ? (
        <EmptyState
          size="sm"
          icon={HardDriveDownload}
          title="No downloads on this device"
          description="Sign in and use “Download for offline” on an issue or series to read it here without a connection."
        />
      ) : (
        <DownloadsList manager={manager} onRead={open} />
      )}
    </main>
  );
}
