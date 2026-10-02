/**
 * WP-7.3 review queue: the list helpers and the mutation invalidation.
 *
 * - `relationshipSuggestionsNextPage` must hand `next_cursor` back so the
 *   infinite query walks every page (no silent truncation).
 * - `relationshipSuggestionsPath` puts every filter on the query string
 *   (server-side filters, never a client `.filter()`).
 * - Accepting a suggestion invalidates both series' "Related" blocks, the
 *   similar-series rails, and every suggestion list.
 */
import { QueryClient } from "@tanstack/react-query";
import type * as ReactQuery from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import type {
  AcceptRelationshipSuggestionResp,
  BulkReviewRelationshipSuggestionsResp,
  RelationshipSuggestionListView,
} from "@/lib/api/types";

const qc = new QueryClient();
const captured: {
  onSuccess?: (data: unknown, input: unknown) => void;
}[] = [];

vi.mock("@tanstack/react-query", async (orig) => ({
  ...(await orig<typeof ReactQuery>()),
  useQueryClient: () => qc,
}));
vi.mock("@/lib/api/mutations/_core", () => ({
  useApiMutation: (
    _build: unknown,
    opts: { onSuccess?: (data: unknown, input: unknown) => void },
  ) => {
    captured.push({ onSuccess: opts.onSuccess });
    return {};
  },
}));

import {
  bulkSummary,
  useAcceptRelationshipSuggestion,
  useBulkAcceptRelationshipSuggestions,
} from "@/lib/api/mutations/relationship-suggestions";
import {
  queryKeys,
  relationshipSuggestionsNextPage,
  relationshipSuggestionsPath,
} from "@/lib/api/queries";

function page(next: string | null): RelationshipSuggestionListView {
  return { items: [], next_cursor: next };
}

describe("relationship suggestion list helpers", () => {
  it("walks next_cursor and stops on null", () => {
    expect(relationshipSuggestionsNextPage(page("abc"))).toBe("abc");
    expect(relationshipSuggestionsNextPage(page(null))).toBeUndefined();
  });

  it("puts every filter on the query string", () => {
    const path = relationshipSuggestionsPath(
      { status: "rejected", bucket: "high", libraryId: "lib-1" },
      "cur",
    );
    const url = new URL(path, "http://x");
    expect(url.pathname).toBe("/admin/relationship-suggestions");
    expect(url.searchParams.get("status")).toBe("rejected");
    expect(url.searchParams.get("bucket")).toBe("high");
    expect(url.searchParams.get("library_id")).toBe("lib-1");
    expect(url.searchParams.get("cursor")).toBe("cur");
    const bare = new URL(
      relationshipSuggestionsPath({
        status: "pending",
        bucket: null,
        libraryId: null,
      }),
      "http://x",
    );
    expect(bare.searchParams.has("bucket")).toBe(false);
    expect(bare.searchParams.has("library_id")).toBe(false);
    expect(bare.searchParams.has("cursor")).toBe(false);
  });
});

describe("relationship suggestion mutations", () => {
  it("accept invalidates both series, similar rails and suggestion lists", () => {
    captured.length = 0;
    const spy = vi.spyOn(qc, "invalidateQueries");
    useAcceptRelationshipSuggestion();
    const resp = {
      suggestion: {
        from_series: { slug: "dd-2014", name: "Daredevil" },
        to_series: { slug: "dd-2011", name: "Daredevil" },
      },
    } as unknown as AcceptRelationshipSuggestionResp;
    captured[0]!.onSuccess!(resp, { id: "s1" });
    const keys = spy.mock.calls.map((c) => JSON.stringify(c[0]?.queryKey));
    expect(keys).toContain(
      JSON.stringify(queryKeys.seriesRelationships("dd-2014")),
    );
    expect(keys).toContain(
      JSON.stringify(queryKeys.seriesRelationships("dd-2011")),
    );
    expect(keys).toContain(JSON.stringify(["similar"]));
    expect(keys).toContain(
      JSON.stringify(queryKeys.relationshipSuggestionsAll),
    );
    // The per-series chip lists go through a predicate.
    const predicate = spy.mock.calls.find((c) => c[0]?.predicate)?.[0]
      ?.predicate;
    expect(predicate).toBeTypeOf("function");
    spy.mockRestore();
  });

  it("bulk accept invalidates every series' relationships", () => {
    captured.length = 0;
    const spy = vi.spyOn(qc, "invalidateQueries");
    useBulkAcceptRelationshipSuggestions();
    captured[0]!.onSuccess!(null, { bucket: "high" });
    const predicates = spy.mock.calls
      .map((c) => c[0]?.predicate)
      .filter((p): p is NonNullable<typeof p> => !!p);
    const matches = (key: unknown[]) =>
      predicates.some((p) => p({ queryKey: key } as never));
    expect(matches(["series", "any-slug", "relationships"])).toBe(true);
    expect(matches(["series", "any-slug", "relationship-suggestions"])).toBe(
      true,
    );
    expect(matches(["series", "any-slug", "issues"])).toBe(false);
    spy.mockRestore();
  });

  it("summarises a bulk batch", () => {
    const data: BulkReviewRelationshipSuggestionsResp = {
      requested: 5,
      succeeded: ["a", "b", "c"],
      failed: [
        { id: "d", code: "conflict", message: "x" },
        { id: "e", code: "already_reviewed", message: "y" },
      ],
      created: 3,
      remaining: 40,
    };
    expect(bulkSummary("Accepted", data)).toBe(
      "Accepted 3 suggestions · 2 skipped · 40 still pending — run again for the next batch",
    );
    expect(
      bulkSummary("Rejected", {
        requested: 1,
        succeeded: ["a"],
        failed: [],
        created: 0,
      }),
    ).toBe("Rejected 1 suggestion");
  });
});
