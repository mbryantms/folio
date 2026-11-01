/**
 * axe-core route sweep (§16.7; audit AC-5 / N-A9).
 *
 * Every reachable HTML route is loaded at desktop width and at phone width
 * (375 px, touch, mobile UA — the one-bar phone chrome) and asserted clean
 * under the shared WCAG 2.2 AA tag set (`support/axe.ts`). A few entries
 * open a phone bottom sheet or switch a tab first, so the sheets and the
 * non-default tabs are swept too. The three reader states stay in
 * `reader-flow.spec.ts` (it owns the reader state); the M7b relationship
 * surfaces stay in `relationship-review.spec.ts`.
 *
 * Adding a route is one line in `ROUTES` (or `PUBLIC_ROUTES` for a page
 * that renders signed out). Dynamic segments come from the fixture library
 * the `setup` project scanned (`fixtures/make-library.mjs`), resolved once
 * per worker through the API — never hard-code a slug.
 *
 * Runs in the `chromium-admin` project: the signed-in entries start from
 * the shared admin session (`admin.setup.ts`); the public entries drop the
 * session with an empty `storageState` (the sign-in page bounces a signed-in
 * user to `/`). Nothing here registers a user. Needs PLAYWRIGHT_BASE_URL at
 * the Rust origin with registration open (compose.test.yml) for the
 * signed-in half; the public half runs anywhere the pages render.
 *
 * Per-route exclusions (`exclude` selectors, or `disableRules` rule ids)
 * are for violations that need a design decision, each with a comment
 * naming the finding it carries; fix the control instead whenever you can.
 */
import { test, expect, type BrowserContext, type Page } from "@playwright/test";

import { csrfToken, json, registrationOpen } from "./support/admin";
import { expectNoAxeViolations } from "./support/axe";

/** Fixture ids resolved from the scanned library (once per worker). */
type Ids = {
  seriesSlug: string;
  issueSlug: string;
  librarySlug: string;
  userId: string;
  viewId: string;
};

type Width = "desktop" | "phone";

type Route = {
  /** Path (unprefixed; the locale segment is internal), or a builder over
   *  the fixture ids for dynamic segments. */
  path: string | ((ids: Ids) => string);
  /** Stable label for the test title and the axe failure message. */
  label: string;
  /** Interaction before the axe pass: open a sheet, switch a tab. */
  prepare?: (page: Page) => Promise<void>;
  /** Sweep at one width only (phone-only sheets, desktop-only chrome). */
  only?: Width;
  /** Skip unless local registration is open (the Register tab is disabled
   *  otherwise). */
  needsRegistration?: boolean;
  /** Elements left out of the axe pass on this route (CSS selectors), with
   *  the finding in a comment. Preferred over `disableRules`: every rule
   *  keeps running on the rest of the page. */
  exclude?: string[];
  /** axe rule ids excluded on this route, with the finding in a comment. */
  disableRules?: string[];
};

const PHONE = {
  viewport: { width: 375, height: 812 },
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
};

/** Name of the throwaway filter view the sweep creates (idempotent). */
const SWEEP_VIEW_NAME = "A11y sweep view";

const PUBLIC_ROUTES: Route[] = [
  { path: "/sign-in", label: "sign-in" },
  {
    path: "/sign-in",
    label: "register tab",
    needsRegistration: true,
    prepare: async (page) => {
      await page.getByRole("tab", { name: "Register" }).click();
      await expect(
        page.getByRole("button", { name: "Create account" }),
      ).toBeVisible();
    },
  },
  { path: "/forgot-password", label: "forgot password" },
  { path: "/reset-password", label: "reset password, no token" },
  {
    path: "/reset-password?token=e2e-sweep-token",
    label: "reset password, token form",
  },
];

const ROUTES: Route[] = [
  // ----- library shell -----
  { path: "/", label: "home" },
  {
    path: "/",
    label: "home, phone nav sheet open",
    only: "phone",
    prepare: async (page) => {
      await page.getByRole("button", { name: "More navigation" }).click();
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  { path: "/browse", label: "browse hub" },
  { path: "/?library=all", label: "library grid" },
  {
    path: "/?library=all",
    label: "library grid, phone sort sheet open",
    only: "phone",
    prepare: async (page) => {
      await page.getByRole("button", { name: /^Sort:/ }).click();
      await expect(page.getByRole("dialog", { name: "Sort by" })).toBeVisible();
    },
  },
  {
    path: "/?library=all",
    label: "library grid, phone view-options sheet open",
    only: "phone",
    prepare: async (page) => {
      await page.getByRole("button", { name: "View options" }).click();
      await expect(
        page.getByRole("dialog", { name: "View options" }),
      ).toBeVisible();
    },
  },
  { path: (ids) => `/series/${ids.seriesSlug}`, label: "series detail" },
  {
    path: (ids) => `/series/${ids.seriesSlug}?tab=credits`,
    label: "series detail, Credits tab",
  },
  {
    path: (ids) => `/series/${ids.seriesSlug}?tab=details`,
    label: "series detail, Details tab",
  },
  {
    path: (ids) => `/series/${ids.seriesSlug}?tab=related`,
    label: "series detail, Related tab",
  },
  {
    path: (ids) => `/series/${ids.seriesSlug}?tab=metadata`,
    label: "series detail, Metadata tab",
    // Finding: the "Needs metadata" tier badge is `text-destructive` on
    // `bg-destructive/15` — 3.3:1 in the dark theme; the dark
    // `--destructive` token (3.7:1 as plain text on a card) needs a
    // palette decision (see PR body). Only the badge is excluded.
    exclude: ['span.text-destructive[class*="bg-destructive/15"]'],
  },
  {
    path: (ids) => `/series/${ids.seriesSlug}/issues/${ids.issueSlug}`,
    label: "issue detail",
  },
  { path: "/search?q=relay", label: "search results" },
  { path: "/bookmarks", label: "bookmarks" },
  { path: "/downloads", label: "downloads" },
  { path: "/collections", label: "collections" },
  { path: "/log", label: "reading log" },
  { path: "/log/activity", label: "reading log, activity" },
  { path: "/views/continue-reading", label: "saved view, system rail" },
  { path: (ids) => `/views/${ids.viewId}`, label: "saved view, filter" },
  { path: "/creators", label: "creators index" },
  { path: "/publishers", label: "publishers index" },
  { path: "/characters", label: "characters index" },
  { path: "/teams", label: "teams index" },
  { path: "/locations", label: "locations index" },
  { path: "/arcs", label: "story arcs index" },
  // ----- settings -----
  { path: "/settings", label: "settings index" },
  { path: "/settings/reading", label: "settings, reading" },
  { path: "/settings/keybinds", label: "settings, keyboard shortcuts" },
  { path: "/settings/theme", label: "settings, theme" },
  {
    path: "/settings/activity",
    label: "settings, reading stats",
    // Finding: the vendored Button `destructive` variant ("Delete
    // history") is `text-destructive` on `dark:bg-destructive/20` —
    // 3.2:1 in the dark theme; a palette / variant decision (see PR body).
    exclude: ['[data-variant="destructive"]'],
  },
  { path: "/settings/downloads", label: "settings, downloads" },
  { path: "/settings/views", label: "settings, views" },
  { path: "/settings/pages", label: "settings, pages" },
  { path: "/settings/sidebar", label: "settings, sidebar" },
  {
    path: "/settings/account",
    label: "settings, account",
    // Same finding as settings, reading stats: the "Delete account"
    // button is the vendored `destructive` variant, 3.2:1 in the dark
    // theme until the `--destructive` palette decision lands.
    exclude: ['[data-variant="destructive"]'],
  },
  { path: "/settings/api-tokens", label: "settings, app passwords" },
  // ----- admin -----
  { path: "/admin", label: "admin dashboard" },
  {
    path: "/admin",
    label: "admin dashboard, phone nav sheet open",
    only: "phone",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Toggle navigation" }).click();
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  { path: "/admin/background-work", label: "admin, background work" },
  { path: "/admin/libraries", label: "admin, libraries" },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}`,
    label: "admin, library overview",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/scan`,
    label: "admin, library scan",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/health`,
    label: "admin, library health",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/history`,
    label: "admin, library history",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/settings`,
    label: "admin, library settings",
    // Finding: the Danger zone heading and its outline "Delete" button are
    // `text-destructive` on a card — 3.7:1 / 3.5:1 in the dark theme (the
    // same `--destructive` palette decision as above). Only those two.
    exclude: ["h3.text-destructive", "button.text-destructive"],
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/thumbnails`,
    label: "admin, library thumbnails",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/duplicates`,
    label: "admin, library duplicates",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/removed`,
    label: "admin, library removed",
  },
  { path: "/admin/scan-dashboard", label: "admin, scan dashboard" },
  { path: "/admin/findings", label: "admin, library events" },
  { path: "/admin/users", label: "admin, users" },
  { path: (ids) => `/admin/users/${ids.userId}`, label: "admin, user detail" },
  { path: "/admin/stats", label: "admin, stats" },
  { path: "/admin/metadata", label: "admin, metadata" },
  { path: "/admin/duplicates", label: "admin, duplicates" },
  { path: "/admin/relationships", label: "admin, relationships" },
  {
    path: (ids) => `/admin/series/${ids.seriesSlug}`,
    label: "admin, series identity",
  },
  { path: "/admin/server", label: "admin, server info" },
  { path: "/admin/activity", label: "admin, server activity" },
  { path: "/admin/logs", label: "admin, server log" },
  { path: "/admin/auth", label: "admin, auth config" },
  { path: "/admin/email", label: "admin, email" },
  { path: "/admin/queue", label: "admin, queue" },
  {
    path: "/admin/api-docs",
    label: "admin, API reference",
    // Finding: the embedded Scalar reference ships its own palette
    // (HTTP-method badges at 1.7–4:1, search placeholder 3.2:1). Third-
    // party styles; recolouring them is a theming decision (see PR body).
    // Folio's admin chrome around it is still checked.
    exclude: ['[data-slot="api-reference"]'],
  },
];

/** Resolve the fixture ids once per worker; later tests reuse the promise.
 *  A failed resolve is dropped so the next test (or a retry) tries again
 *  instead of inheriting one transient error. */
let idsPromise: Promise<Ids> | undefined;
function fixtureIds(page: Page, context: BrowserContext): Promise<Ids> {
  idsPromise ??= resolveIds(page, context).catch((err: unknown) => {
    idsPromise = undefined;
    throw err;
  });
  return idsPromise;
}

async function resolveIds(page: Page, context: BrowserContext): Promise<Ids> {
  type SeriesList = { items: { slug: string; name: string }[] };
  const series = (
    await json<SeriesList>(page.request.get("/api/series?limit=50"))
  ).items.find((s) => s.name === "Test Series");
  expect(series, "fixture series Test Series").toBeTruthy();
  type IssueList = { items: { slug: string }[] };
  const issue = (
    await json<IssueList>(
      page.request.get(`/api/series/${series!.slug}/issues`),
    )
  ).items[0];
  expect(issue, "fixture issue").toBeTruthy();
  const library = (
    await json<{ slug: string }[]>(page.request.get("/api/libraries"))
  )[0];
  expect(library, "fixture library").toBeTruthy();
  const me = await json<{ id: string }>(page.request.get("/api/auth/me"));

  // A filter view of our own, so the DSL-driven grid is swept (the system
  // rails are a different component). Reused when a retry finds it.
  type ViewList = { items: { id: string; name: string }[] };
  let view = (
    await json<ViewList>(page.request.get("/api/me/saved-views"))
  ).items.find((v) => v.name === SWEEP_VIEW_NAME);
  if (!view) {
    view = await json<{ id: string; name: string }>(
      page.request.post("/api/me/saved-views", {
        headers: { "X-CSRF-Token": await csrfToken(context) },
        data: {
          kind: "filter_series",
          name: SWEEP_VIEW_NAME,
          filter: {
            match_mode: "all",
            conditions: [{ field: "name", op: "contains", value: "Relay" }],
          },
        },
      }),
    );
  }
  return {
    seriesSlug: series!.slug,
    issueSlug: issue.slug,
    librarySlug: library.slug,
    userId: me.id,
    viewId: view.id,
  };
}

/** Load a route and wait until it is the surface under test: network quiet
 *  and no loading skeleton left (a skeleton is a placeholder, not the page). */
async function open(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await page.waitForLoadState("networkidle");
  await page
    .locator('[data-slot="skeleton"]')
    .first()
    .waitFor({ state: "detached", timeout: 10_000 })
    .catch(() => {
      /* a skeleton that stays is still a legitimate rendered state */
    });
}

function sweep(routes: Route[], width: Width, withIds: boolean) {
  for (const route of routes) {
    if (route.only && route.only !== width) continue;
    test(`${route.label} is axe-clean (${width})`, async ({
      page,
      context,
    }) => {
      if (route.needsRegistration) {
        test.skip(
          !(await registrationOpen(page.request)),
          "the Register tab is disabled unless local registration is open",
        );
      }
      const ids = withIds ? await fixtureIds(page, context) : null;
      const path =
        typeof route.path === "function" ? route.path(ids!) : route.path;
      await open(page, path);
      await route.prepare?.(page);
      await expectNoAxeViolations(page, `${route.label} (${width})`, {
        exclude: route.exclude,
        disableRules: route.disableRules,
      });
    });
  }
}

test.describe("Accessibility sweep", () => {
  test.describe("public routes", () => {
    // No session: the sign-in page bounces a signed-in user to `/`.
    test.use({ storageState: { cookies: [], origins: [] } });
    test.describe("desktop", () => {
      sweep(PUBLIC_ROUTES, "desktop", false);
    });
    test.describe("phone", () => {
      test.use(PHONE);
      sweep(PUBLIC_ROUTES, "phone", false);
    });
  });

  test.describe("signed-in routes", () => {
    test.beforeEach(async ({ request }) => {
      test.skip(
        !(await registrationOpen(request)),
        "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
      );
    });
    test.describe("desktop", () => {
      sweep(ROUTES, "desktop", true);
    });
    test.describe("phone", () => {
      test.use(PHONE);
      sweep(ROUTES, "phone", true);
    });
  });
});
