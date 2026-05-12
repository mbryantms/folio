"use client";

import { useQuery } from "@tanstack/react-query";

import { apiFetch } from "@/lib/api/auth-refresh";
import { queryKeys } from "@/lib/api/queries";
import type { OptionsEndpoint } from "./field-registry";

type OptionsResp = { values: string[] };

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
    queryFn: async () => {
      if (!endpoint) return { values: [] } as OptionsResp;
      const res = await apiFetch(endpointPath(endpoint, qs), {
        headers: { Accept: "application/json" },
      });
      if (!res.ok) {
        throw new Error(`filter-options ${endpoint.kind} → ${res.status}`);
      }
      return (await res.json()) as OptionsResp;
    },
    enabled: !!endpoint,
    staleTime: 60_000,
  });
}
