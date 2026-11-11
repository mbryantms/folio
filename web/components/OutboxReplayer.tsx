"use client";
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  invalidateRails,
  invalidateSeriesProgress,
} from "@/lib/api/mutations/rails";
import { queryKeys } from "@/lib/api/queries";
import { TOAST } from "@/lib/api/toast-strings";
import { getOutbox, startOutboxReplay } from "@/lib/pwa/outbox";
import {
  PROGRESS_OUTBOX_KIND,
  progressOutboxKind,
} from "@/lib/reader/progress-writer";
import { SESSION_OUTBOX_KIND, sessionOutboxKind } from "@/lib/reader/session";
import { postProgress } from "@/lib/reader/use-progress-write";

/**
 * App-wide replay of the durable write outbox (WP-4.5). Mounted once in
 * `QueryProvider` for a signed-in account: registers the progress and
 * reading-session kinds, scopes the outbox to the account, and replays on
 * launch, `online`, tab-visible, the server becoming reachable, a retry
 * backoff armed by each new entry, and the service worker's Background
 * Sync relay. Replays go through `apiFetch` in the page so the CSRF header
 * and token refresh behave exactly like a live write. When a queue that
 * could not be delivered drains, one success toast says so (sharing the
 * connectivity toast's id, so it replaces a lingering offline notice).
 */
export function OutboxReplayer({ userId }: { userId?: string }) {
  const client = useQueryClient();
  useEffect(() => {
    if (!userId) return;
    const outbox = getOutbox();
    outbox.setAccount(userId);
    const offProgress = outbox.register(
      PROGRESS_OUTBOX_KIND,
      progressOutboxKind(postProgress, () => {
        void client.invalidateQueries({ queryKey: queryKeys.userProgress });
        invalidateRails(client);
        invalidateSeriesProgress(client);
      }),
    );
    const offSession = outbox.register(
      SESSION_OUTBOX_KIND,
      sessionOutboxKind(),
    );
    const stop = startOutboxReplay(outbox, {
      onDrained: () =>
        toast.success(TOAST.PROGRESS_SYNCED, {
          id: "network-status",
          duration: 3000,
        }),
    });
    return () => {
      stop();
      offProgress();
      offSession();
    };
  }, [client, userId]);
  return null;
}
