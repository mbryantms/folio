// @vitest-environment jsdom
/**
 * The admin "Back to library" arrow must work on a collapsed desktop rail.
 * The vendored SidebarGroupLabel hides itself there with `opacity-0` and
 * `-mt-8`, which slides it up over the row above; the admin title label
 * follows the back link, so the invisible label sat on top of the arrow
 * and swallowed every click (2026-11-27). The Folio wrapper's label is
 * click-through while collapsed. jsdom has no layout, so this pins the
 * structure the CSS acts on: a collapsed rail, a real link to "/", and
 * every group label carrying the collapsed `pointer-events-none`.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/users",
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
}));
vi.mock("@/lib/api/queries", () => ({ useMe: () => ({ data: undefined }) }));
vi.mock("@/components/shell/UserFooter", () => ({ UserFooter: () => null }));
vi.mock("@/components/admin/MetadataUnmatchedBadge", () => ({
  MetadataUnmatchedBadge: () => null,
}));

import { AdminSidebar } from "@/components/admin/AdminSidebar";
import { AppSidebar, AppSidebarProvider } from "@/components/folio/sidebar";

const user = { display_name: "Admin", email: "a@example.com", role: "admin" };

function renderRail(open: boolean) {
  return render(
    <AppSidebarProvider defaultOpen={open}>
      <AppSidebar aria-label="Admin sidebar">
        <AdminSidebar
          title="Admin"
          user={user}
          sections={[
            {
              label: "People",
              items: [{ href: "/admin/users", label: "Users", icon: "Users" }],
            },
          ]}
        />
      </AppSidebar>
    </AppSidebarProvider>,
  );
}

describe("AdminSidebar back to library", () => {
  it.each([
    ["collapsed", false],
    ["expanded", true],
  ])("links to the library when %s", (state, open) => {
    const { container } = renderRail(open);
    expect(
      container
        .querySelector('[data-slot="sidebar"][data-state]')
        ?.getAttribute("data-state"),
    ).toBe(state);
    const back = screen.getByRole("link", { name: /back to library/i });
    expect(back.getAttribute("href")).toBe("/");
  });

  it("no hidden group label can cover the back link when collapsed", () => {
    const { container } = renderRail(false);
    const back = screen.getByRole("link", { name: /back to library/i });
    const labels = Array.from(
      container.querySelectorAll('[data-slot="sidebar-group-label"]'),
    );
    expect(labels.length).toBeGreaterThan(0);
    // The admin title label is the one that sits right after the link.
    expect(
      labels.some(
        (l) =>
          back.compareDocumentPosition(l) & Node.DOCUMENT_POSITION_FOLLOWING,
      ),
    ).toBe(true);
    for (const label of labels) {
      expect(label.className.split(/\s+/)).toContain(
        "group-data-[collapsible=icon]:pointer-events-none",
      );
    }
  });
});
