/**
 * The client-side JSON GET every query hook goes through, and the typed
 * error it throws.
 *
 * Split out of `queries.ts` (which re-exports both, so existing imports
 * keep resolving) for the reader bundle budget: Turbopack ships a
 * reachable module whole, so a first-load file that needed only
 * `HttpError` pulled every hook in `queries.ts` into the reader's first
 * load. First-load code imports from here or from the `queries-*` shards
 * (`docs/dev/pwa-performance.md`, "Reader bundle budget").
 */
import { apiFetch } from "./auth-refresh";

/**
 * Typed error thrown by every client-side query so callers can inspect the
 * HTTP status without parsing message strings. The server-side `ApiError`
 * in `lib/api/fetch.ts` cannot be reused here because that module imports
 * `next/headers` (server-only).
 */
export class HttpError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    /** `error.code` from the server's envelope, when it sent one (e.g.
     *  `"filter_too_slow"`), so a caller can branch on the failure. */
    public readonly code: string | null = null,
    /** `error.details` field errors from a 422 (empty otherwise). */
    public readonly fields: { field: string; message: string }[] = [],
  ) {
    super(message);
    this.name = "HttpError";
  }
}

/** Build an [`HttpError`] from a failed response's error envelope. */
export async function httpErrorFrom(
  res: Response,
  path: string,
): Promise<HttpError> {
  let detail = `${path} → ${res.status}`;
  let code: string | null = null;
  let fields: { field: string; message: string }[] = [];
  try {
    const body = await res.json();
    detail = body?.error?.message ?? detail;
    code = typeof body?.error?.code === "string" ? body.error.code : null;
    if (Array.isArray(body?.error?.details)) fields = body.error.details;
  } catch {
    /* ignore */
  }
  return new HttpError(res.status, detail, code, fields);
}

export async function jsonFetch<T>(path: string): Promise<T> {
  const res = await apiFetch(path, {
    headers: { Accept: "application/json" },
  });
  if (!res.ok) throw await httpErrorFrom(res, path);
  return (await res.json()) as T;
}
