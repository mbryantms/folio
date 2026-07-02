/**
 * `<ProviderConfigForm>` smoke — metadata-providers-1.0 M6.
 *
 * Verifies the form reads the right secret-set sentinel ("<set>")
 * vs an empty value, renders the right input shape per provider,
 * and that the Save button is disabled until the user types.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

let settingsState: {
  data:
    | undefined
    | { values: Array<{ key: string; value: unknown; is_secret: boolean }> };
  isLoading: boolean;
} = { data: undefined, isLoading: false };

vi.mock("@/lib/api/queries", () => ({
  useAdminSettings: () => settingsState,
}));

vi.mock("@/lib/api/mutations", () => ({
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

import { ProviderConfigForm } from "@/components/admin/metadata/ProviderConfigForm";

describe("<ProviderConfigForm>", () => {
  it("renders the loading shell while settings are pending", () => {
    settingsState = { data: undefined, isLoading: true };
    const html = renderToStaticMarkup(
      createElement(ProviderConfigForm, { provider: "comicvine" }),
    );
    expect(html).toContain("Loading credentials");
  });

  it("renders the ComicVine form with a (saved) placeholder when key is set", () => {
    settingsState = {
      isLoading: false,
      data: {
        values: [
          {
            key: "metadata.comicvine.api_key",
            value: "<set>",
            is_secret: true,
          },
          {
            key: "metadata.comicvine.enabled",
            value: true,
            is_secret: false,
          },
        ],
      },
    };
    const html = renderToStaticMarkup(
      createElement(ProviderConfigForm, { provider: "comicvine" }),
    );
    expect(html).toContain("API key");
    expect(html).toContain("saved");
    expect(html).toContain("Enable ComicVine");
    // Save disabled until user types — the disabled attribute lands
    // as `disabled=""` in the static markup.
    expect(html).toMatch(/<button[^>]*disabled[^>]*>[^<]*Save/);
  });

  it("renders the Metron form with username + password inputs", () => {
    settingsState = {
      isLoading: false,
      data: {
        values: [
          {
            key: "metadata.metron.username",
            value: "alice",
            is_secret: false,
          },
          { key: "metadata.metron.password", value: "", is_secret: true },
          {
            key: "metadata.metron.enabled",
            value: false,
            is_secret: false,
          },
        ],
      },
    };
    const html = renderToStaticMarkup(
      createElement(ProviderConfigForm, { provider: "metron" }),
    );
    expect(html).toContain("Username");
    expect(html).toContain('value="alice"');
    expect(html).toContain("Password");
    expect(html).toContain("metron.cloud password");
    expect(html).toContain("Enable Metron");
  });
});
