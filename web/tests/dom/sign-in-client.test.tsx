// @vitest-environment jsdom
/**
 * Render + interaction tests for the sign-in / register card. Unlike the
 * static-markup tests in tests/sign-in/, these hydrate the real component
 * in jsdom so zod validation, react-hook-form state, fetch submission and
 * router navigation are all exercised the way a browser would.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

const push = vi.fn();
const refresh = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh }),
  redirect: vi.fn(),
}));
vi.mock("next/link", () => ({
  default: ({ children, href }: { children: React.ReactNode; href: string }) =>
    React.createElement("a", { href }, children),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    info: vi.fn(),
    error: vi.fn(),
    success: vi.fn(),
  }),
}));

import { SignInClient } from "@/app/[locale]/sign-in/SignInClient";

const localConfig = {
  auth_mode: "local",
  oidc_enabled: false,
  registration_open: true,
  password_recovery_enabled: false,
} as const;

function renderSignIn(
  overrides: Partial<React.ComponentProps<typeof SignInClient>> = {},
) {
  return render(
    <SignInClient
      config={localConfig as never}
      next={null}
      banner={null}
      errorMessage={null}
      {...overrides}
    />,
  );
}

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => {
  vi.unstubAllGlobals();
  fetchMock.mockReset();
  push.mockReset();
  refresh.mockReset();
});

function submitButton(name: RegExp) {
  return screen
    .getAllByRole("button", { name })
    .find(
      (b) => (b as HTMLButtonElement).type === "submit",
    ) as HTMLButtonElement;
}

describe("SignInClient (jsdom)", () => {
  it("blocks an empty login submit with zod messages and never calls fetch", async () => {
    renderSignIn();
    fireEvent.click(submitButton(/^sign in$/i));
    await waitFor(() => {
      expect(screen.getByText("Enter a valid email")).toBeTruthy();
      expect(screen.getByText("Required")).toBeTruthy();
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("posts credentials to /auth/local/login and navigates on success", async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({}),
    });
    renderSignIn({ next: "/library" });
    fireEvent.change(screen.getByLabelText("Email"), {
      target: { value: "a@b.com" },
    });
    fireEvent.change(screen.getByLabelText("Password"), {
      target: { value: "pw" },
    });
    fireEvent.click(submitButton(/^sign in$/i));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/auth/local/login");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("include");
    expect(JSON.parse(init.body as string)).toEqual({
      email: "a@b.com",
      password: "pw",
    });
    await waitFor(() => expect(push).toHaveBeenCalledWith("/library"));
    expect(refresh).toHaveBeenCalled();
  });

  it("surfaces a failed login inline as role=alert, not a toast", async () => {
    fetchMock.mockResolvedValue({
      ok: false,
      status: 401,
      json: async () => ({ error: { message: "Invalid credentials" } }),
    });
    renderSignIn();
    fireEvent.change(screen.getByLabelText("Email"), {
      target: { value: "a@b.com" },
    });
    fireEvent.change(screen.getByLabelText("Password"), {
      target: { value: "pw" },
    });
    fireEvent.click(submitButton(/^sign in$/i));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Invalid credentials");
    expect(push).not.toHaveBeenCalled();
  });

  it("register tab enforces the 12-character password rule client-side", async () => {
    renderSignIn();
    // Radix Tabs activate on pointer-down, not click.
    fireEvent.mouseDown(screen.getByRole("tab", { name: /register/i }), {
      button: 0,
    });
    expect(
      await screen.findByText("Must be at least 12 characters."),
    ).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Email"), {
      target: { value: "new@b.com" },
    });
    fireEvent.change(screen.getByLabelText("Password"), {
      target: { value: "short" },
    });
    fireEvent.click(submitButton(/create account|register/i));
    await waitFor(() =>
      expect(screen.getByText("Must be at least 12 characters")).toBeTruthy(),
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("prop-driven branches: closed registration, SSO-only, SSO button href, banners", () => {
    const { unmount } = renderSignIn({
      config: { ...localConfig, registration_open: false } as never,
    });
    expect(
      (screen.getByRole("tab", { name: /register/i }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    unmount();

    const sso = renderSignIn({
      config: {
        ...localConfig,
        auth_mode: "oidc",
        oidc_enabled: true,
      } as never,
      next: "/library",
    });
    expect(screen.queryByRole("tab")).toBeNull();
    expect(screen.getByText(/uses SSO only/i)).toBeTruthy();
    const link = screen.getByRole("link", {
      name: /sign in with sso/i,
    }) as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe(
      "/auth/oidc/start?redirect_after=%2Flibrary",
    );
    sso.unmount();

    renderSignIn({ banner: "verified" });
    expect(screen.getByRole("status").textContent).toContain("Email verified");
  });
});
