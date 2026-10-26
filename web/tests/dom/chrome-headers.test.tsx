// @vitest-environment jsdom
/**
 * Sheet, dialog and alert-dialog headers are left-aligned at every width.
 * shadcn used to ship `text-center sm:text-left`, which centred phone
 * titles over their own description (the "indented" Filters title). The
 * files under components/ui are vendored verbatim now: dialog and sheet
 * headers carry no alignment class upstream, and the alert-dialog header
 * (centred under `sm` upstream) is overridden in globals.css by its
 * data-slot.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AlertDialogHeader } from "@/components/ui/alert-dialog";
import { DialogHeader } from "@/components/ui/dialog";
import { SheetHeader } from "@/components/folio/sheet";

describe("overlay headers", () => {
  it.each([
    ["SheetHeader", SheetHeader],
    ["DialogHeader", DialogHeader],
  ])("%s carries no centring class", (_name, Header) => {
    const { container } = render(
      <Header data-testid="h">
        <h2>Title</h2>
      </Header>,
    );
    const el = container.querySelector("[data-testid=h]")!;
    expect(el.className).not.toMatch(/text-center/);
  });

  it("AlertDialogHeader is forced left by its data-slot at every width", () => {
    const { container } = render(
      <AlertDialogHeader data-testid="h">
        <h2>Title</h2>
      </AlertDialogHeader>,
    );
    const el = container.querySelector("[data-testid=h]")!;
    expect(el.getAttribute("data-slot")).toBe("alert-dialog-header");
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    expect(css).toMatch(
      /\[data-slot="alert-dialog-header"\]\[data-slot="alert-dialog-header"\]\s*\{[^}]*text-align:\s*left;/,
    );
  });
});
