/**
 * Regression tests for `apiFetch`'s 401-driven refresh-and-retry flow.
 *
 * The fix being pinned here: when the first request 401s and
 * `/api/auth/refresh` succeeds, the retry must re-read the CSRF cookie
 * before sending. The refresh handler rotates `__Host-comic_csrf`, so
 * the value the caller embedded into `init.headers['X-CSRF-Token']` is
 * already stale by the time we retry; sending the stale header with
 * the fresh cookie causes a CSRF mismatch 403 on the retry.
 *
 * This was the bug behind the prod incident where the *second*
 * password change in a session always 403'd: the first change worked
 * because no refresh was needed, the second triggered the refresh
 * which rotated the cookie and then sent the old header.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// vitest config defaults to environment:"node", which has no `document`.
// `apiFetch` only reads `document.cookie`, so shim a minimal stand-in
// before importing. Per-test mutations swap the getter to model server-
// side cookie rotation.
let cookieValue = "";
(globalThis as unknown as { document: { cookie: string } }).document = {
  get cookie() {
    return cookieValue;
  },
  set cookie(_v: string) {
    /* writes from real apps go through Set-Cookie; ignore here */
  },
};
function setCookie(value: string) {
  cookieValue = `__Host-comic_csrf=${value}`;
}

const { apiFetch } = await import("@/lib/api/auth-refresh");

type Capture = { url: string; init: RequestInit };

describe("apiFetch refresh-and-retry", () => {
  const captured: Capture[] = [];

  beforeEach(() => {
    captured.length = 0;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("re-reads the CSRF cookie before retrying after a successful refresh", async () => {
    // Sequence:
    //   1. PATCH with CSRF=OLD → 401
    //   2. POST /api/auth/refresh → 200 (server rotates the cookie)
    //   3. Retry PATCH — must carry CSRF=NEW, not the original OLD header
    setCookie("OLD");
    let call = 0;
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation((async (input: RequestInfo | URL, init?: RequestInit) => {
        call += 1;
        captured.push({ url: String(input), init: init ?? {} });
        if (call === 1) {
          return new Response("unauth", { status: 401 });
        }
        if (call === 2) {
          // Simulate the cookie rotation that /api/auth/refresh would
          // trigger server-side; the browser-equivalent here is just
          // flipping what `document.cookie` returns next.
          setCookie("NEW");
          return new Response("ok", { status: 200 });
        }
        // Retry of the original request.
        return new Response("ok", { status: 200 });
      }) as typeof fetch);

    const res = await apiFetch("/me/account", {
      method: "PATCH",
      headers: { "X-CSRF-Token": "OLD", "Content-Type": "application/json" },
      body: JSON.stringify({ new_password: "secret-no-one-will-guess" }),
    });

    expect(res.ok).toBe(true);
    expect(fetchSpy).toHaveBeenCalledTimes(3);
    // Original attempt: still carries OLD (correct — that's what the
    // caller built it with, and at request time the cookie was OLD).
    expect(headerOf(captured[0]!.init, "X-CSRF-Token")).toBe("OLD");
    // Refresh: own internal header set from the (at that time) OLD value.
    expect(captured[1]!.url).toContain("/api/auth/refresh");
    // Retry: MUST be the NEW cookie value, not OLD.
    expect(headerOf(captured[2]!.init, "X-CSRF-Token")).toBe("NEW");
  });

  it("does not retry the refresh endpoint itself (would loop)", async () => {
    setCookie("X");
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation((async () => new Response("unauth", { status: 401 })) as typeof fetch);

    const res = await apiFetch("/auth/refresh", { method: "POST" });
    expect(res.status).toBe(401);
    // Only the initial call — refresh-of-refresh is short-circuited.
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it("returns the original response when refresh itself fails", async () => {
    setCookie("X");
    let call = 0;
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation((async () => {
        call += 1;
        if (call === 1) return new Response("unauth", { status: 401 });
        // refresh fails
        return new Response("nope", { status: 403 });
      }) as typeof fetch);

    const res = await apiFetch("/me/account", { method: "PATCH" });
    expect(res.status).toBe(401);
    expect(fetchSpy).toHaveBeenCalledTimes(2);
  });
});

function headerOf(init: RequestInit, name: string): string | null {
  const h = init.headers;
  if (!h) return null;
  if (h instanceof Headers) return h.get(name);
  if (Array.isArray(h)) {
    const entry = h.find(
      ([k]) => k.toLowerCase() === name.toLowerCase(),
    );
    return entry ? entry[1]! : null;
  }
  const rec = h as Record<string, string>;
  for (const [k, v] of Object.entries(rec)) {
    if (k.toLowerCase() === name.toLowerCase()) return v;
  }
  return null;
}
