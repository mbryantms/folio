// @vitest-environment jsdom
/**
 * docs/dev/phone-layout.md, rule 11: one control order at every width.
 * Right cluster: primary · Select · ▦ View options · ⋯ More actions,
 * with ⋯ always last. The phone bar owns that order itself; the desktop
 * rows are checked by source order in the surfaces that compose them.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PhoneControlsBar } from "@/components/library/PhoneControlsBar";

const read = (rel: string) => readFileSync(join(process.cwd(), rel), "utf8");
const orderOf = (src: string, needles: string[]) =>
  needles.map((n) => {
    const at = src.indexOf(n);
    if (at < 0) throw new Error(`missing ${n}`);
    return at;
  });
const ascending = (xs: number[]) =>
  xs.every((x, i) => i === 0 || (xs[i - 1] as number) < x);

describe("control row order (rule 11)", () => {
  it("PhoneControlsBar: leading left; search · sort · filters · primary · ▦ · ⋯ right", () => {
    render(
      <PhoneControlsBar
        leading={<span data-testid="lead">Scope</span>}
        search={{ value: "", onChange: () => {} }}
        sort={{
          value: "a",
          options: [{ value: "a", label: "A" }],
          onChange: () => {},
        }}
        filters={{ count: 0, onOpen: () => {} }}
        primary={<button type="button">Up Next</button>}
        menu={() => <div>sheet</div>}
        more={<button type="button" aria-label="More actions" />}
      />,
    );
    const bar = document.querySelector("[data-phone-controls]") as HTMLElement;
    const names = within(bar)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label") ?? b.textContent);
    expect(names).toEqual([
      "Search",
      "Sort: A",
      "Filters",
      "Up Next",
      "View options",
      "More actions",
    ]);
    const lead = screen.getByTestId("lead");
    const cluster = within(bar).getByRole("button", { name: "Search" })
      .parentElement as HTMLElement;
    expect(cluster.className).toMatch(/\bml-auto\b/);
    expect(cluster.contains(lead)).toBe(false);
  });

  it("pages pass ⋯ as `more`, never as `primary`", () => {
    for (const rel of [
      "components/saved-views/PageRails.tsx",
      "components/saved-views/CblViewDetail.tsx",
      "components/markers/MarkersList.tsx",
    ]) {
      const src = read(rel);
      expect(src, rel).toMatch(/\n\s*more=\{/);
      expect(src, rel).not.toMatch(/primary=\{pageActions\}/);
      expect(src, rel).not.toMatch(
        /primary=\{\s*<>\s*\{upNextPosition[\s\S]*?<ViewActionsMenu/,
      );
    }
  });

  it("desktop rows: query controls left, then Select · ▦ · ⋯ on the right", () => {
    const bookmarks = read("components/markers/MarkersList.tsx");
    const row = bookmarks.slice(
      bookmarks.indexOf("const actionControls"),
      bookmarks.indexOf("const actionControls") + 1200,
    );
    expect(
      ascending(
        orderOf(row, [
          "<SelectModeButton",
          "<CardSizeOptions",
          "<NotesExportMenu",
        ]),
      ),
    ).toBe(true);
    expect(bookmarks).toMatch(
      /<StickyToolbarRow enabled=\{!selection\.selectMode\}>\s*\{queryControls\}\s*<div className="ml-auto flex items-center gap-2">\s*\{actionControls\}/,
    );

    const cbl = read("components/saved-views/CblViewDetail.tsx");
    const cblRow = cbl.slice(
      cbl.indexOf("<StickyToolbarRow enabled={!selection.selectMode}>"),
      cbl.indexOf("</StickyToolbarRow>"),
    );
    expect(
      ascending(
        orderOf(cblRow, [
          "<CblStatsPills",
          'type="search"',
          'className="ml-auto',
          "Scroll to Up Next entry",
          "<SelectModeButton",
          "<CardSizeOptions",
          "<ViewActionsMenu",
        ]),
      ),
    ).toBe(true);

    const filter = read("components/saved-views/FilterViewDetail.tsx");
    const fx = filter.slice(
      filter.indexOf("extraActions={"),
      filter.indexOf("phoneControlsPlacement"),
    );
    expect(
      ascending(orderOf(fx, ["<SelectModeButton", "<CardSizeOptions"])),
    ).toBe(true);

    const coll = read("components/saved-views/CollectionViewDetail.tsx");
    const cx = coll.slice(
      coll.indexOf("extraActions={"),
      coll.indexOf("phoneControlsPlacement"),
    );
    expect(
      ascending(
        orderOf(cx, [
          "<Badge",
          "<CollectionContinueButton",
          "<SelectModeButton",
          "<CardSizeOptions",
        ]),
      ),
    ).toBe(true);

    const rails = read("components/saved-views/PageRails.tsx");
    const rx = rails.slice(
      rails.indexOf("<StickyToolbarRow"),
      rails.indexOf("</StickyToolbarRow>"),
    );
    expect(rx).toMatch(/justify-end/);
    expect(ascending(orderOf(rx, ["<CardSizeOptions", "{pageActions}"]))).toBe(
      true,
    );

    const lib = read("components/library/LibraryGridToolbar.tsx");
    const lx = lib.slice(
      lib.indexOf("<StickyToolbarRow"),
      lib.indexOf("</StickyToolbarRow>"),
    );
    expect(
      ascending(orderOf(lx, ["<SelectModeButton", "<CardSizeOptions"])),
    ).toBe(true);
  });
});
