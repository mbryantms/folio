/**
 * <MetadataPreviewPane> smoke — metadata-providers-1.0 M5 diff view.
 *
 * Covers the four shell states the parent dialog can route into:
 *   1. loading — spinner copy
 *   2. error   — error message + Back affordance
 *   3. diff with changes — rows render with per-decision badges +
 *      provenance tooltips, default-checked actionable rows
 *   4. diff with empty changes — "Nothing would change" copy
 *
 * Plus a focused conflict-section test: external-IDs conflicts render
 * the amber section + Keep mine / Use theirs toggles.
 *
 * Pure presentational coverage — the hook + apply flow are exercised
 * by the dialog test (mocked) and the server integration tests.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import type * as React from "react";

import type { DiffResp } from "@/lib/api/types";

// Tooltip + checkbox use Radix portals / refs. The test renderer
// (renderToStaticMarkup) doesn't simulate portals; stub them to plain
// shells so the row text is greppable.
vi.mock("@/components/ui/checkbox", () => ({
  Checkbox: (props: { id?: string; checked?: boolean; disabled?: boolean }) =>
    createElement("input", {
      type: "checkbox",
      id: props.id,
      checked: !!props.checked,
      disabled: !!props.disabled,
      readOnly: true,
    }),
}));

vi.mock("@/components/ui/tooltip", () => ({
  TooltipProvider: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  Tooltip: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  TooltipTrigger: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  TooltipContent: ({ children }: { children: React.ReactNode }) =>
    createElement("div", { "data-role": "tooltip" }, children),
}));

import {
  MetadataPreviewPane,
  defaultSelectedFields,
} from "@/components/library/MetadataPreviewPane";

function baseDiff(overrides: Partial<DiffResp> = {}): DiffResp {
  return {
    run_id: "00000000-0000-0000-0000-000000000000",
    ordinal: 0,
    scope: "series",
    source: "comicvine",
    source_external_id: "12345",
    rows: [],
    external_id_conflicts: [],
    external_ids_new: [],
    changes_count: 0,
    ...overrides,
  };
}

describe("<MetadataPreviewPane>", () => {
  it("renders the loading shell while computing", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: undefined,
        isLoading: true,
        errorMessage: null,
        selectedFields: new Set<string>(),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: true,
      }),
    );
    expect(html).toContain("Computing preview");
  });

  it("renders error message + Back affordance on error", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: undefined,
        isLoading: false,
        errorMessage: "Upstream provider exploded",
        selectedFields: new Set<string>(),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: true,
      }),
    );
    expect(html).toContain("Upstream provider exploded");
    expect(html).toContain("Back to candidates");
  });

  it("renders per-field rows with decision badges and current/proposed values", () => {
    const diff = baseDiff({
      rows: [
        {
          field: "title",
          label: "Title",
          current_value: "Old Title",
          proposed_value: "Saga",
          decision: "would_replace",
          current_set_by: "comicvine",
          current_set_at: "2026-04-15T00:00:00Z",
        },
        {
          field: "year_began",
          label: "Year began",
          current_value: null,
          proposed_value: "2012",
          decision: "would_fill",
          current_set_by: null,
          current_set_at: null,
        },
        {
          field: "publisher",
          label: "Publisher",
          current_value: "Image",
          proposed_value: "Image",
          decision: "no_change",
          current_set_by: null,
          current_set_at: null,
        },
      ],
      changes_count: 2,
    });
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: diff,
        isLoading: false,
        errorMessage: null,
        selectedFields: defaultSelectedFields(diff),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: true,
      }),
    );
    expect(html).toContain("Title");
    expect(html).toContain("Will replace");
    expect(html).toContain("Year began");
    expect(html).toContain("Will fill");
    // Same-value row renders the "Same" badge
    expect(html).toContain("Same");
    // Apply N changes button uses the selected count
    expect(html).toContain("Apply 2 changes");
  });

  it("renders the empty-changes copy when nothing would write", () => {
    const diff = baseDiff({
      rows: [
        {
          field: "title",
          label: "Title",
          current_value: "Same value",
          proposed_value: "Same value",
          decision: "no_change",
          current_set_by: "comicvine",
          current_set_at: null,
        },
      ],
      changes_count: 0,
    });
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: diff,
        isLoading: false,
        errorMessage: null,
        selectedFields: new Set<string>(),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: true,
      }),
    );
    expect(html).toContain("Nothing would change");
  });

  it("renders the external-ID conflict section with Keep mine / Use theirs", () => {
    const diff = baseDiff({
      external_id_conflicts: [
        {
          source: "comicvine",
          current_external_id: "1",
          proposed_external_id: "2",
        },
      ],
      external_ids_new: [{ source: "metron", external_id: "77777" }],
      changes_count: 2,
    });
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: diff,
        isLoading: false,
        errorMessage: null,
        selectedFields: new Set<string>(),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: true,
      }),
    );
    expect(html).toContain("External-ID conflicts");
    expect(html).toContain("Keep mine");
    expect(html).toContain("Use theirs");
    expect(html).toContain("New external IDs");
    expect(html).toContain("comicvine");
    expect(html).toContain("metron");
  });

  it("disables blocked_by_user checkbox when canOverride is false", () => {
    const diff = baseDiff({
      rows: [
        {
          field: "title",
          label: "Title",
          current_value: "User-set",
          proposed_value: "Provider",
          decision: "blocked_by_user",
          current_set_by: "user",
          current_set_at: "2026-04-15T00:00:00Z",
        },
      ],
      changes_count: 0,
    });
    const html = renderToStaticMarkup(
      createElement(MetadataPreviewPane, {
        data: diff,
        isLoading: false,
        errorMessage: null,
        selectedFields: new Set<string>(),
        overrideExternalIdSources: new Set<string>(),
        onChangeSelected: () => undefined,
        onChangeOverrideSources: () => undefined,
        onBack: () => undefined,
        onApply: () => undefined,
        isApplying: false,
        canOverride: false,
      }),
    );
    // The blocked_by_user row's checkbox renders the disabled attr —
    // non-admin can't opt the row in even though it's a real diff.
    expect(html).toContain("User-set");
    expect(html).toMatch(
      /<input[^>]*disabled[^>]*type="checkbox"|<input[^>]*type="checkbox"[^>]*disabled/,
    );
  });
});

describe("defaultSelectedFields", () => {
  it("seeds only actionable decisions (would_fill, would_replace)", () => {
    const diff: DiffResp = {
      run_id: "x",
      ordinal: 0,
      scope: "series",
      source: "comicvine",
      source_external_id: "1",
      rows: [
        {
          field: "title",
          label: "Title",
          current_value: null,
          proposed_value: "a",
          decision: "would_fill",
          current_set_by: null,
          current_set_at: null,
        },
        {
          field: "year_began",
          label: "Year began",
          current_value: "1",
          proposed_value: "2",
          decision: "would_replace",
          current_set_by: "metron",
          current_set_at: null,
        },
        {
          field: "publisher",
          label: "Publisher",
          current_value: "x",
          proposed_value: "x",
          decision: "no_change",
          current_set_by: null,
          current_set_at: null,
        },
        {
          field: "sort_name",
          label: "Sort name",
          current_value: "user",
          proposed_value: "x",
          decision: "blocked_by_user",
          current_set_by: "user",
          current_set_at: null,
        },
        {
          field: "deck",
          label: "Deck",
          current_value: "x",
          proposed_value: null,
          decision: "no_incoming_value",
          current_set_by: null,
          current_set_at: null,
        },
      ],
      external_id_conflicts: [],
      external_ids_new: [],
      changes_count: 2,
    };
    const seeded = defaultSelectedFields(diff);
    expect(seeded.has("title")).toBe(true);
    expect(seeded.has("year_began")).toBe(true);
    expect(seeded.has("publisher")).toBe(false);
    expect(seeded.has("sort_name")).toBe(false);
    expect(seeded.has("deck")).toBe(false);
  });
});
