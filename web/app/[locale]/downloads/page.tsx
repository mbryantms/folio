import type { Metadata } from "next";
import { Suspense } from "react";

import { OfflineLibrary } from "./OfflineLibrary";

export const metadata: Metadata = { title: "Downloads · Folio" };

/**
 * `/downloads` — the offline library and reader shell (WP-4.6). Public by
 * design: it reads no server data during render, so the service worker can
 * keep a credential-less copy and serve it with no network. Everything it
 * shows comes from this device's IndexedDB / Cache Storage, scoped to the
 * account that downloaded it.
 */
export default function DownloadsPage() {
  return (
    <Suspense>
      <OfflineLibrary />
    </Suspense>
  );
}
