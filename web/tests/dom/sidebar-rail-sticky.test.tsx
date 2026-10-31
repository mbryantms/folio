// @vitest-environment jsdom
/**
 * The desktop rail is `sticky`, not the vendored Sidebar's `fixed`. iOS
 * pins a fixed box to the viewport during the rubber-band bounce, so the
 * rail stood still while the sticky topbar and the page slid down over
 * it (iPad, 2026-10-30). Sticky keeps it in the scrolling document, as
 * the pre-#1346 `<aside>` was, so the whole shell bounces as one.
 */
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AppSidebar, SidebarProvider } from "@/components/folio/sidebar";

describe("AppSidebar rail positioning", () => {
  it("is sticky under the topbar, never fixed", () => {
    const { container } = render(
      <SidebarProvider defaultOpen>
        <AppSidebar aria-label="Library sidebar">
          <div>nav</div>
        </AppSidebar>
      </SidebarProvider>,
    );
    const rail = container.querySelector('[data-slot="sidebar-container"]');
    expect(rail).not.toBeNull();
    const cls = rail!.className.split(/\s+/);
    expect(cls).toContain("sticky");
    expect(cls).not.toContain("fixed");
    expect(cls).toContain("top-(--topbar-h)");
    expect(cls).toContain("h-[calc(100svh-var(--topbar-h))]");
  });
});
