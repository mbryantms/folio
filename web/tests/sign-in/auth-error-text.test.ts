import { describe, expect, it } from "vitest";

import { authErrorText, REQUEST_NOT_VERIFIED } from "@/lib/auth/error-text";

describe("authErrorText", () => {
  it("explains a refused cross-site check", () => {
    expect(authErrorText("auth.csrf")).toBe(REQUEST_NOT_VERIFIED);
  });

  it("leaves other codes to the page's own wording", () => {
    expect(authErrorText("auth.invalid")).toBeNull();
    expect(authErrorText(null)).toBeNull();
    expect(authErrorText(undefined)).toBeNull();
  });
});
