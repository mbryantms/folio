/**
 * `(admin)` route fallback: the pathname picks the entered page's header
 * + body skeleton (folio-flair pre-merge polish).
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

let pathname = "/admin";
vi.mock("next/navigation", () => ({ usePathname: () => pathname }));

import {
  AdminRouteSkeleton,
  adminSubpath,
} from "@/components/skeletons/AdminRouteSkeleton";

const ROUTES = [
  "/admin",
  "/admin/activity",
  "/admin/audit",
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
  "/admin/libraries/image/history",
  "/admin/libraries/image/duplicates",
  "/admin/libraries/image/removed",
  "/admin/libraries/image/scan",
  "/admin/libraries/image/thumbnails",
  "/admin/logs",
  "/admin/metadata",
  "/admin/queue",
  "/admin/relationships",
  "/admin/scan-dashboard",
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
    expect(html).toContain("skeleton-shimmer");
  });

  it("draws the library chrome (no PageHeader) under a library", () => {
    pathname = "/admin/libraries/image/history";
    const html = renderToStaticMarkup(createElement(AdminRouteSkeleton));
    // The library layout's tab strip: seven underline tabs.
    expect(html.match(/border-b-2 px-3 py-2/g)?.length).toBe(7);
  });
});
