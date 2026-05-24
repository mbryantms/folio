"use client";

import { useQuery } from "@tanstack/react-query";

import { apiFetch } from "@/lib/api/auth-refresh";
import { queryKeys } from "@/lib/api/queries";
import type { components } from "@/lib/api/types.generated";
import type { OptionsEndpoint } from "./field-registry";

/**
 * Server returns `CursorPage<String>` (audit-remediation M4-residual,
 * 2026-05-23). The hook unwraps `.items` so this surface stays
 * `string[]` for consumers. Pagination tokens (`next_cursor`) are
 * dropped here — autocomplete UX is refine-by-prefix rather than
 * load-more; if a deployment ever needs deep walking the hook can
 * upgrade to `useInfiniteQuery` without breaking call sites.
 */
type OptionsPage = components["schemas"]["CursorPage_String"];

function endpointKey(endpoint: OptionsEndpoint): string {
  switch (endpoint.kind) {
    case "genres":
    case "tags":
    case "publishers":
    case "languages":
    case "age_ratings":
    case "characters":
    case "teams":
    case "locations":
      return endpoint.kind;
    case "credits":
      return `credits/${endpoint.role}`;
    case "libraries":
      return "libraries";
  }
}

function endpointPath(endpoint: OptionsEndpoint, qs: string): string {
  switch (endpoint.kind) {
    case "genres":
      return `/filter-options/genres${qs}`;
    case "tags":
      return `/filter-options/tags${qs}`;
    case "publishers":
      return `/filter-options/publishers${qs}`;
    case "languages":
      return `/filter-options/languages${qs}`;
    case "age_ratings":
      return `/filter-options/age_ratings${qs}`;
    case "characters":
      return `/filter-options/characters${qs}`;
    case "teams":
      return `/filter-options/teams${qs}`;
    case "locations":
      return `/filter-options/locations${qs}`;
    case "credits":
      return `/filter-options/credits/${encodeURIComponent(endpoint.role)}${qs}`;
    case "libraries":
      // Libraries are served from the existing list endpoint and adapted
      // by the caller — no `OptionsResp` shape there.
      return `/libraries`;
  }
}

/** Async option lookup for filter value editors. Pass `endpoint =
 *  undefined` to skip — the hook returns `data: undefined` and never
 *  fires. */
export function useFilterOptions(
  endpoint: OptionsEndpoint | undefined,
  opts?: { library?: string; q?: string },
) {
  const sp = new URLSearchParams();
  if (opts?.library) sp.set("library", opts.library);
  if (opts?.q && opts.q.trim() !== "") sp.set("q", opts.q.trim());
  const qs = sp.toString() ? `?${sp.toString()}` : "";

  return useQuery({
    queryKey: endpoint
      ? queryKeys.filterOptions(endpointKey(endpoint), {
          library: opts?.library,
          q: opts?.q,
        })
      : ["filter-options", "noop"],
    queryFn: async (): Promise<string[]> => {
      if (!endpoint) return [];
      const res = await apiFetch(endpointPath(endpoint, qs), {
        headers: { Accept: "application/json" },
      });
      if (!res.ok) {
        throw new Error(`filter-options ${endpoint.kind} → ${res.status}`);
      }
      const page = (await res.json()) as OptionsPage;
      return page.items ?? [];
    },
    enabled: !!endpoint,
    staleTime: 60_000,
  });
}
