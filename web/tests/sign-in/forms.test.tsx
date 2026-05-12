/**
 * Defensive snapshot: every credential-bearing `<form>` ships with a real
 * `method="POST"` + `action` attribute. M9 was triggered because these
 * forms used to ship with neither, so the browser's default form handler
 * (GET + current URL) leaked `?email=&password=` into the address bar
 * whenever JS hadn't hydrated by the time the user pressed Enter.
 *
 * The progressive-enhancement contract: JS still intercepts via
 * `preventDefault()` (covered by the runtime tests); these assertions
 * cover the *fallback*. Removing `method` or `action` from any of these
 * forms is the failure mode this test exists to catch.
 *
 * Implementation note: vitest runs in Node (no DOM), so we render to a
 * static HTML string via `renderToStaticMarkup` and grep the markup
 * rather than mounting through React Testing Library.
 */

import React from "react";
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  redirect: vi.fn(),
}));
vi.mock("next/link", () => ({
  default: ({
    children,
    href,
  }: {
    children: React.ReactNode;
    href: string;
  }) => React.createElement("a", { href }, children),
}));
// Radix Tabs only renders the active TabsContent. For this snapshot we
// want both the login *and* register form HTML in the output so we can
// assert both have method/action. Replace each Tabs primitive with a
// transparent fragment that renders all children.
vi.mock("@/components/ui/tabs", () => {
  const passthrough = ({ children }: { children: React.ReactNode }) =>
    React.createElement(React.Fragment, null, children);
  return {
    Tabs: passthrough,
    TabsList: passthrough,
    TabsTrigger: passthrough,
    TabsContent: passthrough,
  };
});

import { SignInClient } from "@/app/[locale]/sign-in/SignInClient";
import { ForgotPasswordForm } from "@/app/[locale]/forgot-password/ForgotPasswordForm";
import { ResetPasswordForm } from "@/app/[locale]/reset-password/ResetPasswordForm";

function assertPostFormPresent(html: string, action: string) {
  const re = new RegExp(
    `<form[^>]*\\bmethod=\"POST\"[^>]*\\baction=\"${action.replace(/\//g, "\\/")}\"|` +
      `<form[^>]*\\baction=\"${action.replace(/\//g, "\\/")}\"[^>]*\\bmethod=\"POST\"`,
    "i",
  );
  expect(re.test(html), `expected POST form with action="${action}"`).toBe(
    true,
  );
}

describe("credential forms — progressive enhancement", () => {
  it("sign-in login form ships method=POST + real action", () => {
    const html = renderToStaticMarkup(
      React.createElement(SignInClient, {
        config: {
          auth_mode: "local",
          oidc_enabled: false,
          registration_open: true,
        },
        next: null,
        banner: null,
        errorMessage: null,
      }),
    );
    assertPostFormPresent(html, "/api/auth/local/login");
    assertPostFormPresent(html, "/api/auth/local/register");
  });

  it("forgot-password ships method=POST + real action", () => {
    const html = renderToStaticMarkup(
      React.createElement(ForgotPasswordForm),
    );
    assertPostFormPresent(html, "/api/auth/local/request-password-reset");
  });

  it("reset-password ships method=POST + real action + hidden token", () => {
    const html = renderToStaticMarkup(
      React.createElement(ResetPasswordForm, { token: "abc.def.ghi" }),
    );
    assertPostFormPresent(html, "/api/auth/local/reset-password");
    expect(
      /<input[^>]*\btype="hidden"[^>]*\bname="token"[^>]*\bvalue="abc\.def\.ghi"/i.test(
        html,
      ) ||
        /<input[^>]*\bname="token"[^>]*\btype="hidden"[^>]*\bvalue="abc\.def\.ghi"/i.test(
          html,
        ),
    ).toBe(true);
  });
});
