import { beforeEach, describe, expect, it, vi } from "vitest";
import { LEGACY_CACHES } from "@/lib/pwa/cache-policy";
const mock = vi.hoisted(() => ({
  config: {} as Record<string, unknown>,
  precache: vi.fn(),
}));
vi.mock("serwist", () => ({
  Serwist: class {
    constructor(config: Record<string, unknown>) {
      mock.config = config;
    }
    addEventListeners() {}
    matchPrecache = mock.precache;
  },
  CacheFirst: class {},
  ExpirationPlugin: class {},
}));
let listeners: Record<string, (event: never) => void>;
function dispatch(type: string, event: unknown) {
  listeners[type]!(event as never);
}
function request(path: string, options: { mode?: string; rsc?: boolean } = {}) {
  const event = {
    request: {
      url: `https://folio.test${path}`,
      method: "GET",
      mode: options.mode ?? "cors",
      headers: new Headers(options.rsc ? { RSC: "1" } : {}),
    },
    stopImmediatePropagation: vi.fn(),
    respondWith: vi.fn(),
    waitUntil: vi.fn(),
  };
  dispatch("fetch", event);
  return event;
}
beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  listeners = {};
  vi.stubGlobal("self", {
    location: { origin: "https://folio.test" },
    __SW_MANIFEST: [],
    addEventListener: (name: string, cb: (event: never) => void) => {
      listeners[name] = cb;
    },
  });
  vi.stubGlobal("caches", { delete: vi.fn().mockResolvedValue(true) });
  vi.stubGlobal("fetch", vi.fn());
  await import("@/app/sw");
});
describe("worker isolation", () => {
  it.each([
    "/api/me",
    "/api/series/1",
    "/api/admin/users",
    "/unrecognized/private",
    "/issues/a/pages/0",
  ])("leaves %s entirely to the native loader", (path) => {
    const event = request(path);
    expect(event.stopImmediatePropagation).toHaveBeenCalled();
    expect(event.respondWith).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });
  it("never reissues RSC navigations", () => {
    const event = request("/series/example", { rsc: true });
    expect(event.respondWith).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });
  it("uses the public fallback only after document transport failure", async () => {
    vi.mocked(fetch).mockRejectedValue(new TypeError("offline"));
    mock.precache.mockResolvedValue(new Response("offline page"));
    const event = request("/bookmarks", { mode: "navigate" });
    const response = await event.respondWith.mock.calls[0]![0];
    expect(await response.text()).toBe("offline page");
    expect(mock.precache).toHaveBeenCalledWith("/offline.html");
  });
  it("preserves server errors rather than pretending they are offline", async () => {
    vi.mocked(fetch).mockResolvedValue(new Response("error", { status: 503 }));
    const event = request("/", { mode: "navigate" });
    expect((await event.respondWith.mock.calls[0]![0]).status).toBe(503);
    expect(mock.precache).not.toHaveBeenCalled();
  });
  it("removes all known legacy private cache buckets on activation", async () => {
    let work!: Promise<unknown>;
    dispatch("activate", {
      waitUntil: (p: Promise<unknown>) => {
        work = p;
      },
    });
    await work;
    expect(vi.mocked(caches.delete).mock.calls.map(([name]) => name)).toEqual(
      LEGACY_CACHES,
    );
  });
});

it("does not repopulate thumbnails with a response started before logout", async () => {
  const cache = {
    match: vi.fn().mockResolvedValue(undefined),
    put: vi.fn(),
    keys: vi.fn().mockResolvedValue([]),
    delete: vi.fn(),
  };
  vi.stubGlobal("caches", {
    open: vi.fn().mockResolvedValue(cache),
    delete: vi.fn().mockResolvedValue(true),
  });
  let finish!: (response: Response) => void;
  vi.mocked(fetch).mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const event = request("/issues/a/pages/0/thumb");
  let clear!: Promise<unknown>;
  dispatch("message", {
    data: { type: "FOLIO_CLEAR_PRIVATE" },
    ports: [],
    waitUntil: (p: Promise<unknown>) => {
      clear = p;
    },
  });
  await clear;
  finish(new Response("old account image"));
  await event.waitUntil.mock.calls[0]![0];
  expect(cache.put).not.toHaveBeenCalled();
});
