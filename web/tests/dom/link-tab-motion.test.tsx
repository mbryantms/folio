// @vitest-environment jsdom
/**
 * Link-tab navs (reading log, admin library sections) share the Radix tab
 * strips' sliding underline, and the route under them enters from the
 * side its tab sits on (folio-flair pre-merge polish, 2026-11-04).
 */
import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

let pathname = "/log";
vi.mock("next/navigation", () => ({ usePathname: () => pathname }));

import { RouteTabPanel } from "@/components/folio/route-tab-panel";
import { LogChrome, LogPanel } from "@/components/log/LogChrome";

describe("link-tab motion", () => {
  it("draws one sliding underline and hides the links' own", () => {
    pathname = "/log/stats";
    const { container } = render(<LogChrome />);
    const strip = container.querySelector("ul")!;
    expect(strip.dataset.indicator).toBe("");
    expect(strip.querySelectorAll("li[aria-hidden]")).toHaveLength(1);
    const stats = screen.getByRole("link", { name: "Stats" });
    expect(stats.getAttribute("aria-current")).toBe("page");
    expect(stats.className).toContain(
      "group-data-[indicator]/strip:after:opacity-0",
    );
  });

  it("enters forward or back by tab index, not on first render", () => {
    const { container, rerender } = render(
      <RouteTabPanel index={1}>a</RouteTabPanel>,
    );
    const panel = container.firstElementChild as HTMLElement;
    expect(panel.classList.contains("route-tab-in")).toBe(false);
    rerender(<RouteTabPanel index={3}>b</RouteTabPanel>);
    expect(panel.classList.contains("route-tab-in")).toBe(true);
    expect(panel.dataset.routeTabMotion).toBe("fwd");
    rerender(<RouteTabPanel index={0}>c</RouteTabPanel>);
    expect(panel.dataset.routeTabMotion).toBe("back");
  });

  it("follows the log's active tab", () => {
    pathname = "/log";
    const { container, rerender } = render(<LogPanel>x</LogPanel>);
    pathname = "/log/sessions";
    act(() => rerender(<LogPanel>y</LogPanel>));
    const panel = container.firstElementChild as HTMLElement;
    expect(panel.dataset.routeTabMotion).toBe("fwd");
  });
});
