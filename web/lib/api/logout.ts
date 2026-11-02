import type { LogoutResp } from "@/lib/api/types";

/** Where sign-out lands when the IdP has no logout page of its own. The
 *  flag keeps `/sign-in` from auto-redirecting straight back to the IdP,
 *  whose session is still live, which would sign the user back in. */
export const SIGNED_OUT_HREF = "/sign-in?signed_out=1";

/**
 * `POST /auth/logout`, then say where to navigate, or `null` when the
 * server could not be reached.
 *
 * Asks for JSON: for an OIDC session whose IdP publishes an end-session
 * URL, the server answers `200 {redirect_to}` instead of a 302. `fetch()`
 * can't follow that redirect itself (the IdP is another origin and sends no
 * CORS headers, so the request rejects), which left the IdP session alive
 * and showed "Could not sign out" after the cookies were already gone.
 */
export async function postLogout(csrf: string | null): Promise<string | null> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (csrf) headers["X-CSRF-Token"] = csrf;
  let res: Response;
  try {
    res = await fetch("/auth/logout", {
      method: "POST",
      credentials: "include",
      headers,
    });
  } catch {
    return null;
  }
  if (!res.ok) return null;
  if (res.status === 200) {
    const body = (await res
      .json()
      .catch(() => null)) as Partial<LogoutResp> | null;
    const target = body?.redirect_to;
    if (typeof target === "string" && /^https?:\/\//i.test(target))
      return target;
  }
  return SIGNED_OUT_HREF;
}
