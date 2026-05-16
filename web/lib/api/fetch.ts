/**
 * Server-side fetch helper used by RSC pages. Forwards the user's cookies to
 * the Rust API so per-user filtering (library ACLs) applies.
 *
 * Client-side code uses `lib/api/client.ts` (or `apiFetch` in
 * `auth-refresh.ts`) instead. All three helpers route through the
 * Rust binary's `/api/` namespace so they don't collide with HTML
 * page paths — callers pass the bare backend path here and the
 * helper prepends `/api`.
 */
import { cookies } from "next/headers";

const API_BASE =
  process.env.COMIC_API_BASE ||
  process.env.NEXT_PUBLIC_API_URL ||
  "http://127.0.0.1:8080";

export async function apiGet<T>(path: string): Promise<T> {
  const cookieHeader = (await cookies()).toString();
  const res = await fetch(`${API_BASE}/api${path}`, {
    headers: {
      Accept: "application/json",
      Cookie: cookieHeader,
    },
    cache: "no-store",
  });
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
