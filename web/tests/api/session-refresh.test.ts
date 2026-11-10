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

const {
  ensureFreshSession,
  minRefreshIntervalMs,
  needsRefresh,
  refreshDelayMs,
  resetRefreshFloorForTests,
  startSessionRefresh,
} = await import("@/lib/api/session-refresh");
const { apiFetch, resetClockOffsetForTests, serverNow } =
  await import("@/lib/api/auth-refresh");

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
  resetRefreshFloorForTests();
  resetClockOffsetForTests();
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
    // Past the refresh floor (60 s for this cookie form).
    await vi.advanceTimersByTimeAsync(61_000);

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

/** The server's clock is the real one; the client's runs `skewMs` off. */
function skewedServer(skewMs: number, lifetime = 3600, sendDate = true) {
  const serverMs = () => Date.now() - skewMs;
  const exp = (offset: number) => Math.floor(serverMs() / 1000) + offset;
  const setExp = () => jar.set(EXP, `${exp(lifetime)}.${lifetime}`);
  jar.clear();
  jar.set(CSRF, "c");
  setExp();
  onRefresh = async () => {
    setExp();
    return new Response("{}", {
      status: 200,
      headers: sendDate ? { date: new Date(serverMs()).toUTCString() } : {},
    });
  };
  return { serverMs };
}

describe("a client clock that is wrong", () => {
  for (const hours of [1, 3]) {
    it(`+${hours} h with Date headers: one early refresh, then the server's schedule`, async () => {
      skewedServer(hours * 3600_000);
      // Before any estimate, a fresh token looks expired.
      expect(needsRefresh()).toBe(true);
      const stop = startSessionRefresh();
      await vi.advanceTimersByTimeAsync(0);
      expect(refreshes).toBe(1);
      // The refresh response's Date header taught the offset.
      expect(needsRefresh()).toBe(false);
      await vi.advanceTimersByTimeAsync(3600_000);
      // One more, about 48 min later on the server's clock.
      expect(refreshes).toBe(2);
      stop();
    });

    it(`+${hours} h without any Date header: at most one refresh per floor interval`, async () => {
      skewedServer(hours * 3600_000, 3600, false);
      // 10% of the 1 h lifetime from the cookie.
      expect(minRefreshIntervalMs()).toBe(360_000);
      const stop = startSessionRefresh();
      const times: number[] = [];
      let last = refreshes;
      for (let t = 0; t < 3600; t += 10) {
        await vi.advanceTimersByTimeAsync(10_000);
        if (refreshes !== last) {
          times.push(Date.now());
          last = refreshes;
        }
      }
      expect(refreshes).toBeLessThanOrEqual(1 + 3600 / 360);
      for (let i = 1; i < times.length; i++)
        expect(times[i]! - times[i - 1]!).toBeGreaterThanOrEqual(360_000);
      // `ensureFreshSession` inside the floor lets the request go at once.
      fetchSpy.mockClear();
      const before = refreshes;
      await ensureFreshSession();
      expect(refreshes).toBe(before);
      stop();
    });
  }

  it("−1 h: no proactive refresh storm, and a 401 still renews", async () => {
    skewedServer(-3600_000);
    const stop = startSessionRefresh();
    await vi.advanceTimersByTimeAsync(3600_000);
    // The token looks two hours long: at most the one scheduled refresh.
    expect(refreshes).toBeLessThanOrEqual(1);
    stop();

    let call = 0;
    fetchSpy.mockImplementation((async (input: RequestInfo | URL) => {
      if (String(input).includes("/api/auth/refresh")) {
        refreshes += 1;
        return onRefresh();
      }
      call += 1;
      return call === 1
        ? new Response("unauth", { status: 401 })
        : new Response("ok", { status: 200 });
    }) as typeof fetch);
    const before = refreshes;
    const res = await apiFetch("/series", {});
    expect(res.ok).toBe(true);
    expect(refreshes).toBe(before + 1);
  });

  it("learns the offset from any API response's Date header", async () => {
    jar.clear();
    fetchSpy.mockImplementation(
      (async () =>
        new Response("{}", {
          status: 200,
          headers: { date: new Date(Date.now() + 3600_000).toUTCString() },
        })) as typeof fetch,
    );
    await apiFetch("/series", {});
    expect(Math.abs(serverNow() - (Date.now() + 3600_000))).toBeLessThan(1_500);
  });

  it("a response without a Date header changes nothing", async () => {
    jar.clear();
    fetchSpy.mockImplementation(
      (async () =>
        new Response("{}", {
          status: 200,
          headers: { date: new Date(Date.now() + 3600_000).toUTCString() },
        })) as typeof fetch,
    );
    await apiFetch("/series", {});
    const learned = serverNow() - Date.now();
    fetchSpy.mockImplementation(
      (async () => new Response("{}", { status: 200 })) as typeof fetch,
    );
    await apiFetch("/series", {});
    expect(serverNow() - Date.now()).toBe(learned);
    fetchSpy.mockImplementation(
      (async () =>
        new Response("{}", {
          status: 200,
          headers: { date: "not a date" },
        })) as typeof fetch,
    );
    await apiFetch("/series", {});
    expect(serverNow() - Date.now()).toBe(learned);
  });
});

describe("no-answer backoff", () => {
  it("doubles the retry delay up to ten minutes, and resets on visible", async () => {
    signIn(10);
    onRefresh = async () => new Response("down", { status: 502 });
    const stop = startSessionRefresh();
    await vi.advanceTimersByTimeAsync(0);
    expect(refreshes).toBe(1);
    // 30 s would be inside the 60 s floor: the floor wins, then 60 s ...
    const at: number[] = [Date.now()];
    let last = refreshes;
    for (let t = 0; t < 40 * 60; t += 5) {
      await vi.advanceTimersByTimeAsync(5_000);
      if (refreshes !== last) {
        at.push(Date.now());
        last = refreshes;
      }
    }
    const gaps = at.slice(1).map((t, i) => (t - at[i]!) / 1000);
    // Growing gaps, never more than ten minutes apart.
    for (let i = 1; i < gaps.length; i++)
      expect(gaps[i]!).toBeGreaterThanOrEqual(gaps[i - 1]!);
    expect(Math.max(...gaps)).toBeLessThanOrEqual(600);
    expect(gaps.at(-1)).toBe(600);

    const before = refreshes;
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(refreshes).toBe(before + 1);
    stop();
  });
});
