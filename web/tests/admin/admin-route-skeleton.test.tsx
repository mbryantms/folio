/**
 * `(admin)` route fallback: the pathname picks the entered page's header
 * + body skeleton (folio-flair pre-merge polish).
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

let pathname = "/admin";
let search = "";
vi.mock("next/navigation", () => ({
  usePathname: () => pathname,
  useSearchParams: () => new URLSearchParams(search),
}));

import {
  AdminRouteSkeleton,
  adminSubpath,
} from "@/components/skeletons/AdminRouteSkeleton";

const ROUTES = [
  "/admin",
  "/admin/activity",
  "/admin/api-docs",
  "/admin/auth",
  "/admin/background-work",
  "/admin/duplicates",
  "/admin/email",
  "/admin/findings",
  "/admin/libraries",
  "/admin/libraries/image",
  "/admin/libraries/image/settings",
  "/admin/libraries/image/health",
  "/admin/libraries/image/duplicates",
  "/admin/libraries/image/removed",
  "/admin/libraries/image/scan",
  "/admin/metadata",
  "/admin/relationships",
  "/admin/scans",
  "/admin/series/the-walking-dead",
  "/admin/server",
  "/admin/starter-lists",
  "/admin/stats",
  "/admin/users",
  "/admin/users/test",
];

describe("adminSubpath", () => {
  it("strips the admin root and an optional locale prefix", () => {
    expect(adminSubpath("/admin")).toBe("");
    expect(adminSubpath("/admin/")).toBe("");
    expect(adminSubpath("/en/admin/users/x")).toBe("/users/x");
    expect(adminSubpath("/admin/libraries/a/health")).toBe(
      "/libraries/a/health",
    );
  });
});

describe("AdminRouteSkeleton", () => {
  it.each(ROUTES)("renders a busy, labelled shape for %s", (route) => {
    pathname = route;
    const html = renderToStaticMarkup(createElement(AdminRouteSkeleton));
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('role="status"');
    // The vendored <Skeleton>; globals.css gives its slot the shared sweep
    // (tests/ui/vendored-slot-styles.test.ts).
    expect(html).toContain('data-slot="skeleton"');
  });

  it("draws the library chrome (no PageHeader) under a library", () => {
    pathname = "/admin/libraries/image/scan";
    const html = renderToStaticMarkup(createElement(AdminRouteSkeleton));
    // The library layout's tab strip: six underline tabs.
    expect(html.match(/border-b-2 px-3 py-2/g)?.length).toBe(6);
  });

  it.each([
    ["/admin/activity", "view=log"],
    ["/admin/background-work", "view=jobs"],
  ])("draws the %s ?%s view's own body", (route, query) => {
    pathname = route;
    search = "";
    const base = renderToStaticMarkup(createElement(AdminRouteSkeleton));
    search = query;
    const view = renderToStaticMarkup(createElement(AdminRouteSkeleton));
    search = "";
    expect(view).toContain('aria-busy="true"');
    expect(view).not.toBe(base);
  });
});
