/**
 * Offline reader shell precache (WP-4.6, offline plan step 2).
 *
 * The regular reader route is authenticated SSR, so it cannot boot without
 * the server. The offline shell is `/downloads`: a client-rendered page that
 * reads its data from IndexedDB. This module stores a **public** copy of
 * that document — fetched with `credentials: "omit"`, so it carries no
 * account data and is safe to keep across sessions — together with every
 * hashed `/_next/static` asset it references, found by crawling the HTML,
 * its flight data and the referenced chunks/stylesheets (lazy chunks are
 * named inside their parent chunk). The service worker serves the stored
 * document for offline navigations and falls back to these assets when the
 * network is unreachable.
 *
 * The document is written last, after every asset it needs, so a partial
 * warm never leaves a shell whose chunks are missing. Assets no longer
 * referenced by the new document are pruned afterwards.
 */

import { OFFLINE_SHELL_CACHE, OFFLINE_SHELL_PATH } from "./offline-store";

const ASSET_RE =
  /(?:\/_next\/)?static\/(?:chunks|css|media)\/[A-Za-z0-9_\-.~/%@]+?\.(?:js|css|woff2?|ttf|png|svg|jpe?g|webp|ico)(?=[^A-Za-z0-9_\-.~/%@]|$)/g;
const CSS_URL_RE = /url\(\s*['"]?([^'")]+)['"]?\s*\)/g;

/** Every `/_next/static/...` reference in a document or asset body. */
export function extractAssetRefs(body: string, base?: string): string[] {
  const found = new Set<string>();
  for (const match of body.matchAll(ASSET_RE)) {
    const raw = match[0];
    found.add(raw.startsWith("/_next/") ? raw : `/_next/${raw}`);
  }
  if (base && base.endsWith(".css")) {
    for (const match of body.matchAll(CSS_URL_RE)) {
      const ref = match[1]!;
      if (ref.startsWith("data:")) continue;
      try {
        const url = new URL(ref, `https://shell.invalid${base}`);
        if (
          url.origin === "https://shell.invalid" &&
          url.pathname.startsWith("/_next/static/")
        )
          found.add(url.pathname);
      } catch {
        /* not a URL */
      }
    }
  }
  return [...found];
}

export type WarmResult = { assets: number; html: boolean };

export async function warmOfflineShell(
  opts: {
    fetch?: typeof fetch;
    caches?: CacheStorage;
    maxAssets?: number;
  } = {},
): Promise<WarmResult> {
  const doFetch = opts.fetch ?? globalThis.fetch.bind(globalThis);
  const cs = opts.caches ?? globalThis.caches;
  const maxAssets = opts.maxAssets ?? 1500;
  if (!cs) return { assets: 0, html: false };
  const page = await doFetch(OFFLINE_SHELL_PATH, {
    credentials: "omit",
    cache: "no-store",
    headers: { Accept: "text/html" },
  });
  if (
    !page.ok ||
    !(page.headers.get("content-type") ?? "").includes("text/html")
  )
    return { assets: 0, html: false };
  const html = await page.clone().text();
  const cache = await cs.open(OFFLINE_SHELL_CACHE);
  const seen = new Set<string>();
  const queue = extractAssetRefs(html);
  while (queue.length && seen.size < maxAssets) {
    const path = queue.shift()!;
    if (seen.has(path)) continue;
    seen.add(path);
    let response = await cache.match(path);
    if (!response) {
      const fresh = await doFetch(path, { credentials: "omit" }).catch(
        () => null,
      );
      if (!fresh?.ok) continue;
      await cache.put(path, fresh.clone());
      response = fresh;
    }
    if (path.endsWith(".js") || path.endsWith(".css")) {
      const body = await response.text().catch(() => "");
      for (const ref of extractAssetRefs(body, path))
        if (!seen.has(ref)) queue.push(ref);
    }
  }
  // A rebuilt response: a navigation cannot be answered with one that
  // carries a `redirected` flag, and the stored copy needs no stream.
  // The body is already decoded, so the transfer headers no longer apply.
  const headers = new Headers(page.headers);
  headers.delete("content-encoding");
  headers.delete("content-length");
  await cache.put(
    OFFLINE_SHELL_PATH,
    new Response(html, { status: 200, headers }),
  );
  // Prune assets the current document no longer references.
  for (const request of await cache.keys()) {
    const path = new URL(request.url, "https://shell.invalid").pathname;
    if (path !== OFFLINE_SHELL_PATH && !seen.has(path))
      await cache.delete(request);
  }
  return { assets: seen.size, html: true };
}
