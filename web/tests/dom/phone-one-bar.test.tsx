// @vitest-environment jsdom
/**
 * Phone chrome (docs/dev/phone-layout.md, rule 0): below `md` the library
 * shell has no topbar, the page's control row is the top bar, and global
 * search is a bottom tab. Before this, every phone page stacked the 16px
 * "Folio" bar over a 20px page title with a magnifier in each, and both
 * bands were sticky (2026-10-30).
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const nav = vi.hoisted(() => ({ pathname: "/", search: "" }));
vi.mock("next/navigation", () => ({
  usePathname: () => nav.pathname,
  useSearchParams: () => new URLSearchParams(nav.search),
}));

import { BottomTabBar } from "@/components/library/BottomTabBar";

const read = (rel: string) => readFileSync(join(process.cwd(), rel), "utf8");

describe("phone one-bar shell", () => {
  it("the library topbar renders from md up only and marks the shell", () => {
    const src = read("components/library/MainShell.tsx");
    const header = src.slice(src.indexOf("<header"), src.indexOf("</header>"));
    expect(header).toMatch(/\bhidden\b/);
    expect(header).toMatch(/\bmd:flex\b/);
    expect(src).toContain('data-phone-topbar="hidden"');
    // The page column clears the status bar itself now that no bar does.
    expect(src).toMatch(
      /SidebarInset[\s\S]*?pt-\[calc\(var\(--safe-top\)\+1\.5rem\)\]/,
    );
  });

  it("a bar that is the page's first element sits at the status bar", () => {
    // `pageTop` cancels the column's 1.5rem page padding, not the
    // status-bar inset: the bar starts exactly where it pins.
    const bar = read("components/library/PhoneControlsBar.tsx");
    expect(bar).toMatch(/pageTop && "-mt-6"/);
    expect(bar).not.toMatch(/-mt-\[calc\(var\(--safe-top\)/);
    for (const page of [
      "components/saved-views/PageRails.tsx",
      "components/markers/MarkersList.tsx",
    ]) {
      expect(read(page), page).toMatch(/<PhoneControlsBar\s+pageTop\b/);
    }
    // A bar below a title block keeps the padding above the title.
    expect(read("components/library/LibraryGridView.tsx")).not.toMatch(
      /<PhoneControlsBar[\s\S]{0,200}?pageTop/,
    );
  });

  it("globals.css collapses --topbar-h to the status-bar inset inside the shell", () => {
    const css = read("styles/globals.css");
    expect(css).toMatch(
      /@media \(width < 48rem\) \{\s*\[data-phone-topbar="hidden"\] \{\s*--topbar-h: var\(--safe-top\);/,
    );
  });

  it("the bottom tab bar carries global search and marks it active", () => {
    nav.pathname = "/search";
    render(<BottomTabBar onMore={() => {}} />);
    const tabs = screen.getAllByRole("link");
    expect(tabs.map((t) => t.textContent)).toEqual([
      "Home",
      "Library",
      "Search",
      "Bookmarks",
    ]);
    const search = screen.getByRole("link", { name: "Search" });
    expect(search.getAttribute("href")).toBe("/search");
    expect(search.getAttribute("aria-current")).toBe("page");
    expect(
      screen.getByRole("link", { name: "Home" }).getAttribute("aria-current"),
    ).toBeNull();
  });
});
