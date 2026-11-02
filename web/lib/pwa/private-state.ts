import type { QueryClient } from "@tanstack/react-query";
import { THUMB_CACHE } from "./cache-policy";

export const PRIVATE_RESET = "folio:private-reset";
export const ACCOUNT_CHANNEL = "folio-account";

export async function clearPrivateState(client: QueryClient): Promise<void> {
  window.dispatchEvent(new Event(PRIVATE_RESET));
  await client.cancelQueries();
  client.clear();
  const controller = navigator.serviceWorker?.controller;
  if (controller) {
    await new Promise<void>((resolve) => {
      const channel = new MessageChannel();
      const done = () => {
        clearTimeout(timeout);
        channel.port1.close();
        resolve();
      };
      const timeout = setTimeout(done, 2000);
      channel.port1.onmessage = done;
      controller.postMessage({ type: "FOLIO_CLEAR_PRIVATE" }, [channel.port2]);
    });
  }
  if ("caches" in window) await caches.delete(THUMB_CACHE).catch(() => false);
}

let leaving = false;

/** This tab is signing out (or deleting the account) and navigates away
 *  itself. A BroadcastChannel message reaches every *other* channel object
 *  of the same name, including `AccountCacheBoundary`'s in this same tab,
 *  so without this flag the tab hears its own reset and reloads, racing
 *  the navigation to the sign-in page (or the IdP's logout page). */
export function beginLeaving() {
  leaving = true;
}

export function isLeaving() {
  return leaving;
}

export function broadcastPrivateReset() {
  if (!("BroadcastChannel" in window)) return;
  const channel = new BroadcastChannel(ACCOUNT_CHANNEL);
  channel.postMessage("reset");
  channel.close();
}
