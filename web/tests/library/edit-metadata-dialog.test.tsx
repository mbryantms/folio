/**
 * <EditMetadataForm> smoke — `manga-and-bulk-metadata-1.0` M5.
 *
 * Verifies the form renders the field set + mode controls. We test
 * the inner form rather than the Dialog shell because Radix Dialog
 * uses a portal and `renderToStaticMarkup` doesn't traverse portals.
 * We mock `useBulkUpdateMetadata` to avoid pulling in TanStack Query
 * plumbing.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type * as React from "react";
import { createElement } from "react";

vi.mock("@/lib/api/mutations", () => ({
  useBulkUpdateMetadata: () => ({
    mutate: () => undefined,
    isPending: false,
  }),
}));

// Stub the shadcn dialog primitives so the form can render outside a
// Radix `<Dialog>` context. The real components require the context;
// for unit tests we only care that the children mount, so flat <div>
// stand-ins are enough.
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

import { EditMetadataForm } from "@/components/library/EditMetadataDialog";

const noop = () => undefined;

function render(ids: string[]): string {
  return renderToStaticMarkup(
    createElement(EditMetadataForm, {
      issueIds: ids,
      onClose: noop,
    }),
  );
}

describe("<EditMetadataForm>", () => {
  it("shows the count of selected issues in the title", () => {
    const html = render(["a", "b", "c"]);
    expect(html).toContain("Edit 3 issues");
    expect(html).toContain("Apply to 3 issues");
  });

  it("singular pluralizes correctly for a single issue", () => {
    const html = render(["a"]);
    expect(html).toContain("Edit 1 issue");
    expect(html).toContain("Apply to 1 issue");
    expect(html).not.toContain("Apply to 1 issues");
  });

  it("surfaces the supported field set (and only that set)", () => {
    const html = render(["a"]);
    for (const label of [
      "Language",
      "Manga (reading direction)",
      "Publisher",
      "Imprint",
      "Age rating",
      "Format",
      "Genre (CSV)",
      "Tags (CSV)",
      "Story arc",
    ]) {
      expect(html).toContain(label);
    }
    // Credit fields must not appear — they're deliberately excluded.
    for (const credit of ["Writer", "Penciller", "Translator", "Editor"]) {
      expect(html).not.toContain(credit);
    }
  });

  it("includes both mode options with skip-if-set as the default", () => {
    const html = render(["a"]);
    expect(html).toContain("Skip already-set");
    expect(html).toContain("Replace existing values");
    // Default radio: skip_if_set is `checked`. React serializes
    // attributes in declaration order which puts `checked` before
    // `value`, so just check both appear inside one `<input>`.
    expect(html).toMatch(/<input[^>]*checked[^>]*value="skip_if_set"/);
  });

  it("describes the credit-field exclusion in the dialog body", () => {
    const html = render(["a"]);
    expect(html).toContain("Credit fields");
    expect(html).toContain("per-issue");
  });

  it("disables the Apply button when no issues are selected", () => {
    const html = render([]);
    expect(html).toMatch(/Apply to 0 issues[^<]*<\/button>/s);
    expect(html).toMatch(/disabled[^>]*>\s*Apply to 0/);
  });
});
