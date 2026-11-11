"use client";

import { useEffect, useState } from "react";
import { Check, CloudOff, RefreshCw } from "lucide-react";

import { getOutbox, type Outbox } from "@/lib/pwa/outbox";
import {
  PROGRESS_OUTBOX_KIND,
  type ProgressBody,
} from "@/lib/reader/progress-writer";

export type ProgressSyncState = "saved" | "queued" | "retrying";

/** Where this issue's reading progress stands, from the durable outbox:
 *  nothing queued → saved; queued and the replayer has already failed to
 *  deliver it while online → retrying; otherwise saved on this device and
 *  waiting to sync. */
export async function progressSyncState(
  outbox: Outbox,
  issueId: string,
  online: boolean,
): Promise<ProgressSyncState> {
  const entries = await outbox.entries<ProgressBody>({
    kind: PROGRESS_OUTBOX_KIND,
  });
  const mine = entries.filter((e) => e.payload.issue_id === issueId);
  if (mine.length === 0) return "saved";
  if (online && mine.some((e) => e.attempts > 0)) return "retrying";
  return "queued";
}

const COPY: Record<ProgressSyncState, string> = {
  saved: "Progress saved",
  queued: "Progress saved on this device, will sync",
  retrying:
    "Can’t reach the server, retrying. Progress is saved on this device.",
};

/**
 * One status line for the reader settings popover. Lives in the
 * popover's lazy chunk (`ReaderSettings` via `lazy.tsx`), so it adds
 * nothing to the reader's first load. Re-reads the outbox on each queue
 * change and on `online` / `offline`; only while the popover is open.
 */
export function ProgressSyncStatus({ issueId }: { issueId: string }) {
  const [state, setState] = useState<ProgressSyncState>("saved");
  useEffect(() => {
    const outbox = getOutbox();
    let live = true;
    const refresh = () => {
      void progressSyncState(outbox, issueId, navigator.onLine)
        .then((next) => {
          if (live) setState(next);
        })
        .catch(() => undefined);
    };
    refresh();
    const off = outbox.subscribe(refresh);
    window.addEventListener("online", refresh);
    window.addEventListener("offline", refresh);
    return () => {
      live = false;
      off();
      window.removeEventListener("online", refresh);
      window.removeEventListener("offline", refresh);
    };
  }, [issueId]);
  const Icon =
    state === "saved" ? Check : state === "retrying" ? RefreshCw : CloudOff;
  return (
    <p
      role="status"
      className="flex items-center gap-2 text-xs text-neutral-400"
      data-sync-state={state}
    >
      <Icon className="size-3.5 shrink-0" aria-hidden="true" />
      <span>{COPY[state]}</span>
    </p>
  );
}
