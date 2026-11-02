// @vitest-environment jsdom
/**
 * `useMe` must not probe `/auth/me` on the anonymous auth pages: there it
 * only 401s, and the 401 runs the refresh dance (`POST /auth/refresh` →
 * 403) on every sign-in visit. The root-layout listeners call it
 * unconditionally, so the guard lives in the hook.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const nav = vi.hoisted(() => ({ pathname: "/sign-in" }));
vi.mock("next/navigation", () => ({ usePathname: () => nav.pathname }));

import { isAuthSurface, useMe } from "@/lib/api/queries";

const fetchMock = vi.fn();

function Probe({ enabled }: { enabled?: boolean }) {
  useMe(enabled === undefined ? undefined : { enabled });
  return null;
}

function renderProbe(enabled?: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Probe enabled={enabled} />
    </QueryClientProvider>,
  );
}

const fetchedPaths = () =>
  fetchMock.mock.calls.map(
    ([url]) => new URL(String(url), "http://x").pathname,
  );

beforeEach(() => {
  fetchMock.mockResolvedValue(Response.json({ id: "u1" }));
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => {
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});

describe("useMe", () => {
  it.each(["/sign-in", "/forgot-password", "/reset-password/token"])(
    "does not probe /auth/me on %s",
    async (pathname) => {
      nav.pathname = pathname;
      renderProbe();
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it("probes on signed-in pages", async () => {
    nav.pathname = "/library";
    renderProbe();
    await waitFor(() => expect(fetchedPaths()).toContain("/api/auth/me"));
  });

  it("still honours enabled: false", async () => {
    nav.pathname = "/library";
    renderProbe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchMock).not.toHaveBeenCalled();
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
