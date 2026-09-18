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

export function broadcastPrivateReset() {
  if (!("BroadcastChannel" in window)) return;
  const channel = new BroadcastChannel(ACCOUNT_CHANNEL);
  channel.postMessage("reset");
  channel.close();
}
