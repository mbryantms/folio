// @vitest-environment jsdom
/**
 * Sheet, dialog and alert-dialog headers are left-aligned at every width.
 * shadcn ships `text-center sm:text-left`, which centred phone titles over
 * their own description (the "indented" Filters title) and centred the
 * Edit issue sheet; a few sheets had patched it with `text-left` while the
 * rest had not.
 */
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AlertDialogHeader } from "@/components/ui/alert-dialog";
import { DialogHeader } from "@/components/ui/dialog";
import { SheetHeader } from "@/components/ui/sheet";

describe("overlay headers", () => {
  it.each([
    ["SheetHeader", SheetHeader],
    ["DialogHeader", DialogHeader],
    ["AlertDialogHeader", AlertDialogHeader],
  ])("%s is left-aligned on every pointer and width", (_name, Header) => {
    const { container } = render(
      <Header data-testid="h">
        <h2>Title</h2>
      </Header>,
    );
    const el = container.querySelector("[data-testid=h]")!;
    expect(el.className).toMatch(/\btext-left\b/);
    expect(el.className).not.toMatch(/text-center/);
  });
});
