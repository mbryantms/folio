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
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const nav = vi.hoisted(() => ({ pathname: "/", search: "" }));
vi.mock("next/navigation", () => ({
  usePathname: () => nav.pathname,
  useSearchParams: () => new URLSearchParams(nav.search),
}));
const searchModal = vi.hoisted(() => ({ open: false, setOpen: vi.fn() }));
vi.mock("@/lib/search/use-search-modal", () => ({
  useSearchModal: () => searchModal,
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

  it("the bottom tab bar's Search tab opens the search sheet", () => {
    nav.pathname = "/";
    searchModal.open = false;
    render(<BottomTabBar onMore={() => {}} />);
    expect(screen.getAllByRole("link").map((t) => t.textContent)).toEqual([
      "Home",
      "Library",
      "Bookmarks",
    ]);
    const search = screen.getByRole("button", { name: "Search" });
    expect(search.getAttribute("aria-haspopup")).toBe("dialog");
    expect(search.getAttribute("aria-expanded")).toBe("false");
    expect(search.className).toMatch(/\btext-muted-foreground\b/);
    fireEvent.click(search);
    expect(searchModal.setOpen).toHaveBeenCalledWith(true);
  });

  it("the Search tab reads active while the sheet is open and on /search", () => {
    searchModal.open = true;
    const { unmount } = render(<BottomTabBar onMore={() => {}} />);
    let search = screen.getByRole("button", { name: "Search" });
    expect(search.getAttribute("aria-expanded")).toBe("true");
    expect(search.className).toMatch(/\btext-foreground\b/);
    unmount();
    searchModal.open = false;
    nav.pathname = "/search";
    render(<BottomTabBar onMore={() => {}} />);
    search = screen.getByRole("button", { name: "Search" });
    expect(search.getAttribute("aria-current")).toBe("page");
    expect(
      screen.getByRole("link", { name: "Home" }).getAttribute("aria-current"),
    ).toBeNull();
  });

  it("the search modal is a top sheet on phones with no corner close button", () => {
    const src = read("components/SearchModal.tsx");
    // The dialog's absolute corner X overlaid the input's right end at
    // every width; the phone closes from a Cancel beside the input.
    expect(src).toMatch(/<DialogContent\s+showCloseButton=\{false\}/);
    // globals.css makes every dialog a bottom sheet on phones (doubled
    // `data-slot` selector); the search sheet opts into the top edge
    // with the same specificity, so the input never moves with the
    // result count and the keyboard has the bottom of the screen.
    expect(src).toMatch(/data-search-sheet=""/);
    const css = read("styles/globals.css");
    const rule = css.slice(
      css.indexOf('[data-slot="dialog-content"][data-search-sheet]'),
    );
    expect(rule).toMatch(/^[^}]*inset: 0 0 auto 0;/);
    expect(rule).toMatch(/^[^}]*max-height: var\(--visual-height, 100dvh\);/);
    expect(rule).toMatch(/^[^}]*padding-top: var\(--safe-top\);/);
    expect(rule).toMatch(/^[^}]*--tw-enter-translate-y: -100%;/);
    expect(
      css.indexOf('[data-slot="dialog-content"][data-search-sheet]'),
    ).toBeGreaterThan(
      css.indexOf('[data-slot="dialog-content"][data-slot="dialog-content"]'),
    );
    const cancel = src.slice(src.indexOf("<DialogClose asChild>"));
    expect(cancel).toMatch(
      /className="[^"]*\bsm:hidden\b[^"]*"[\s\S]{0,40}Cancel/,
    );
    // The bottom bar warms the chunk so the first tap opens in-gesture.
    expect(read("components/library/BottomTabBar.tsx")).toMatch(
      /void import\("@\/components\/SearchModal"\)/,
    );
  });
});
