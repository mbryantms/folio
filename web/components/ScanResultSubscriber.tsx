"use client";

import { useScanEvents } from "@/lib/api/scan-events";

/** Admin-only scan-events WebSocket, split out of the root layout's
 *  first-load JS (see `ScanResultListener`). */
export function ScanResultSubscriber() {
  // Side-effects only; we don't render anything from this hook.
  useScanEvents({ toastErrors: true, toastCompletions: true });
  return null;
}
