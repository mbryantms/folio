// @vitest-environment jsdom
/**
 * Admin tablet layout: between `md` and `lg` the docked rail is hidden and
 * the same nav opens as a left sheet from a "Menu" button (the phone
 * pattern); at `lg` and up the rail stays. jsdom has no real media
 * queries, so these assert the class structure the CSS acts on, plus the
 * sheet's behaviour with `matchMedia` answering for the tablet band.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/users",
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
}));
vi.mock("@/lib/api/queries", () => ({ useMe: () => ({ data: undefined }) }));
vi.mock("@/components/admin/AdminSidebar", () => ({
  AdminSidebar: ({ title }: { title: string }) => (
    <nav aria-label={`${title} navigation`}>
      <a href="/admin/users">Users</a>
    </nav>
  ),
}));
vi.mock("@/components/TopbarSearchTrigger", () => ({
  TopbarSearchTrigger: () => null,
}));
vi.mock("@/components/PullToRefresh", () => ({ PullToRefresh: () => null }));
vi.mock("@/components/admin/ScanEventBeacon", () => ({
  ScanEventBeacon: () => null,
}));
vi.mock("@/components/admin/RestartPendingBanner", () => ({
  RestartPendingBanner: () => null,
}));

import {
  AdminShell,
  TABLET_NAV_QUERY,
  TABLET_RAIL_HIDDEN_CLASS,
} from "@/components/admin/AdminShell";

const user = { display_name: "Admin", email: "a@example.com", role: "admin" };

function renderShell(tabletSheet: boolean) {
  return render(
    <AdminShell
      user={user}
      sections={[]}
      title="Admin"
      homeHref="/"
      tabletSheet={tabletSheet}
    >
      <p>page</p>
    </AdminShell>,
  );
}

const originalMatchMedia = window.matchMedia;
function matchTablet() {
  window.matchMedia = ((query: string) => ({
    matches: query === TABLET_NAV_QUERY,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
}
afterEach(() => {
  window.matchMedia = originalMatchMedia;
});

describe("AdminShell tablet sheet", () => {
  it("the query and the hide class cover the same md..lg band", () => {
    expect(TABLET_NAV_QUERY).toBe(
      "(min-width: 48rem) and (max-width: 63.99rem)",
    );
    expect(TABLET_RAIL_HIDDEN_CLASS).toMatch(/^md:max-lg:/);
  });

  it("hides the docked rail and shows a Menu trigger in the tablet band", () => {
    const { container } = renderShell(true);
    const wrapper = container.querySelector('[data-slot="sidebar-wrapper"]')!;
    expect(wrapper.className.split(/\s+/)).toContain(TABLET_RAIL_HIDDEN_CLASS);
    // The rail is still rendered (it is the desktop nav at lg+).
    expect(
      wrapper.querySelector('[data-slot="sidebar-container"]'),
    ).not.toBeNull();
    // Rail toggle hides in the band; the Menu trigger shows only there.
    const toggle = screen.getByRole("button", { name: "Toggle navigation" });
    expect(toggle.className.split(/\s+/)).toContain("md:max-lg:hidden");
    const menu = screen.getByTestId("admin-tablet-nav-trigger");
    const cls = menu.className.split(/\s+/);
    expect(cls).toContain("hidden");
    expect(cls).toContain("md:max-lg:inline-flex");
    // Visible text is the whole accessible name (label-in-name).
    expect(menu.getAttribute("aria-label")).toBeNull();
    expect(menu.textContent).toBe("Menu");
    expect(menu.getAttribute("data-slot")).toBe("button");
  });

  it("keeps the docked rail and no Menu trigger without tabletSheet", () => {
    const { container } = renderShell(false);
    const wrapper = container.querySelector('[data-slot="sidebar-wrapper"]')!;
    expect(wrapper.className).not.toContain("md:max-lg:");
    expect(
      wrapper.querySelector('[data-slot="sidebar-container"]'),
    ).not.toBeNull();
    expect(screen.queryByTestId("admin-tablet-nav-trigger")).toBeNull();
    const toggle = screen.getByRole("button", { name: "Toggle navigation" });
    expect(toggle.className).not.toContain("md:max-lg:hidden");
  });

  it("opens the nav sheet from Menu in the band and closes on a link", () => {
    matchTablet();
    renderShell(true);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Menu" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("data-mobile")).toBe("true");
    expect(dialog.getAttribute("data-side")).toBe("left");
    fireEvent.click(screen.getAllByRole("link", { name: "Users" }).at(-1)!);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("the rail toggle opens the sheet in the band instead of collapsing", () => {
    matchTablet();
    const { container } = renderShell(true);
    fireEvent.click(screen.getByRole("button", { name: "Toggle navigation" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    // The provider keeps reporting the rail expanded.
    const rail = container.querySelector('[data-slot="sidebar"]')!;
    expect(rail.getAttribute("data-state")).toBe("expanded");
  });
});
