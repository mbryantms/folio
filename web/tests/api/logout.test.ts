import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { postLogout, SIGNED_OUT_HREF } from "@/lib/api/logout";

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => {
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});

describe("postLogout", () => {
  it("asks for JSON and sends the CSRF token", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await postLogout("tok");
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe("/auth/logout");
    expect(init).toMatchObject({
      method: "POST",
      credentials: "include",
      headers: { Accept: "application/json", "X-CSRF-Token": "tok" },
    });
  });

  it("navigates to the IdP's end-session URL when the server returns one", async () => {
    const target =
      "https://auth.example.com/application/o/folio/end-session/?id_token_hint=x";
    fetchMock.mockResolvedValue(
      Response.json({ redirect_to: target }, { status: 200 }),
    );
    await expect(postLogout(null)).resolves.toBe(target);
  });

  it("lands on the signed-out sign-in page for a local session (204)", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await expect(postLogout(null)).resolves.toBe(SIGNED_OUT_HREF);
  });

  it("ignores a redirect_to that isn't an http(s) URL", async () => {
    fetchMock.mockResolvedValue(
      Response.json({ redirect_to: "javascript:alert(1)" }, { status: 200 }),
    );
    await expect(postLogout(null)).resolves.toBe(SIGNED_OUT_HREF);
  });

  it("reports failure when the server can't be reached or refuses", async () => {
    fetchMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await expect(postLogout(null)).resolves.toBeNull();
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 403 }));
    await expect(postLogout(null)).resolves.toBeNull();
  });
});
