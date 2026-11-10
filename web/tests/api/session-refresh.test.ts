// @vitest-environment jsdom
/**
 * Proactive session renewal (`lib/api/session-refresh.ts`): the scheduler
 * reads `__Host-comic_access_exp` and refreshes before the access token
 * lapses; `ensureFreshSession` lets cookie-authed requests the 401 retry
 * can't cover (page images, offline downloads) renew first.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const jar = new Map<string, string>();
Object.defineProperty(document, "cookie", {
  configurable: true,
  get: () => [...jar].map(([k, v]) => `${k}=${v}`).join("; "),
  set: (v: string) => {
    const [kv, ...attrs] = v.split(";").map((p) => p.trim());
    const [name, value] = kv!.split("=");
    if (attrs.includes("Max-Age=0")) jar.delete(name!);
    else jar.set(name!, value ?? "");
  },
});
const CSRF = "__Host-comic_csrf";
const EXP = "__Host-comic_access_exp";
const epoch = (offset: number) => Math.floor(Date.now() / 1000) + offset;
const signIn = (expOffset: number | null) => {
  jar.clear();
  jar.set(CSRF, "c");
  if (expOffset !== null) jar.set(EXP, String(epoch(expOffset)));
};

const { ensureFreshSession, refreshDelayMs, startSessionRefresh } =
  await import("@/lib/api/session-refresh");

let refreshes = 0;
let fetchSpy: ReturnType<typeof vi.spyOn>;
/** What the next `/auth/refresh` does; default renews for an hour. */
let onRefresh: () => Promise<Response> = async () => {
  jar.set(EXP, String(epoch(3600)));
  return new Response("{}", { status: 200 });
};

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
  vi.setSystemTime(new Date("2026-11-10T12:00:00Z"));
  refreshes = 0;
  onRefresh = async () => {
    jar.set(EXP, String(epoch(3600)));
    return new Response("{}", { status: 200 });
  };
  fetchSpy = vi.spyOn(globalThis, "fetch").mockImplementation((async (
    input: RequestInfo | URL,
  ) => {
    if (String(input).includes("/api/auth/refresh")) {
      refreshes += 1;
      return onRefresh();
    }
    return new Response("{}", { status: 200 });
  }) as typeof fetch);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  jar.clear();
});

describe("refreshDelayMs", () => {
  it("waits about 80% of the remaining lifetime", () => {
    const now = Date.now();
    expect(refreshDelayMs(Math.floor(now / 1000) + 3600, now)).toBeCloseTo(
      0.8 * 3600_000,
      -3,
    );
  });
  it("never later than 30 s before expiry, and never negative", () => {
    const now = Date.now();
    const sec = Math.floor(now / 1000);
    // 100 s left: 80% would be 80 s, but that leaves only 20 s.
    expect(refreshDelayMs(sec + 100, sec * 1000)).toBe(70_000);
    expect(refreshDelayMs(sec + 10, sec * 1000)).toBe(0);
    expect(refreshDelayMs(sec - 10, sec * 1000)).toBe(0);
  });
});

describe("the refresh scheduler", () => {
  it("arms from the expiry cookie and refreshes before the token lapses", async () => {
    signIn(3600);
    const stop = startSessionRefresh();
    await vi.advanceTimersByTimeAsync(0.8 * 3600_000 - 1_000);
    expect(refreshes).toBe(0);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(refreshes).toBe(1);
    // Re-armed from the renewed cookie: the next one is ~48 min later.
    await vi.advanceTimersByTimeAsync(0.8 * 3600_000 - 5_000);
    expect(refreshes).toBe(1);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(refreshes).toBe(2);
    stop();
  });

  it("re-arms instead of refreshing when another tab already did", async () => {
    signIn(100);
    const stop = startSessionRefresh();
    // Another tab renews before this tab's timer (70 s) fires.
    await vi.advanceTimersByTimeAsync(40_000);
    jar.set(EXP, String(epoch(3600)));
    await vi.advanceTimersByTimeAsync(31_000);
    expect(refreshes).toBe(0);
    await vi.advanceTimersByTimeAsync(0.8 * 3600_000);
    expect(refreshes).toBe(1);
    stop();
  });

  it("does nothing for a signed-out visitor", async () => {
    jar.clear();
    const stop = startSessionRefresh();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(24 * 3600_000);
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("online"));
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchSpy).not.toHaveBeenCalled();
    stop();
  });

  it("refreshes at once when the page becomes visible near or past expiry", async () => {
    signIn(3600);
    const stop = startSessionRefresh();
    // A sleeping machine: the timer never ran, the token is now nearly
    // gone, then the tab comes back.
    jar.set(EXP, String(epoch(10)));
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(refreshes).toBe(1);

    // Past expiry: the cookie's Max-Age ran out, only the CSRF cookie
    // says a session may exist.
    jar.delete(EXP);
    onRefresh = async () => {
      jar.set(EXP, String(epoch(3600)));
      return new Response("{}", { status: 200 });
    };
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(refreshes).toBe(2);
    stop();
  });

  it("stops after the server refuses the session", async () => {
    signIn(10);
    onRefresh = async () => new Response("{}", { status: 401 });
    const stop = startSessionRefresh();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(refreshes).toBe(1);
    expect(jar.has(CSRF)).toBe(false);
    await vi.advanceTimersByTimeAsync(3600_000);
    expect(refreshes).toBe(1);
    stop();
  });
});

describe("ensureFreshSession", () => {
  it("resolves at once when the token is fresh", async () => {
    signIn(3600);
    await ensureFreshSession();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("awaits a refresh when the token is about to lapse", async () => {
    signIn(5);
    let release!: () => void;
    onRefresh = () =>
      new Promise<Response>((resolve) => {
        release = () => {
          jar.set(EXP, String(epoch(3600)));
          resolve(new Response("{}", { status: 200 }));
        };
      });
    let done = false;
    const p = ensureFreshSession().then(() => (done = true));
    await vi.advanceTimersByTimeAsync(0);
    expect(refreshes).toBe(1);
    expect(done).toBe(false);
    release();
    await p;
    expect(done).toBe(true);
  });

  it("resolves promptly offline without trying", async () => {
    signIn(null);
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    await ensureFreshSession();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("resolves promptly when the refresh fails on the network", async () => {
    signIn(null);
    onRefresh = async () => {
      throw new TypeError("Failed to fetch");
    };
    await ensureFreshSession();
    expect(refreshes).toBe(1);
    expect(jar.get(CSRF)).toBe("c");
  });
});
