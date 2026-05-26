/**
 * <ProviderBadges> smoke — metadata-providers-1.0 M5.2.
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { ProviderBadges } from "@/components/library/ProviderBadges";
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

describe("<ProviderBadges>", () => {
  it("renders nothing for empty rows", () => {
    const html = renderToStaticMarkup(
      createElement(ProviderBadges, { rows: [] }),
    );
    expect(html).toBe("");
  });

  it("renders nothing when no row carries an attribution-required source", () => {
    const html = renderToStaticMarkup(
      createElement(ProviderBadges, { rows: [isbn] }),
    );
    expect(html).toBe("");
  });

  it("renders abbreviated linked pills for CV + Metron", () => {
    const html = renderToStaticMarkup(
      createElement(ProviderBadges, { rows: [cv, metron] }),
    );
    expect(html).toContain("CV");
    expect(html).toContain("Metron");
    expect(html).toContain('href="https://comicvine.gamespot.com/volume/4050-12345/"');
    expect(html).toContain('href="https://metron.cloud/series/456/"');
  });

  it("renders bare span when external_url missing on an attributable row", () => {
    const cvNoUrl: ExternalIdRow = { ...cv, external_url: null };
    const html = renderToStaticMarkup(
      createElement(ProviderBadges, { rows: [cvNoUrl] }),
    );
    expect(html).toContain("CV");
    expect(html).not.toContain("href=");
  });
});
