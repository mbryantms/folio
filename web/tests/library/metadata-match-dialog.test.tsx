/**
 * <MetadataMatchForm> smoke — metadata-providers-1.0 M5.
 *
 * Renders the inner form (Radix Dialog portals don't traverse
 * `renderToStaticMarkup`) in three states — polling, completed with
 * candidates, awaiting_quota — and asserts the right shell is
 * present. Mocks the mutation + query hooks so we don't pull in
 * TanStack Query or network plumbing.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type * as React from "react";
import { createElement } from "react";

let candidatesState = {
  data: undefined as
    | undefined
    | {
        status: string;
        candidates: Array<{
          source: string;
          external_id: string;
          bucket: string;
          score: number;
          candidate: unknown;
        }>;
        providers: string[];
        error_summary: string | null;
      },
};

// vitest hoists `vi.mock` factories above their lexical position, so
// we inline the noop shape rather than referencing a shared const
// from outside the factory (that throws "Cannot access X before
// initialization" at module-load time).
//
// The dialog uses `apiMutate` directly for the auto-kick (instead of
// `useSearchMetadataForSeries`/`useSearchMetadataForIssue`) — see the
// kick-effect comment in MetadataMatchDialog.tsx for the StrictMode
// rationale. The mock stub returns a never-resolving promise so the
// "Searching providers…" shell renders deterministically (the test
// asserts the polling state, not the resolution).
vi.mock("@/lib/api/mutations", () => ({
  apiMutate: () => new Promise(() => {}),
  useApplyMetadataForSeries: () => ({
    mutate: () => undefined,
    isPending: false,
    isSuccess: false,
  }),
  useApplyMetadataForIssue: () => ({
    mutate: () => undefined,
    isPending: false,
    isSuccess: false,
  }),
}));

vi.mock("@tanstack/react-query", async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  return {
    ...actual,
    useQueryClient: () => ({
      invalidateQueries: () => undefined,
    }),
  };
});

vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: { role: "admin", id: "u1", email: "a@b.c" } }),
  useMetadataCandidatesSeries: () => candidatesState,
  // The form calls both series + issue candidate hooks; the issue
  // path is inactive in series-scope tests so just return an empty
  // shell.
  useMetadataCandidatesIssue: () => ({ data: undefined }),
  // M5 preview pane — the candidate-list rendering tests don't drill
  // into the preview, so a no-op shell is sufficient. Component-level
  // PreviewPane coverage lives in its own test file.
  useMetadataProposedDiffSeries: () => ({ data: undefined, isLoading: false, isFetching: false, error: null }),
  useMetadataProposedDiffIssue: () => ({ data: undefined, isLoading: false, isFetching: false, error: null }),
  // M5.2 — dialog queries the library to learn whether writeback is on
  // (drives the wait-for-rescan flow). Default-off in tests since the
  // immediate-close path is what the existing assertions cover.
  useLibrary: () => ({
    data: {
      allow_archive_writeback: false,
      metadata_writeback_enabled: false,
    },
  }),
}));

vi.mock("@/lib/api/scan-events", () => ({
  useScanEvents: () => ({ status: "open" as const, events: [] }),
}));

vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
}));

// Stub the Dialog primitives — Radix needs a real DialogRoot context
// that `renderToStaticMarkup` doesn't simulate. Flat passthroughs are
// fine since the inner `MetadataMatchForm` doesn't depend on Dialog
// behavior beyond rendering its children.
vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  DialogContent: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  DialogHeader: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  DialogFooter: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  DialogTitle: ({ children }: { children: React.ReactNode }) =>
    createElement("h2", null, children),
  DialogDescription: ({ children }: { children: React.ReactNode }) =>
    createElement("p", null, children),
}));

vi.mock("@/components/ui/radio-group", () => ({
  RadioGroup: ({
    value,
    children,
  }: {
    value?: string;
    children: React.ReactNode;
  }) =>
    createElement("div", { role: "radiogroup", "data-value": value }, children),
  RadioGroupItem: ({ value, id }: { value: string; id?: string }) =>
    createElement("button", {
      type: "button",
      role: "radio",
      "data-value": value,
      id,
    }),
}));

vi.mock("@/components/ui/switch", () => ({
  Switch: ({
    id,
    checked,
  }: {
    id?: string;
    checked?: boolean;
  }) =>
    createElement("input", {
      type: "checkbox",
      id,
      checked: !!checked,
      readOnly: true,
    }),
}));

import { MetadataMatchForm } from "@/components/library/MetadataMatchDialog";

describe("<MetadataMatchForm>", () => {
  it("renders the polling shell when no run yet", () => {
    candidatesState = { data: undefined };
    const html = renderToStaticMarkup(
      createElement(MetadataMatchForm, {
        scope: { kind: "series" as const, seriesSlug: "saga", libraryId: "lib-fixture" },
        onClose: () => undefined,
        open: true,
      }),
    );
    expect(html).toContain("Searching providers");
    expect(html).toContain("Fetch metadata");
    // Mode radios are present.
    expect(html).toContain('data-value="fill_missing"');
    expect(html).toContain('data-value="replace_all"');
  });

  it("renders ranked candidates when the run is completed", () => {
    candidatesState = {
      data: {
        status: "completed",
        providers: ["metron", "comicvine"],
        error_summary: null,
        candidates: [
          {
            source: "metron",
            external_id: "12345",
            bucket: "high",
            score: 92.5,
            candidate: {
              name: "Saga",
              year: 2012,
              publisher: "Image Comics",
              issue_count: 60,
            },
          },
          {
            source: "comicvine",
            external_id: "abc",
            bucket: "medium",
            score: 78,
            candidate: { name: "Sagas", year: 2013 },
          },
        ],
      },
    };
    const html = renderToStaticMarkup(
      createElement(MetadataMatchForm, {
        scope: { kind: "series" as const, seriesSlug: "saga", libraryId: "lib-fixture" },
        onClose: () => undefined,
        open: true,
      }),
    );
    expect(html).toContain("Saga");
    expect(html).toContain("Image Comics");
    expect(html).toContain("HIGH");
    expect(html).toContain("MEDIUM");
    expect(html).toContain("ComicVine");
    expect(html).toContain("Metron");
  });

  it("renders awaiting_quota explanation when every provider is exhausted", () => {
    candidatesState = {
      data: {
        status: "awaiting_quota",
        providers: ["comicvine"],
        error_summary: null,
        candidates: [],
      },
    };
    const html = renderToStaticMarkup(
      createElement(MetadataMatchForm, {
        scope: { kind: "series" as const, seriesSlug: "saga", libraryId: "lib-fixture" },
        onClose: () => undefined,
        open: true,
      }),
    );
    expect(html).toContain("out of quota");
    expect(html).toContain("Retry");
  });

  it("surfaces the admin-only override toggle", () => {
    candidatesState = {
      data: {
        status: "completed",
        providers: ["comicvine"],
        error_summary: null,
        candidates: [],
      },
    };
    const html = renderToStaticMarkup(
      createElement(MetadataMatchForm, {
        scope: { kind: "series" as const, seriesSlug: "saga", libraryId: "lib-fixture" },
        onClose: () => undefined,
        open: true,
      }),
    );
    expect(html).toContain("Override user-edited fields");
    expect(html).toContain("metadata_apply_force");
  });
});
