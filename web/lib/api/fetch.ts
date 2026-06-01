/**
 * Server-side fetch helper used by RSC pages. Forwards the user's cookies to
 * the Rust API so per-user filtering (library ACLs) applies.
 *
 * Client-side code uses `apiFetch` (in `auth-refresh.ts`) and the
 * mutation hooks in `lib/api/mutations/` instead. Both route through
 * the Rust binary's `/api/` namespace so they don't collide with HTML
 * page paths — callers pass the bare backend path here and the
 * helper prepends `/api`.
 */
import { cookies } from "next/headers";

const API_BASE =
  process.env.COMIC_API_BASE ||
  process.env.NEXT_PUBLIC_API_URL ||
  "http://127.0.0.1:8080";

/**
 * Hard ceiling on a single server-side API request. Without it a hung or
 * very slow backend request blocks the RSC render indefinitely — the App
 * Router then sits on `loading.tsx` forever with no client-side recovery
 * (the reader-exit / archive-apply "spins forever" hangs). 10s is generous
 * for legitimately slow queries on large libraries while still recovering
 * from a true stall: the fetch aborts, this throws, and the page surfaces
 * its error boundary (or a best-effort caller falls back) instead of
 * hanging. Paired with the client-side `LoadingWatchdog` for stalls that
 * happen outside this fetch (proxy / upstream / stream).
 */
const REQUEST_TIMEOUT_MS = 10_000;

export async function apiGet<T>(path: string): Promise<T> {
  const cookieHeader = (await cookies()).toString();
  let res: Response;
  try {
    res = await fetch(`${API_BASE}/api${path}`, {
      headers: {
        Accept: "application/json",
        Cookie: cookieHeader,
      },
      cache: "no-store",
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (e) {
    // AbortSignal.timeout → DOMException "TimeoutError"; surface as a
    // gateway-timeout ApiError so error boundaries + best-effort callers
    // treat it like any other failed fetch instead of hanging.
    if (e instanceof DOMException && e.name === "TimeoutError") {
      throw new ApiError(504, `upstream timeout after ${REQUEST_TIMEOUT_MS}ms: ${path}`);
    }
    throw e;
  }
  if (!res.ok) {
    let detail = "";
    try {
      detail = JSON.stringify(await res.json());
    } catch {
      detail = await res.text();
    }
    throw new ApiError(res.status, detail);
  }
  return (await res.json()) as T;
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}
