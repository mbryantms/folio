"use client";

import { usePathname } from "next/navigation";
import { useEffect } from "react";

import { startSessionRefresh } from "@/lib/api/session-refresh";

/** Auth pages manage the session themselves (sign-in resumes it). */
const AUTH_PAGE =
  /^\/(?:[a-z]{2}(?:-[A-Z]{2})?\/)?(?:sign-in|forgot-password|reset-password)(?:\/|$)/;

/**
 * Renews the access token before it lapses (`lib/api/session-refresh.ts`).
 * Mounted once by the root layout, only for a signed-in request; inert on
 * the auth pages.
 */
export function SessionRefresher() {
  const pathname = usePathname();
  const onAuthPage = AUTH_PAGE.test(pathname ?? "");
  useEffect(() => {
    if (onAuthPage) return;
    return startSessionRefresh();
  }, [onAuthPage]);
  return null;
}
