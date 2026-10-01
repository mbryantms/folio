"use client";

import { useEffect } from "react";

/**
 * Signed-in bootstrap for offline downloads (WP-4.6), mounted once in
 * `QueryProvider` next to the outbox replayer. Everything it does lives in
 * `lib/pwa/offline-bootstrap.ts`, imported lazily so this effect is all
 * the first-load bundle carries.
 */
export function OfflineBootstrap({ userId }: { userId?: string }) {
  useEffect(() => {
    if (!userId) return;
    let live = true;
    let stop: (() => void) | undefined;
    void import("@/lib/pwa/offline-bootstrap")
      .then(({ startOfflineDownloads }) => {
        if (live) stop = startOfflineDownloads(userId);
      })
      .catch(() => {
        /* Chunk unavailable (offline): downloads start on the next load. */
      });
    return () => {
      live = false;
      stop?.();
    };
  }, [userId]);
  return null;
}
