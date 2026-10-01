/**
 * `<ProvidersTab>` budget bar + last-error smoke (WP-2.9).
 *
 * Static-markup render with the queries/mutations mocked, mirroring the
 * `provider-config-form` test. Verifies the per-provider card renders
 * the upstream budget line, the low-budget tone, and the last error.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

const providers = [
  {
    id: "metron",
    label: "Metron",
    enabled: true,
    configured: true,
    quota: { remaining_hour: 18, remaining_day: 812, seconds_until_reset: 42 },
    budget: {
      limit: 5000,
      remaining: 812,
      // Far future so the countdown is present whatever the wall clock says.
      reset_at: "2999-01-01T00:00:00Z",
      window: "day",
    },
    last_error: {
      message: "provider error: HTTP 503",
      at: "2026-01-01T00:00:00Z",
    },
  },
  {
    id: "comicvine",
    label: "ComicVine",
    enabled: false,
    configured: false,
    quota: null,
    budget: null,
    last_error: null,
  },
  {
    // WP-6.1: GCD's bar comes from the local 2,000/day bucket.
    id: "gcd",
    label: "Grand Comics Database",
    enabled: true,
    configured: true,
    quota: {
      remaining_hour: 95,
      remaining_day: 1900,
      seconds_until_reset: 600,
    },
    budget: {
      limit: 2000,
      remaining: 1900,
      reset_at: "2999-01-01T00:00:00Z",
      window: "day",
    },
    last_error: null,
  },
];

vi.mock("@/lib/api/queries", () => ({
  useAdminMetadataProviders: () => ({ data: { providers }, isLoading: false }),
  useAdminSettings: () => ({ data: { values: [] }, isLoading: false }),
}));

vi.mock("@/lib/api/mutations", () => ({
  useTestMetadataProvider: () => ({
    mutate: () => undefined,
    isPending: false,
    data: undefined,
    error: null,
  }),
  useUpdateSettings: () => ({
    mutateAsync: async () => undefined,
    isPending: false,
  }),
}));

vi.mock("@/components/ui/switch", () => ({
  Switch: ({ id, checked }: { id?: string; checked?: boolean }) =>
    createElement("input", {
      type: "checkbox",
      id,
      checked: !!checked,
      readOnly: true,
    }),
}));

import { ProvidersTab } from "@/components/admin/metadata/ProvidersTab";

describe("<ProvidersTab> budget bar", () => {
  it("renders the upstream budget, low tone, and last error per provider", () => {
    const html = renderToStaticMarkup(createElement(ProvidersTab));
    // Budget line from the upstream figure (812/5000 = 16% → low).
    expect(html).toContain("812 of 5,000 left today");
    expect(html).toContain("resets in");
    expect(html).toContain('data-testid="provider-budget"');
    expect(html).toContain("text-warning");
    // Last error surfaces with its message.
    expect(html).toContain('data-testid="provider-last-error"');
    expect(html).toContain("provider error: HTTP 503");
    // The unconfigured provider renders neither.
    expect(html.match(/data-testid="provider-budget"/g)?.length).toBe(2);
    expect(html).toContain("NOT CONFIGURED");
  });

  it("renders the GCD card with its budget bar, docs link, and credential form", () => {
    const html = renderToStaticMarkup(createElement(ProvidersTab));
    expect(html).toContain("Grand Comics Database");
    expect(html).toContain("1,900 of 2,000 left today");
    expect(html).toContain('href="https://www.comics.org/api/"');
    expect(html).toContain('id="gcd-username"');
    expect(html).toContain("Enable GCD");
  });
});
