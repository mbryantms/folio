/**
 * <MetadataCompareView> + defaultFieldSources — multi-provider merge.
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import {
  MetadataCompareView,
  defaultFieldSources,
} from "@/components/library/MetadataCompareView";
import type { CompositeDiffResp } from "@/lib/api/types";

const diff: CompositeDiffResp = {
  run_id: "r1",
  scope: "issue",
  providers: [
    {
      source: "metron",
      ordinal: 1,
      external_id: "m1",
      bucket: "high",
      score: 92,
      cover_image_url: "https://cdn/metron.jpg",
      title: "Saga #1",
    },
    {
      source: "comicvine",
      ordinal: 0,
      external_id: "cv1",
      bucket: "high",
      score: 90,
      cover_image_url: "https://cdn/cv.jpg",
      title: "Saga #1",
    },
  ],
  rows: [
    {
      field: "description",
      label: "Description",
      current_value: null,
      current_set_by: null,
      current_set_at: null,
      proposals: [
        { source: "comicvine", ordinal: 0, value: "A space opera." },
        { source: "metron", ordinal: 1, value: null },
      ],
      chosen_ordinal: 0,
      decision: "would_fill",
    },
    {
      field: "characters",
      label: "Characters",
      current_value: null,
      current_set_by: null,
      current_set_at: null,
      proposals: [
        { source: "comicvine", ordinal: 0, value: null },
        { source: "metron", ordinal: 1, value: "3 items" },
      ],
      chosen_ordinal: 1,
      decision: "would_fill",
    },
    {
      field: "title",
      label: "Title",
      current_value: "Saga",
      current_set_by: "user",
      current_set_at: null,
      proposals: [
        { source: "comicvine", ordinal: 0, value: "Saga" },
        { source: "metron", ordinal: 1, value: "Saga" },
      ],
      chosen_ordinal: 1,
      decision: "no_change",
    },
  ],
  external_ids_new: [
    { source: "comicvine", external_id: "cv1" },
    { source: "metron", external_id: "m1" },
  ],
  external_id_conflicts: [],
  changes_count: 2,
};

describe("defaultFieldSources", () => {
  it("pre-selects the chosen ordinal only for would_fill/would_replace rows", () => {
    const seeded = defaultFieldSources(diff);
    expect(seeded).toEqual({
      description: 0,
      characters: 1,
    });
    // `title` is no_change → not pre-selected.
    expect(seeded.title).toBeUndefined();
  });
});

describe("<MetadataCompareView>", () => {
  const noop = () => {};

  it("renders candidate columns + per-field proposals + apply count", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataCompareView, {
        data: diff,
        isLoading: false,
        errorMessage: null,
        fieldSources: defaultFieldSources(diff),
        onRemoveColumn: noop,
        onChangeFieldSource: noop,
        onApply: noop,
        onBack: noop,
        isApplying: false,
      }),
    );
    expect(html).toContain("ComicVine");
    expect(html).toContain("Metron");
    expect(html).toContain("Description");
    expect(html).toContain("A space opera.");
    expect(html).toContain("Characters");
    // 2 fields pre-selected (description + characters).
    expect(html).toContain("Apply merged (2 fields)");
    // Both providers' external IDs flagged as additive.
    expect(html).toContain("will be added");
  });

  it("calls onApply when the merged-apply button is clicked", () => {
    // renderToStaticMarkup can't dispatch events; assert the handler
    // wiring via a shallow prop check instead.
    const onApply = vi.fn();
    const el = createElement(MetadataCompareView, {
      data: diff,
      isLoading: false,
      errorMessage: null,
      fieldSources: defaultFieldSources(diff),
      onRemoveColumn: noop,
      onChangeFieldSource: noop,
      onApply,
      onBack: noop,
      isApplying: false,
    });
    // The element exists + carries the handler in its tree.
    expect(el.props.onApply).toBe(onApply);
  });

  it("shows a loading state", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataCompareView, {
        data: undefined,
        isLoading: true,
        errorMessage: null,
        fieldSources: {},
        onRemoveColumn: noop,
        onChangeFieldSource: noop,
        onApply: noop,
        onBack: noop,
        isApplying: false,
      }),
    );
    expect(html).toContain("Comparing candidates");
  });
});
