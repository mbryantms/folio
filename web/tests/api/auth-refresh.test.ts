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
const jar = new Map<string, string>();
/** Cookie strings scripts wrote (`document.cookie = ...`). */
const written: string[] = [];
(globalThis as unknown as { document: { cookie: string } }).document = {
  get cookie() {
    return [...jar].map(([k, v]) => `${k}=${v}`).join("; ");
  },
  set cookie(v: string) {
    written.push(v);
    const [kv, ...attrs] = v.split(";").map((p) => p.trim());
    const [name, value] = kv!.split("=");
    if (attrs.some((a) => a === "Max-Age=0")) jar.delete(name!);
    else jar.set(name!, value ?? "");
  },
};
const CSRF = "__Host-comic_csrf";
const EXP = "__Host-comic_access_exp";
/** Seconds since the epoch, `offset` from now. */
const epoch = (offset: number) => Math.floor(Date.now() / 1000) + offset;
function setCookie(value: string) {
  jar.clear();
  jar.set(CSRF, value);
}
function setExp(exp: number | null) {
  if (exp === null) jar.delete(EXP);
  else jar.set(EXP, String(exp));
}

const { apiFetch, resumeSession, REFRESH_TIMEOUT_MS } =
  await import("@/lib/api/auth-refresh");

type Capture = { url: string; init: RequestInit };

describe("apiFetch refresh-and-retry", () => {
  const captured: Capture[] = [];

  beforeEach(() => {
    captured.length = 0;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("re-reads the CSRF cookie before retrying after a successful refresh", async () => {
    // Sequence:
    //   1. PATCH with CSRF=OLD → 401
    //   2. POST /api/auth/refresh → 200 (server rotates the cookie)
    //   3. Retry PATCH — must carry CSRF=NEW, not the original OLD header
    setCookie("OLD");
    let call = 0;
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockImplementation((async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => {
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
      .mockImplementation(
        (async () => new Response("unauth", { status: 401 })) as typeof fetch,
      );

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

/**
 * Refresh rotation is single-use on the server: of two tabs that refresh
 * with the same cookie at once, one gets 401. The loser must not be signed
 * out — cookies are shared, so the winner's Set-Cookie already renewed it.
 */
describe("apiFetch when another tab wins the refresh", () => {
  const captured: Capture[] = [];
  let events: string[] = [];

  beforeEach(() => {
    captured.length = 0;
    events = [];
    vi.stubGlobal("window", {
      dispatchEvent: (e: Event) => {
        events.push(String((e as CustomEvent).detail));
        return true;
      },
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("retries the original request once after losing the refresh race", async () => {
    vi.stubGlobal("navigator", {}); // no Web Locks: the race can happen
    setCookie("OLD");
    setExp(null); // this tab's access token already lapsed
    let call = 0;
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockImplementation((async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => {
      call += 1;
      captured.push({ url: String(input), init: init ?? {} });
      if (call === 1) return new Response("unauth", { status: 401 });
      if (call === 2) {
        // The other tab's refresh committed first and its Set-Cookie
        // rotated the shared cookies; ours presented the stale token.
        setCookie("NEW");
        setExp(epoch(3600));
        return new Response("refresh replay", { status: 401 });
      }
      return new Response("ok", { status: 200 });
    }) as typeof fetch);

    const res = await apiFetch("/me/progress", {
      method: "POST",
      headers: { "X-CSRF-Token": "OLD" },
    });

    expect(res.ok).toBe(true);
    expect(fetchSpy).toHaveBeenCalledTimes(3);
    expect(captured[1]!.url).toContain("/api/auth/refresh");
    expect(captured[2]!.url).toBe("/api/me/progress");
    expect(headerOf(captured[2]!.init, "X-CSRF-Token")).toBe("NEW");
    expect(events).not.toContain("authentication");
  });

  it("skips its own refresh when another tab refreshed while it waited for the lock", async () => {
    setCookie("OLD");
    setExp(epoch(10));
    const request = vi.fn(async (_name: string, fn: () => Promise<unknown>) => {
      // While this tab queued on the lock, the holder refreshed.
      setCookie("NEW");
      setExp(epoch(3600));
      return fn();
    });
    vi.stubGlobal("navigator", { locks: { request } });
    let call = 0;
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockImplementation((async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => {
      call += 1;
      captured.push({ url: String(input), init: init ?? {} });
      return call === 1
        ? new Response("unauth", { status: 401 })
        : new Response("ok", { status: 200 });
    }) as typeof fetch);

    const res = await apiFetch("/series", {});

    expect(res.ok).toBe(true);
    expect(request).toHaveBeenCalledWith(
      "folio:auth-refresh",
      expect.any(Function),
    );
    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(captured.some((c) => c.url.includes("/auth/refresh"))).toBe(false);
    expect(headerOf(captured[1]!.init, "X-CSRF-Token")).toBe("NEW");
  });

  it("still reports a signed-out session when no tab refreshed", async () => {
    vi.stubGlobal("navigator", {});
    setCookie("SAME");
    let call = 0;
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation((async () => {
        call += 1;
        return new Response("unauth", { status: 401 });
      }) as typeof fetch);

    const res = await apiFetch("/series", {});

    expect(res.status).toBe(401);
    expect(fetchSpy).toHaveBeenCalledTimes(2); // original + refresh, no retry
    expect(call).toBe(2);
    expect(events).toContain("authentication");
    // A definitive refusal expires the readable auth cookies, so the
    // sign-in page's resume and the scheduler stop trying.
    expect(jar.has(CSRF)).toBe(false);
    expect(written.some((w) => w.startsWith(`${CSRF}=;`))).toBe(true);
    expect(written.some((w) => w.startsWith(`${EXP}=;`))).toBe(true);
  });

  it("refreshes when only the CSRF cookie changed, not the expiry", async () => {
    setCookie("OLD");
    const exp = epoch(10);
    setExp(exp);
    const request = vi.fn(async (_name: string, fn: () => Promise<unknown>) => {
      // Something re-minted the CSRF cookie (a password change, `/auth/me`)
      // but nobody renewed the access token.
      jar.set(CSRF, "REMINTED");
      return fn();
    });
    vi.stubGlobal("navigator", { locks: { request } });
    let call = 0;
    vi.spyOn(globalThis, "fetch").mockImplementation((async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => {
      call += 1;
      captured.push({ url: String(input), init: init ?? {} });
      if (call === 1) return new Response("unauth", { status: 401 });
      if (call === 2) {
        setExp(epoch(3600));
        return new Response("{}", { status: 200 });
      }
      return new Response("ok", { status: 200 });
    }) as typeof fetch);

    const res = await apiFetch("/series", {});

    expect(res.ok).toBe(true);
    expect(captured[1]!.url).toContain("/api/auth/refresh");
    expect(captured).toHaveLength(3);
  });

  it("does not take a sign-out's cleared cookie for another tab's refresh", async () => {
    vi.stubGlobal("navigator", {});
    setCookie("OLD");
    setExp(epoch(10));
    let call = 0;
    vi.spyOn(globalThis, "fetch").mockImplementation((async () => {
      call += 1;
      if (call === 2) {
        // Another tab signed out meanwhile: both cookies are gone.
        jar.clear();
      }
      return new Response("unauth", { status: 401 });
    }) as typeof fetch);

    const res = await apiFetch("/series", {});

    expect(res.status).toBe(401);
    expect(call).toBe(2);
    expect(events).toContain("authentication");
  });

  it("does not count an expiry that moved but has already passed", async () => {
    vi.stubGlobal("navigator", {});
    setCookie("OLD");
    setExp(epoch(-120));
    let call = 0;
    vi.spyOn(globalThis, "fetch").mockImplementation((async () => {
      call += 1;
      if (call === 2) setExp(epoch(-60));
      return new Response("unauth", { status: 401 });
    }) as typeof fetch);

    const res = await apiFetch("/series", {});
    expect(res.status).toBe(401);
    expect(events).toContain("authentication");
  });
});

/**
 * A refresh that gets no answer (5xx, network failure, timeout) knows
 * nothing about the session: no "authentication" event (which would wipe
 * the account's private caches), and the original request is not retried
 * more than once.
 */
describe("apiFetch when the refresh gets no answer", () => {
  let events: string[] = [];
  beforeEach(() => {
    events = [];
    vi.stubGlobal("window", {
      dispatchEvent: (e: Event) => {
        events.push(String((e as CustomEvent).detail));
        return true;
      },
    });
    vi.stubGlobal("navigator", {});
    setCookie("X");
    setExp(null);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  const cases: [string, () => Promise<Response>][] = [
    ["a 5xx", async () => new Response("down", { status: 502 })],
    [
      "a network error",
      async () => {
        throw new TypeError("Failed to fetch");
      },
    ],
    [
      "a timeout",
      async () => {
        throw new DOMException("signal timed out", "TimeoutError");
      },
    ],
  ];
  for (const [label, refresh] of cases) {
    it(`keeps the session after ${label}`, async () => {
      const urls: string[] = [];
      const inits: RequestInit[] = [];
      vi.spyOn(globalThis, "fetch").mockImplementation((async (
        input: RequestInfo | URL,
        init?: RequestInit,
      ) => {
        urls.push(String(input));
        inits.push(init ?? {});
        if (String(input).includes("/auth/refresh")) return refresh();
        return new Response("unauth", { status: 401 });
      }) as typeof fetch);

      const res = await apiFetch("/me/progress", { method: "POST" });

      expect(res.status).toBe(401);
      expect(events).not.toContain("authentication");
      expect(
        urls.filter((u) => u === "/api/me/progress").length,
      ).toBeLessThanOrEqual(2);
      // Private state stays: the readable cookies were not expired.
      expect(jar.get(CSRF)).toBe("X");
      // The refresh carries a bounded timeout signal.
      const refreshInit =
        inits[urls.findIndex((u) => u.includes("/auth/refresh"))]!;
      expect(refreshInit.signal).toBeInstanceOf(AbortSignal);
    });
  }

  it("bounds the refresh at a named 15 s timeout", () => {
    expect(REFRESH_TIMEOUT_MS).toBe(15_000);
  });
});

describe("resumeSession", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("sends nothing without a CSRF cookie", async () => {
    jar.clear();
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    expect(await resumeSession()).toBe(false);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("renews the session when the refresh cookie is live", async () => {
    vi.stubGlobal("navigator", {});
    setCookie("X");
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(
        (async () => new Response("{}", { status: 200 })) as typeof fetch,
      );
    expect(await resumeSession()).toBe(true);
    expect(String(fetchSpy.mock.calls[0]![0])).toContain("/api/auth/refresh");
  });
});

function headerOf(init: RequestInit, name: string): string | null {
  const h = init.headers;
  if (!h) return null;
  if (h instanceof Headers) return h.get(name);
  if (Array.isArray(h)) {
    const entry = h.find(([k]) => k.toLowerCase() === name.toLowerCase());
    return entry ? entry[1]! : null;
  }
  const rec = h as Record<string, string>;
  for (const [k, v] of Object.entries(rec)) {
    if (k.toLowerCase() === name.toLowerCase()) return v;
  }
  return null;
}
