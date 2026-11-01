import type { QueryClient } from "@tanstack/react-query";

import { getOutbox } from "./outbox";
import { broadcastPrivateReset, clearPrivateState } from "./private-state";

/**
 * Drop everything this device holds for the account that just signed out
 * (or deleted itself), then leave with a hard navigation, which also drops
 * private state outside the query cache. Shared by sign-out
 * (`UserFooter`) and account deletion (`DeleteAccountCard`) so the two
 * can't drift as private stores are added.
 *
 * The query cache goes first: once the server has cleared the cookies, a
 * refetch during the slower IndexedDB cleanup would 401, and
 * `AccountCacheBoundary` answers that with a reload that can beat the
 * navigation to `href`.
 */
export async function forgetAccountOnDevice(
  client: QueryClient,
  href: string,
): Promise<void> {
  broadcastPrivateReset();
  await clearPrivateState(client).catch(() => undefined);
  await getOutbox()
    .clear()
    .catch(() => undefined);
  // Offline downloads belong to the account: an explicit sign-out removes
  // them from the device (session expiry keeps them for offline reads).
  await import("./downloads")
    .then(({ getDownloadManager }) => getDownloadManager().clearAll())
    .catch(() => undefined);
  window.location.assign(href);
}
