import { cache } from "react";

import { apiGet } from "@/lib/api/fetch";
import type { MeView } from "@/lib/api/types";

/** Name of the auth session cookie (mirrors the server's `SESSION_COOKIE`). */
export const SESSION_COOKIE = "__Host-comic_session";

/**
 * Server-side `/auth/me` fetch, deduped per request via React `cache()`.
 *
 * The root layout seeds the query cache with `me` (audit G7), and a
 * route-group layout (library / admin / settings) also needs it for its
 * redirect + shell props. Routing both through `getMe()` means they share a
 * single round-trip per request instead of fetching `/auth/me` twice.
 */
export const getMe = cache(() => apiGet<MeView>("/auth/me"));
