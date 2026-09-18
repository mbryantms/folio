// @vitest-environment jsdom
import { QueryClient } from "@tanstack/react-query";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { clearPrivateState } from "@/lib/pwa/private-state";
import { THUMB_CACHE } from "@/lib/pwa/cache-policy";
beforeEach(() => {
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {},
  });
  vi.stubGlobal("caches", { delete: vi.fn().mockResolvedValue(true) });
});
afterEach(() => vi.unstubAllGlobals());
it("clears private query data and prevents an old request from repopulating it", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  client.setQueryData(["private-user"], { name: "previous account" });
  let complete!: (value: string) => void;
  const pending = client
    .fetchQuery({
      queryKey: ["private-late"],
      queryFn: () =>
        new Promise<string>((resolve) => {
          complete = resolve;
        }),
    })
    .catch(() => undefined);
  await clearPrivateState(client);
  complete("private result");
  await pending;
  expect(client.getQueryCache().getAll()).toHaveLength(0);
  expect(caches.delete).toHaveBeenCalledWith(THUMB_CACHE);
});
