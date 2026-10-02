/**
 * Shared admin session for the docker-smoke suite (WP-8.5).
 *
 * The `setup` project runs this once, before every spec in the
 * `chromium-admin` project (`playwright.config.ts` `dependencies`):
 *
 *   register the first user through the real sign-in form (it becomes the
 *   admin) → create the library over the generated fixture
 *   (`fixtures/make-library.mjs`) → scan → wait until every fixture series
 *   exists → save the signed-in browser state to `ADMIN_STATE`.
 *
 * The admin specs (`reader-flow`, `relationship-review`) start from that
 * `storageState` instead of registering themselves, so neither races the
 * other for the first-user admin role and they can run in any order or in
 * parallel. Sessions are stateless JWT cookies (24 h access TTL) and CSRF is
 * double-submit, so sharing one cookie jar across contexts is safe.
 *
 * Idempotent: on a retry (or a rerun against a stack that already has the
 * admin) it signs in with the same fixed credentials instead of
 * registering again, and reuses an existing library.
 *
 * Outside the docker-smoke stack (no PLAYWRIGHT_BASE_URL at the Rust origin
 * with registration open) it writes an empty state and skips; the admin
 * specs skip themselves on the same check.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

import { test as setup, expect } from "@playwright/test";

import {
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
  ADMIN_STATE,
  FIXTURE_SERIES,
  csrfToken,
  json,
  registrationOpen,
} from "./support/admin";

setup(
  "register the admin and scan the fixture library",
  async ({ page, context, request }) => {
    setup.setTimeout(150_000);
    // Dependants load this file whether or not the setup ran.
    mkdirSync(dirname(ADMIN_STATE), { recursive: true });
    writeFileSync(ADMIN_STATE, JSON.stringify({ cookies: [], origins: [] }));
    setup.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );

    // 1. Sign in if the admin already exists (a retry), else register the
    //    first user through the real form — the hydration / form / cookie
    //    path a React or Radix bump can break.
    const login = await page.request.post("/auth/local/login", {
      data: { email: ADMIN_EMAIL, password: ADMIN_PASSWORD },
    });
    if (login.status() !== 200) {
      await page.goto("/sign-in");
      await expect(
        page.getByRole("heading", { name: "Sign in to Folio" }),
      ).toBeVisible();
      await page.getByRole("tab", { name: "Register" }).click();
      await page.getByLabel(/^Email$/).fill(ADMIN_EMAIL);
      await page.getByLabel(/^Password$/).fill(ADMIN_PASSWORD);
      await page.getByRole("button", { name: "Create account" }).click();
      await page.waitForURL((u) => u.pathname === "/");
    }
    const me = await json<{ email: string; role: string }>(
      page.request.get("/api/auth/me"),
    );
    expect(me.email).toBe(ADMIN_EMAIL);
    expect(me.role, "the setup user must be the first (admin) user").toBe(
      "admin",
    );

    // 2. One library over /library, scanned (the admin "New library" dialog's
    //    endpoint; cookie session + CSRF header).
    const libraries = await json<{ root_path: string }[]>(
      page.request.get("/api/libraries"),
    );
    if (!libraries.some((l) => l.root_path === "/library")) {
      const created = await page.request.post("/api/libraries", {
        headers: { "X-CSRF-Token": await csrfToken(context) },
        data: { name: "E2E Library", root_path: "/library", scan_now: true },
      });
      expect(created.status(), await created.text()).toBe(201);
    }

    // 3. The scan is async (apalis worker): wait for every fixture series.
    type SeriesList = { items: { name: string; year?: number | null }[] };
    await expect
      .poll(
        async () =>
          (await json<SeriesList>(page.request.get("/api/series?limit=50")))
            .items.length,
        {
          timeout: 90_000,
          message: `scan should produce ${FIXTURE_SERIES} series`,
        },
      )
      .toBe(FIXTURE_SERIES);

    await context.storageState({ path: ADMIN_STATE });
  },
);
