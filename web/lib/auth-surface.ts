"use client";

import { usePathname } from "next/navigation";

/** Routes that are anonymous by construction. */
const AUTH_ROUTES = ["/sign-in", "/forgot-password", "/reset-password"];

export function isAuthSurface(pathname: string | null): boolean {
  return AUTH_ROUTES.some(
    (route) => pathname === route || !!pathname?.startsWith(`${route}/`),
  );
}

/**
 * Root-layout listeners call `useMe({ enabled: !useOnAuthSurface() })`.
 * Probing `/auth/me` from a sign-in page only 401s, and the 401 then runs
 * the refresh dance (`POST /auth/refresh`, a 403 without a session) on
 * every visit before the user has typed anything.
 */
export function useOnAuthSurface(): boolean {
  return isAuthSurface(usePathname());
}
