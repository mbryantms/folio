import { headers } from "next/headers";
import { isSafeNextPath } from "@/app/[locale]/sign-in/safe-next";
/** Set by the proxy, never trust a caller-provided return URL header. */
export async function signInUrl() {
  const next = (await headers()).get("x-folio-return-to");
  return isSafeNextPath(next)
    ? `/sign-in?next=${encodeURIComponent(next)}`
    : "/sign-in";
}
