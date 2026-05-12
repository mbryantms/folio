import { afterEach, describe, expect, it, vi } from "vitest";

// Stub the AdminShell + nav so we don't pull a client-component tree (lucide,
// react) into a node-env test. We only care about the layout's ACL logic.
vi.mock("@/components/admin/AdminShell", () => ({
  AdminShell: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("@/components/admin/nav", () => ({
  adminNav: () => [],
  settingsNav: () => [],
}));

const redirected: { url: string }[] = [];
class RedirectError extends Error {
  constructor(public url: string) {
    super(`NEXT_REDIRECT:${url}`);
  }
}
vi.mock("next/navigation", () => ({
  redirect: (url: string) => {
    redirected.push({ url });
    throw new RedirectError(url);
  },
}));

// next/headers needs a request scope at runtime, but the layouts only use
// it for the SSR sidebar-state cookie. Stub a no-op cookie store so the
// layout's `parseSidebarState(undefined)` falls through to the default.
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: () => undefined,
  }),
}));

const apiGet = vi.fn();
class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
vi.mock("@/lib/api/fetch", () => ({
  apiGet: (path: string) => apiGet(path),
  ApiError,
}));

afterEach(() => {
  redirected.length = 0;
  apiGet.mockReset();
});

async function runLayout(
  importPath: string,
): Promise<{ redirect: string | null; rendered: boolean }> {
  const mod = await import(importPath);
  try {
    await mod.default({
      children: null,
    });
    return { redirect: null, rendered: true };
  } catch (e) {
    if (e instanceof RedirectError) {
      return { redirect: e.url, rendered: false };
    }
    throw e;
  }
}

describe("(admin) layout", () => {
  it("redirects to sign-in when /auth/me returns 401", async () => {
    apiGet.mockRejectedValue(new ApiError(401, "unauthenticated"));
    const r = await runLayout("@/app/[locale]/(admin)/layout");
    expect(r.redirect).toBe("/sign-in");
  });

  it("redirects non-admin users to home", async () => {
    apiGet.mockResolvedValue({
      id: "u1",
      email: "u@example.com",
      display_name: "User",
      role: "user",
      csrf_token: "t",
    });
    const r = await runLayout("@/app/[locale]/(admin)/layout");
    expect(r.redirect).toBe("/");
  });

  it("renders the shell for admin users", async () => {
    apiGet.mockResolvedValue({
      id: "u1",
      email: "admin@example.com",
      display_name: "Admin",
      role: "admin",
      csrf_token: "t",
    });
    const r = await runLayout("@/app/[locale]/(admin)/layout");
    expect(r.redirect).toBeNull();
    expect(r.rendered).toBe(true);
  });
});

describe("(settings) layout", () => {
  it("redirects to sign-in when unauthenticated", async () => {
    apiGet.mockRejectedValue(new ApiError(401, "unauthenticated"));
    const r = await runLayout("@/app/[locale]/(settings)/layout");
    expect(r.redirect).toBe("/sign-in");
  });

  it("renders for authenticated users regardless of role", async () => {
    apiGet.mockResolvedValue({
      id: "u1",
      email: "u@example.com",
      display_name: "User",
      role: "user",
      csrf_token: "t",
    });
    const r = await runLayout("@/app/[locale]/(settings)/layout");
    expect(r.redirect).toBeNull();
    expect(r.rendered).toBe(true);
  });
});
