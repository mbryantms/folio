/**
 * WP-8.4 — marker Undo is one `POST /me/markers/restore`.
 *
 * - `markerToRestoreItem` carries everything the server keeps (id,
 *   created_at, page hash, colour, region, selection, tags, body).
 * - `useRestoreMarkers` sends every snapshot in a single request body and
 *   invalidates each touched issue's overlay plus the global feed, count
 *   and tag rollup.
 */
import { QueryClient } from "@tanstack/react-query";
import type * as ReactQuery from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import type { MarkerView } from "@/lib/api/types";

const qc = new QueryClient();
const captured: {
  build?: (input: unknown) => { path: string; method: string; body: unknown };
  onSuccess?: (data: unknown, input: unknown) => void;
}[] = [];

vi.mock("@tanstack/react-query", async (orig) => ({
  ...(await orig<typeof ReactQuery>()),
  useQueryClient: () => qc,
}));
vi.mock("@/lib/api/mutations/_core", () => ({
  useApiMutation: (
    build: (input: unknown) => { path: string; method: string; body: unknown },
    opts: { onSuccess?: (data: unknown, input: unknown) => void },
  ) => {
    captured.push({ build, onSuccess: opts.onSuccess });
    return {};
  },
}));

import { useRestoreMarkers } from "@/lib/api/mutations/markers";

/** The mutation the last `useRestoreMarkers()` call registered. */
function lastRestore() {
  const entry = captured.at(-1);
  if (!entry?.build || !entry.onSuccess) throw new Error("no mutation");
  return { build: entry.build, onSuccess: entry.onSuccess };
}
import { markerToRestoreItem } from "@/lib/markers/recreate";

const HASH = "a".repeat(64);

function marker(over: Partial<MarkerView>): MarkerView {
  return {
    id: "m-1",
    user_id: "u-1",
    series_id: "s-1",
    issue_id: "i-1",
    page_index: 4,
    kind: "highlight",
    is_favorite: true,
    tags: ["sfx", "plot"],
    region: { x: 10, y: 20, w: 30, h: 15, shape: "text" },
    selection: { text: "BLAM" },
    body: "a note",
    color: "#336699",
    page_hash: HASH,
    hidden_from_log: true,
    created_at: "2026-09-01T10:00:00+00:00",
    updated_at: "2026-09-02T10:00:00+00:00",
    series_name: "Hydrated",
    ...over,
  };
}

describe("markerToRestoreItem", () => {
  it("keeps identity, anchor and content", () => {
    const item = markerToRestoreItem(marker({}));
    expect(item).toEqual({
      id: "m-1",
      issue_id: "i-1",
      page_index: 4,
      kind: "highlight",
      region: { x: 10, y: 20, w: 30, h: 15, shape: "text" },
      selection: { text: "BLAM" },
      body: "a note",
      color: "#336699",
      is_favorite: true,
      tags: ["sfx", "plot"],
      page_hash: HASH,
      hidden_from_log: true,
      created_at: "2026-09-01T10:00:00+00:00",
    });
    // Hydration-only fields never go back to the server.
    expect(item).not.toHaveProperty("series_name");
    expect(item).not.toHaveProperty("user_id");
  });

  it("sends null for absent optional fields", () => {
    const item = markerToRestoreItem(
      marker({
        region: undefined,
        selection: undefined,
        body: undefined,
        color: undefined,
        page_hash: undefined,
      }),
    );
    expect(item.region).toBeNull();
    expect(item.page_hash).toBeNull();
    expect(item.color).toBeNull();
  });
});

describe("useRestoreMarkers", () => {
  it("restores any number of markers in one request", () => {
    useRestoreMarkers();
    const { build } = lastRestore();
    const snapshots = [
      marker({ id: "m-1", issue_id: "i-1" }),
      marker({ id: "m-2", issue_id: "i-2", kind: "bookmark", region: null }),
      marker({ id: "m-3", issue_id: "i-1", kind: "note" }),
    ];
    const req = build(snapshots);
    expect(req.path).toBe("/me/markers/restore");
    expect(req.method).toBe("POST");
    const body = req.body as { markers: { id: string }[] };
    expect(body.markers.map((m) => m.id)).toEqual(["m-1", "m-2", "m-3"]);
  });

  it("invalidates every touched issue overlay and the global feeds", () => {
    useRestoreMarkers();
    const { onSuccess } = lastRestore();
    const spy = vi.spyOn(qc, "invalidateQueries");
    onSuccess({ restored: [], skipped: 0 }, [
      marker({ id: "m-1", issue_id: "i-1" }),
      marker({ id: "m-2", issue_id: "i-2" }),
      marker({ id: "m-3", issue_id: "i-1" }),
    ]);
    const keys = spy.mock.calls.map((c) => JSON.stringify(c[0]?.queryKey));
    expect(keys).toEqual([
      JSON.stringify(["markers", "issue", "i-1"]),
      JSON.stringify(["markers", "issue", "i-2"]),
      JSON.stringify(["markers", "list"]),
      JSON.stringify(["markers", "count"]),
      JSON.stringify(["markers", "tags"]),
    ]);
  });
});
