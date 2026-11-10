// @vitest-environment jsdom
/**
 * The sign-in page's silent session resume (`shouldResumeSession`): when
 * it may try, and the loop guard that stops a resumed-but-still-bounced
 * session — or a failed one — from trying again and again.
 */
import React from "react";
import { render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  redirect: vi.fn(),
}));
vi.mock("next/link", () => ({
  default: ({ children, href }: { children: React.ReactNode; href: string }) =>
    React.createElement("a", { href }, children),
}));
const resumeSession = vi.fn(async () => false);
vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () =>
    /(?:^|;\s*)__Host-comic_csrf=([^;]+)/.exec(document.cookie)?.[1] ?? null,
  resumeSession: () => resumeSession(),
}));

const { SignInClient, shouldResumeSession, resetResumeGuardForTests } =
  await import("@/app/[locale]/sign-in/SignInClient");

const jar = new Map<string, string>();
Object.defineProperty(document, "cookie", {
  configurable: true,
  get: () => [...jar].map(([k, v]) => `${k}=${v}`).join("; "),
  set: () => {},
});
const RESUME_KEY = "folio:session-resume-at";
const config = {
  auth_mode: "local",
  oidc_enabled: false,
  registration_open: true,
  password_recovery_enabled: false,
  oidc_auto_redirect: false,
} as const;

beforeEach(() => {
  jar.set("__Host-comic_csrf", "c");
  window.sessionStorage.clear();
  window.history.replaceState(null, "", "/sign-in");
  resetResumeGuardForTests();
  resumeSession.mockReset();
});
afterEach(() => {
  jar.clear();
  vi.restoreAllMocks();
});

describe("shouldResumeSession", () => {
  it("tries when a session may exist and nothing rules it out", () => {
    expect(shouldResumeSession(null, null)).toBe(true);
  });

  it("never without a CSRF cookie", () => {
    jar.clear();
    expect(shouldResumeSession(null, null)).toBe(false);
  });

  it("never after a sign-out", () => {
    window.history.replaceState(null, "", "/sign-in?signed_out=1");
    expect(shouldResumeSession(null, null)).toBe(false);
  });

  it("never when the page carries a banner or an error", () => {
    expect(shouldResumeSession("verified", null)).toBe(false);
    expect(shouldResumeSession(null, "auth.invalid")).toBe(false);
  });

  it("not again within the guard window after a successful resume", async () => {
    resumeSession.mockResolvedValue(true);
    render(
      <SignInClient
        config={config}
        next={null}
        banner={null}
        errorMessage={null}
      />,
    );
    await waitFor(() => expect(resumeSession).toHaveBeenCalledTimes(1));
    expect(window.sessionStorage.getItem(RESUME_KEY)).not.toBeNull();
    expect(shouldResumeSession(null, null)).toBe(false);
  });

  it("not again within the guard window after a failed resume", async () => {
    resumeSession.mockResolvedValue(false);
    const view = render(
      <SignInClient
        config={config}
        next={null}
        banner={null}
        errorMessage={null}
      />,
    );
    await waitFor(() => expect(resumeSession).toHaveBeenCalledTimes(1));
    // The form shows after the failure.
    await waitFor(() =>
      expect(view.container.querySelector("form")).not.toBeNull(),
    );
    expect(window.sessionStorage.getItem(RESUME_KEY)).not.toBeNull();
    expect(shouldResumeSession(null, null)).toBe(false);
  });

  it("at most once per page load without sessionStorage", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    resumeSession.mockResolvedValue(false);
    render(
      <SignInClient
        config={config}
        next={null}
        banner={null}
        errorMessage={null}
      />,
    );
    await waitFor(() => expect(resumeSession).toHaveBeenCalledTimes(1));
    expect(shouldResumeSession(null, null)).toBe(false);
  });

  it("the server-rendered first paint is the resume card when a session may exist", async () => {
    const { renderToString } = await import("react-dom/server");
    const html = renderToString(
      <SignInClient
        config={config}
        next={null}
        banner={null}
        errorMessage={null}
        mayResume
      />,
    );
    expect(html).toContain("Restoring your session");
    expect(html).not.toContain("<form");
    const plain = renderToString(
      <SignInClient
        config={config}
        next={null}
        banner={null}
        errorMessage={null}
      />,
    );
    expect(plain).toContain("<form");
  });
});
