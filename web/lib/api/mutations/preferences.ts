/**
 * `PATCH /me/preferences`. Sharded out of the mutations barrel
 * (re-exported there) so the reader's display-pref sync imports it
 * without pulling the whole barrel into its first-load JS
 * (`scripts/check-bundle-size.mjs`).
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { MeView, PreferencesReq } from "../types";
import { useApiMutation } from "./_core";

/**
 * `PATCH /me/preferences`. Updates the calling user's reader/theme/keybind
 * preferences and returns the refreshed `MeView`. The cache is updated in
 * place so the new prefs are immediately visible to every consumer of
 * `useMe()` without an extra round-trip.
 */
export function useUpdatePreferences(opts?: {
  silent?: boolean;
  /** Collapse repeated error toasts into one (autosave-like callers such
   *  as the reader's coalesced display-pref sync). */
  toastId?: string;
}) {
  const qc = useQueryClient();
  return useApiMutation<MeView, PreferencesReq>(
    (body) => ({ path: "/me/preferences", method: "PATCH", body }),
    {
      ...(opts?.silent ? {} : { successMessage: "Preferences saved" }),
      ...(opts?.toastId ? { toastId: opts.toastId } : {}),
      onSuccess: (data) => {
        if (data) qc.setQueryData(queryKeys.me, data);
      },
    },
  );
}
