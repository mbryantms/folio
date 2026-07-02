/**
 * <SourcesFooter> smoke — metadata-providers-1.0 M5.
 *
 * Verifies the TOS-attribution behavior:
 * - renders nothing for empty + scanner-only rows
 * - links every provider row to its external_url
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { SourcesFooter } from "@/components/library/SourcesFooter";
import type { ExternalIdRow } from "@/lib/api/types";

const cv: ExternalIdRow = {
  source: "comicvine",
  source_label: "ComicVine",
  external_id: "12345",
  external_url: "https://comicvine.gamespot.com/volume/4050-12345/",
  set_by: "comicvine",
  first_set_at: "2026-05-26T00:00:00Z",
  last_synced_at: "2026-05-26T00:00:00Z",
};

const metron: ExternalIdRow = {
  source: "metron",
  source_label: "Metron",
  external_id: "456",
  external_url: "https://metron.cloud/series/456/",
  set_by: "metron",
  first_set_at: "2026-05-26T00:00:00Z",
  last_synced_at: "2026-05-26T00:00:00Z",
};

const isbn: ExternalIdRow = {
  source: "isbn",
  source_label: "ISBN",
  external_id: "9780123456789",
  external_url: null,
  set_by: "user",
  first_set_at: "2026-05-26T00:00:00Z",
  last_synced_at: "2026-05-26T00:00:00Z",
};

describe("<SourcesFooter>", () => {
  it("renders nothing when rows is empty", () => {
    const html = renderToStaticMarkup(
      createElement(SourcesFooter, { rows: [] }),
    );
    expect(html).toBe("");
  });

  it("renders nothing when no row carries an attribution-required source", () => {
    const html = renderToStaticMarkup(
      createElement(SourcesFooter, { rows: [isbn] }),
    );
    expect(html).toBe("");
  });

  it("renders linked provider sources comma-separated", () => {
    const html = renderToStaticMarkup(
      createElement(SourcesFooter, { rows: [cv, metron] }),
    );
    expect(html).toContain("Data from");
    expect(html).toContain("ComicVine");
    expect(html).toContain("Metron");
    expect(html).toContain(
      'href="https://comicvine.gamespot.com/volume/4050-12345/"',
    );
    expect(html).toContain('href="https://metron.cloud/series/456/"');
    expect(html).toContain(", ");
  });

  it("filters out non-attribution sources but keeps CV / Metron", () => {
    const html = renderToStaticMarkup(
      createElement(SourcesFooter, { rows: [cv, isbn] }),
    );
    expect(html).toContain("ComicVine");
    expect(html).not.toContain("ISBN");
  });
});
