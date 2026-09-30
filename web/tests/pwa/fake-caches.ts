/**
 * Minimal in-memory Cache Storage for unit tests (node has none). Keys are
 * normalized to path + search, which is what the app stores under.
 */
const keyOf = (input: RequestInfo | URL): string => {
  const raw =
    typeof input === "string"
      ? input
      : input instanceof URL
        ? input.href
        : input.url;
  const url = new URL(raw, "https://folio.test");
  return url.pathname + url.search;
};

type Options = {
  ignoreSearch?: boolean;
  ignoreVary?: boolean;
  cacheName?: string;
};

export class FakeCache {
  readonly entries = new Map<string, Response>();
  /** When set, `put` rejects with this error (quota simulation). */
  failPut: Error | null = null;

  async match(input: RequestInfo | URL, opts?: Options) {
    const key = keyOf(input);
    const hit = opts?.ignoreSearch
      ? [...this.entries].find(
          ([k]) => k.split("?")[0] === key.split("?")[0],
        )?.[1]
      : this.entries.get(key);
    return hit?.clone();
  }
  async put(input: RequestInfo | URL, response: Response) {
    if (this.failPut) throw this.failPut;
    // Consume like the real API does.
    const body = await response.arrayBuffer();
    this.entries.set(
      keyOf(input),
      new Response(body, {
        status: response.status,
        headers: response.headers,
      }),
    );
  }
  async delete(input: RequestInfo | URL) {
    return this.entries.delete(keyOf(input));
  }
  async keys() {
    return [...this.entries.keys()].map(
      (k) => new Request(new URL(k, "https://folio.test")),
    );
  }
}

export class FakeCacheStorage {
  readonly stores = new Map<string, FakeCache>();
  async open(name: string) {
    let cache = this.stores.get(name);
    if (!cache) this.stores.set(name, (cache = new FakeCache()));
    return cache;
  }
  async has(name: string) {
    return this.stores.has(name);
  }
  async delete(name: string) {
    return this.stores.delete(name);
  }
  async keys() {
    return [...this.stores.keys()];
  }
  async match(input: RequestInfo | URL, opts?: Options) {
    const names = opts?.cacheName ? [opts.cacheName] : [...this.stores.keys()];
    for (const name of names) {
      const hit = await this.stores.get(name)?.match(input, opts);
      if (hit) return hit;
    }
    return undefined;
  }
}

export const asCacheStorage = (fake: FakeCacheStorage) =>
  fake as unknown as CacheStorage;
