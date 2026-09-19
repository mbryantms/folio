"use client";
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { queryKeys } from "@/lib/api/queries";
import type { MeView } from "@/lib/api/types";
import {
  ACCOUNT_CHANNEL,
  broadcastPrivateReset,
  clearPrivateState,
} from "@/lib/pwa/private-state";

const ANONYMOUS = "anonymous";

/** Observe identity without fetching it again or retaining another query cache. */
export function AccountCacheBoundary({ userId }: { userId?: string }) {
  const client = useQueryClient();
  useEffect(() => {
    let current = userId ?? client.getQueryData<MeView>(queryKeys.me)?.id;
    let resetting = false;
    const reset = async (broadcast: boolean, me?: MeView) => {
      if (resetting) return;
      resetting = true;
      if (broadcast) broadcastPrivateReset();
      await clearPrivateState(client);
      if (me) client.setQueryData(queryKeys.me, me);
      resetting = false;
      // Rebuild server-authenticated surfaces; a refresh also drops old
      // component state which is outside TanStack's ownership.
      window.location.reload();
    };
    try {
      const stored = localStorage.getItem("folio:account-id");
      const identity = userId ?? ANONYMOUS;
      localStorage.setItem("folio:account-id", identity);
      // Only a change *between* identities is an account switch. Signing in
      // from an anonymous session leaves nothing private behind to clear, and
      // the sign-in flow already navigates; resetting here forced a full
      // reload of the landing page on every sign-in, which raced whatever
      // the user (or the e2e reader-flow spec) did next.
      if (stored !== null && stored !== ANONYMOUS && stored !== identity)
        void reset(true);
    } catch {
      /* Storage may be disabled; live identity and logout still clear. */
    }
    const onAuthentication = (event: Event) => {
      if (
        (event as CustomEvent<string>).detail === "authentication" &&
        current !== undefined
      )
        void reset(true);
    };
    window.addEventListener("folio:connectivity", onAuthentication);
    const unsubscribe = client.getQueryCache().subscribe((event) => {
      if (
        resetting ||
        event.type !== "updated" ||
        event.action.type !== "success"
      )
        return;
      if (JSON.stringify(event.query.queryKey) !== JSON.stringify(queryKeys.me))
        return;
      const me = event.query.state.data as MeView | undefined;
      try {
        localStorage.setItem("folio:account-id", me?.id ?? ANONYMOUS);
      } catch {
        /* Live identity checks still apply without storage. */
      }
      if (current !== undefined && me?.id !== current) void reset(true, me);
      current = me?.id;
    });
    const channel =
      typeof BroadcastChannel !== "undefined"
        ? new BroadcastChannel(ACCOUNT_CHANNEL)
        : null;
    if (channel)
      channel.onmessage = (event) => {
        if (event.data === "reset") void reset(false);
      };
    return () => {
      window.removeEventListener("folio:connectivity", onAuthentication);
      unsubscribe();
      channel?.close();
    };
  }, [client, userId]);
  return null;
}
