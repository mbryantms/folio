"use client";

import { useScanEvents } from "@/lib/api/scan-events";
import { useMe } from "@/lib/api/queries";

/**
 * Headless WebSocket subscriber that runs on every page (mounted from the
 * locale layout) so admins get a "Scan complete" toast no matter where they
 * triggered the scan from — admin shell, the home page, a series page, an
 * issue page, anywhere. Non-admins skip the subscription entirely; the
 * scan-events backend is admin-only and a non-admin attempt would just back
 * off and retry, wasting handshakes.
 *
 * Toasts are deduped by `scan_id` inside `useScanEvents`, so coexisting
 * with the admin shell's `ScanEventBeacon` (which subscribes for its own
 * status pill) does not produce duplicate notifications.
 */
export function ScanResultListener() {
  const me = useMe();
  const enabled = me.data?.role === "admin";
  return enabled ? <Subscriber /> : null;
}

function Subscriber() {
  // Side-effects only; we don't render anything from this hook.
  useScanEvents({ toastErrors: true, toastCompletions: true });
  return null;
}
