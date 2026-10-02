/**
 * Shared admin session for the docker-smoke specs (WP-8.5): constants and
 * helpers used by `admin.setup.ts` (which creates the session) and the
 * `chromium-admin` specs (which start from it). See `admin.setup.ts`.
 */
import { fileURLToPath } from "node:url";

import {
  expect,
  type APIRequestContext,
  type APIResponse,
  type BrowserContext,
} from "@playwright/test";

/** Signed-in admin browser state, written by the `setup` project. Kept out
 *  of `test-results/` (uploaded as a CI artifact) and gitignored. */
export const ADMIN_STATE = fileURLToPath(
  new URL("../../../playwright/.auth/admin.json", import.meta.url),
);

/** Fixed so a retried setup can sign back in instead of registering a
 *  second (non-admin) user. Only ever used on a throwaway smoke stack. */
export const ADMIN_EMAIL = "e2e-admin@example.test";
export const ADMIN_PASSWORD = "correct-horse-battery-staple";

/** Series folders `fixtures/make-library.mjs` writes. */
export const FIXTURE_SERIES = 3;

/** True when the target is the Rust origin with open local registration
 *  (the docker-smoke stack); the admin specs skip otherwise. */
export async function registrationOpen(
  request: APIRequestContext,
): Promise<boolean> {
  const cfg = await request.get("/api/auth/config");
  return (
    cfg.ok() &&
    !!((await cfg.json()) as { registration_open?: boolean }).registration_open
  );
}

export async function csrfToken(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies()).find(
    (c) => c.name === "__Host-comic_csrf",
  );
  expect(cookie, "CSRF cookie set on the admin session").toBeTruthy();
  return cookie!.value;
}

export async function json<T>(req: Promise<APIResponse>): Promise<T> {
  const res = await req;
  expect(res.ok(), `${res.url()} → ${res.status()}`).toBeTruthy();
  return (await res.json()) as T;
}
