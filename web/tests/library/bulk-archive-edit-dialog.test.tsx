/**
 * <BulkArchiveEditDialog> smoke — `archive-rewrite-1.0` M7.
 *
 * Renders the dialog body with the Radix primitives stubbed (so
 * `renderToStaticMarkup` traverses them) and the mutation mocked. Verifies
 * the op set, the selected-issue count plumbing, and the empty-selection
 * guard on the Apply button.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type * as React from "react";
import { createElement } from "react";

vi.mock("@/lib/api/mutations", () => ({
  useBulkArchiveEditMutation: () => ({
    mutate: () => undefined,
    isPending: false,
  }),
}));

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

vi.mock("@/components/ui/select", () => ({
  Select: ({ children }: { children: React.ReactNode }) =>
    createElement("div", null, children),
  SelectTrigger: ({ children }: { children: React.ReactNode }) =>
    createElement("button", { type: "button" }, children),
  SelectValue: ({ placeholder }: { placeholder?: string }) =>
    createElement("span", null, placeholder ?? ""),
  SelectContent: ({ children }: { children: React.ReactNode }) =>
    createElement("ul", null, children),
  SelectItem: ({
    value,
    children,
  }: {
    value: string;
    children: React.ReactNode;
  }) => createElement("li", { "data-value": value, role: "option" }, children),
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
  RadioGroupItem: ({ value }: { value: string }) =>
    createElement("button", {
      type: "button",
      role: "radio",
      "data-value": value,
    }),
}));

import { BulkArchiveEditDialog } from "@/components/library/BulkArchiveEditDialog";

const noop = () => undefined;

function render(ids: string[]): string {
  return renderToStaticMarkup(
    createElement(BulkArchiveEditDialog, {
      open: true,
      onOpenChange: noop,
      issueIds: ids,
    }),
  );
}

describe("<BulkArchiveEditDialog>", () => {
  it("offers the four relative ops", () => {
    const html = render(["a", "b"]);
    for (const label of [
      "Rotate cover",
      "Rotate every page",
      "Remove first pages",
      "Remove last pages",
    ]) {
      expect(html).toContain(label);
    }
  });

  it("defaults to rotate-cover and shows the rotation choices", () => {
    const html = render(["a"]);
    expect(html).toMatch(/role="radiogroup"[^>]*data-value="rotate_cover"/);
    // Rotation branch (not the remove-count input) renders by default.
    expect(html).toContain("180°");
    expect(html).toContain("90° clockwise");
  });

  it("plumbs the selected-issue count into the description + button", () => {
    const many = render(["a", "b", "c"]);
    expect(many).toContain("all 3 selected issues");
    expect(many).toContain("Apply to 3 issues");

    const one = render(["a"]);
    expect(one).toContain("all 1 selected issue");
    expect(one).toContain("Apply to 1 issue");
    expect(one).not.toContain("Apply to 1 issues");
  });

  it("disables Apply when nothing is selected", () => {
    const html = render([]);
    expect(html).toMatch(/disabled[^>]*>\s*Apply to 0 issues/);
  });
});
