// @vitest-environment jsdom
/**
 * The selected tab was a 0.01 L step on the dark theme (upstream's
 * `border-input` + `bg-input/30` on Folio's opaque `--input`). Section
 * navigation now uses the line variant with an accent underline, mode
 * pickers the segmented variant with a solid primary segment; both
 * treatments are `data-slot` hooks in `globals.css` (2026-10-30).
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Tabs, TabsList, TabsTrigger } from "@/components/folio/tabs";

describe("TabsList variants", () => {
  it("defaults to the line variant on a full-width hairline", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">Credits</TabsTrigger>
          <TabsTrigger value="b">Details</TabsTrigger>
        </TabsList>
      </Tabs>,
    );
    const list = screen.getByRole("tablist");
    expect(list.dataset.variant).toBe("line");
    expect(list.className).toMatch(/\bborder-b\b/);
    expect(list.className).toMatch(/\bmd:w-full\b/);
    expect(list.className).not.toMatch(/\bmd:w-fit\b/);
  });

  it("passes the segmented variant through for mode pickers", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList variant="default">
          <TabsTrigger value="a">Sign in</TabsTrigger>
          <TabsTrigger value="b">Register</TabsTrigger>
        </TabsList>
      </Tabs>,
    );
    const list = screen.getByRole("tablist");
    expect(list.dataset.variant).toBe("default");
    expect(list.className).toMatch(/\bbg-muted\b/);
    expect(list.className).not.toMatch(/\bborder-b\b/);
  });

  it("globals.css paints the active tab with the accent in both variants", () => {
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    // Line: the upstream `::after` indicator takes `--primary`.
    expect(css).toMatch(
      /\[data-slot="tabs-list"\]\[data-variant="line"\]\s*\[data-slot="tabs-trigger"\]\[data-slot="tabs-trigger"\]::after\s*\{[^}]*background-color:\s*var\(--primary\)/,
    );
    // Default: the selected segment is a solid primary pill.
    expect(css).toMatch(
      /\[data-slot="tabs-list"\]\[data-variant="default"\]\s*\[data-slot="tabs-trigger"\]\[data-slot="tabs-trigger"\]\[data-state="active"\]\s*\{[^}]*background-color:\s*var\(--primary\)/,
    );
  });
});
