// @vitest-environment jsdom
/**
 * Root-layout listeners must not probe `/auth/me` on the anonymous auth
 * pages: there it only 401s, and the 401 runs the refresh dance
 * (`POST /auth/refresh` → 403) on every sign-in visit.
 */
import * as React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

const nav = vi.hoisted(() => ({ pathname: "/sign-in" }));
const useMe = vi.hoisted(() =>
  vi.fn((_opts?: { enabled?: boolean }) => ({ data: undefined })),
);
vi.mock("next/navigation", () => ({
  usePathname: () => nav.pathname,
  useRouter: () => ({ push: vi.fn() }),
}));
vi.mock("@/lib/api/queries", () => ({ useMe }));
vi.mock("@/lib/search/use-search-modal", () => ({
  useSearchModal: () => ({ setOpen: vi.fn() }),
}));
vi.mock("@/lib/api/scan-events", () => ({ useScanEvents: vi.fn() }));

import { GlobalHotkeys } from "@/components/GlobalHotkeys";
import { GlobalShortcutsSheet } from "@/components/GlobalShortcutsSheet";
import { ScanResultListener } from "@/components/ScanResultListener";
import { isAuthSurface } from "@/lib/auth-surface";

function renderListeners() {
  render(
    <GlobalShortcutsSheet>
      <GlobalHotkeys />
      <ScanResultListener />
    </GlobalShortcutsSheet>,
  );
}

afterEach(() => useMe.mockClear());

describe("root-layout useMe probes", () => {
  it.each(["/sign-in", "/forgot-password", "/reset-password"])(
    "are disabled on %s",
    (pathname) => {
      nav.pathname = pathname;
      renderListeners();
      expect(useMe).toHaveBeenCalled();
      for (const [opts] of useMe.mock.calls)
        expect(opts).toEqual({ enabled: false });
    },
  );

  it("run on signed-in pages", () => {
    nav.pathname = "/library";
    renderListeners();
    for (const [opts] of useMe.mock.calls)
      expect(opts).toEqual({ enabled: true });
  });
});

describe("isAuthSurface", () => {
  it("matches the auth routes and their subpaths only", () => {
    expect(isAuthSurface("/sign-in")).toBe(true);
    expect(isAuthSurface("/reset-password/abc")).toBe(true);
    expect(isAuthSurface("/sign-in-help")).toBe(false);
    expect(isAuthSurface("/library")).toBe(false);
    expect(isAuthSurface(null)).toBe(false);
  });
});
