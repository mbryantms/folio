"use client";

import dynamic from "next/dynamic";

import { useMe } from "@/lib/api/queries";

// Code-split: the scan-events client is admin-only, but this listener sits
// in the root layout, so a static import put it in every page's first-load
// JS, the reader's included (`scripts/check-bundle-size.mjs`). `ssr: false`
// keeps it out of hydration; it loads once `me` says admin.
const Subscriber = dynamic(
  () => import("./ScanResultSubscriber").then((m) => m.ScanResultSubscriber),
  { ssr: false },
);

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
  // `useMe` skips the sign-in pages itself, so `data` is unset there.
  const me = useMe();
  return me.data?.role === "admin" ? <Subscriber /> : null;
}
