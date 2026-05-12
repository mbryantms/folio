/// <reference lib="webworker" />

/**
 * Off-main-thread page decode (§7.2). Receives a `{ url }` message, fetches the
 * blob, decodes via `createImageBitmap`, transfers the resulting `ImageBitmap`
 * back to the main thread.
 *
 * Per spec: WebCodecs `ImageDecoder` is *not* used — `createImageBitmap` is
 * sufficient for static images and has broader format support across browsers.
 */
type DecodeRequest = { id: string; url: string };
type DecodeResponse =
  | { id: string; ok: true; bitmap: ImageBitmap }
  | { id: string; ok: false; error: string };

self.addEventListener("message", async (ev: MessageEvent<DecodeRequest>) => {
  const { id, url } = ev.data;
  try {
    const resp = await fetch(url, { credentials: "include" });
    if (!resp.ok) {
      const msg: DecodeResponse = {
        id,
        ok: false,
        error: `HTTP ${resp.status}`,
      };
      (self as DedicatedWorkerGlobalScope).postMessage(msg);
      return;
    }
    const blob = await resp.blob();
    const bitmap = await createImageBitmap(blob, {
      colorSpaceConversion: "none",
      resizeQuality: "high",
    });
    const msg: DecodeResponse = { id, ok: true, bitmap };
    (self as DedicatedWorkerGlobalScope).postMessage(msg, [bitmap]);
  } catch (e) {
    const msg: DecodeResponse = {
      id,
      ok: false,
      error: e instanceof Error ? e.message : String(e),
    };
    (self as DedicatedWorkerGlobalScope).postMessage(msg);
  }
});

export {};
