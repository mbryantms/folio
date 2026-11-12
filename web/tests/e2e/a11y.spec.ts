/**
 * axe-core route sweep (§16.7; audit AC-5 / N-A9).
 *
 * Every reachable HTML route is loaded at desktop width and at phone width
 * (375 px, touch, mobile UA — the one-bar phone chrome) and asserted clean
 * under the shared WCAG 2.2 AA tag set (`support/axe.ts`). A few entries
 * open a phone bottom sheet or switch a tab first, so the sheets and the
 * non-default tabs are swept too. The three reader states stay in
 * `reader-flow.spec.ts` (it owns the reader state); the M7b relationship
 * surfaces stay in `relationship-review.spec.ts`. Not swept: the
 * `error.tsx` / `global-error.tsx` boundaries (no safe way to make a page
 * throw) and `/settings/activity`, whose whole body is a redirect to
 * `/settings/privacy`. Token pages are swept in their invalid-token or
 * landing state: the email-verification link lands on `/sign-in?verified=1`
 * (a bad token is a JSON 400, not a page), and a bad email-change link
 * redirects to the account settings with its error banner.
 *
 * Adding a route is one line in `ROUTES` (or `PUBLIC_ROUTES` for a page
 * that renders signed out). Dynamic segments come from the fixture library
 * the `setup` project scanned (`fixtures/make-library.mjs`), resolved once
 * per worker through the API — never hard-code a slug. The fixture's
 * Relay (2011) carries one character, team, location and story arc for
 * the entity landing pages. A collection, a reading list (an uploaded
 * `.cbl`) and a custom page are throwaway rows of the shared admin, created
 * here idempotently (reused by name) like the filter view.
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
  creatorSlug: string;
  publisherSlug: string;
  characterSlug: string;
  teamSlug: string;
  locationSlug: string;
  arcSlug: string;
  collectionId: string;
  readingListId: string;
  pageSlug: string;
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
  /** Load with `prefers-reduced-motion: reduce`, for a surface that would
   *  otherwise keep a finite animation running (an auto-advancing story):
   *  the axe pass waits for those to settle. */
  reducedMotion?: boolean;
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

/** Names of the throwaway rows the sweep creates (idempotent). */
const SWEEP_VIEW_NAME = "A11y sweep view";
const SWEEP_COLLECTION_NAME = "A11y sweep collection";
const SWEEP_LIST_NAME = "A11y sweep reading list";
const SWEEP_PAGE_NAME = "A11y sweep page";
const SWEEP_LIST_GROUP_NAME = "A11y sweep folder";

/** Entity names `fixtures/make-library.mjs` writes into Relay (2011). */
const FIXTURE_ENTITIES = {
  creators: "Ada Fixture",
  publishers: "Folio Test Comics",
  characters: "Captain Fixture",
  teams: "Fixture League",
  locations: "Fixture City",
  arcs: "Fixture Saga",
} as const;

/** The reading list the sweep uploads: both Relay volumes. */
const SWEEP_CBL = `<?xml version="1.0" encoding="utf-8"?>
<ReadingList><Name>${SWEEP_LIST_NAME}</Name><Books>
<Book Series="Relay" Number="1" Volume="2011" Year="2011" />
<Book Series="Relay" Number="1" Volume="2016" Year="2016" />
</Books></ReadingList>
`;

/** The Year in review story for the current year (empty or not, it is
 *  the same story frame). */
const THIS_YEAR = new Date().getUTCFullYear();

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
  // Where the email-verification link lands.
  { path: "/sign-in?verified=1", label: "sign-in, email verified banner" },
  // The service worker's offline fallback (static, precached).
  { path: "/offline.html", label: "offline fallback page" },
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
  // The first-run tour (replay link): the welcome card, then the last
  // step's starter reading-list shelf.
  {
    path: "/?tour=1",
    label: "home, first-run tour open",
    prepare: async (page) => {
      await expect(
        page.getByRole("dialog", { name: "Welcome to Folio" }),
      ).toBeVisible();
    },
  },
  {
    path: "/?tour=1",
    label: "home, first-run tour starter lists",
    prepare: async (page) => {
      await expect(
        page.getByRole("dialog", { name: "Welcome to Folio" }),
      ).toBeVisible();
      for (let i = 0; i < 7; i++) await page.keyboard.press("ArrowRight");
      await expect(
        page.getByRole("dialog", { name: "Pick a place to start" }),
      ).toBeVisible();
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
  },
  {
    path: (ids) => `/series/${ids.seriesSlug}/issues/${ids.issueSlug}`,
    label: "issue detail",
  },
  { path: "/search?q=relay", label: "search results" },
  { path: "/bookmarks", label: "bookmarks" },
  { path: "/downloads", label: "downloads" },
  { path: "/collections", label: "collections" },
  { path: "/lists", label: "reading lists" },
  { path: "/log", label: "reading log" },
  { path: "/log/activity", label: "reading log, activity" },
  { path: "/log/stats", label: "reading log, stats" },
  { path: "/log/sessions", label: "reading log, sessions" },
  {
    path: `/log/year/${THIS_YEAR}`,
    label: "reading log, year in review",
    reducedMotion: true,
  },
  { path: "/views/continue-reading", label: "saved view, system rail" },
  { path: (ids) => `/views/${ids.viewId}`, label: "saved view, filter" },
  {
    path: (ids) => `/views/${ids.collectionId}`,
    label: "collection detail",
  },
  {
    path: (ids) => `/views/${ids.readingListId}`,
    label: "reading list detail",
  },
  { path: (ids) => `/pages/${ids.pageSlug}`, label: "custom page" },
  { path: "/creators", label: "creators index" },
  {
    path: (ids) => `/creators/${ids.creatorSlug}`,
    label: "creator detail",
  },
  { path: "/publishers", label: "publishers index" },
  {
    path: (ids) => `/publishers/${ids.publisherSlug}`,
    label: "publisher detail",
  },
  { path: "/characters", label: "characters index" },
  {
    path: (ids) => `/characters/${ids.characterSlug}`,
    label: "character detail",
  },
  { path: "/teams", label: "teams index" },
  { path: (ids) => `/teams/${ids.teamSlug}`, label: "team detail" },
  { path: "/locations", label: "locations index" },
  {
    path: (ids) => `/locations/${ids.locationSlug}`,
    label: "location detail",
  },
  { path: "/arcs", label: "story arcs index" },
  { path: (ids) => `/arcs/${ids.arcSlug}`, label: "story arc detail" },
  { path: "/a11y-sweep-no-such-page", label: "not found" },
  // ----- settings -----
  { path: "/settings", label: "settings index" },
  { path: "/settings/reading", label: "settings, reading" },
  { path: "/settings/keybinds", label: "settings, keyboard shortcuts" },
  { path: "/settings/theme", label: "settings, theme" },
  { path: "/settings/privacy", label: "settings, privacy" },
  { path: "/settings/downloads", label: "settings, downloads" },
  { path: "/settings/views", label: "settings, views" },
  {
    path: "/settings/views?type=reading-lists",
    label: "settings, views, reading lists table",
  },
  { path: "/settings/pages", label: "settings, pages" },
  { path: "/settings/sidebar", label: "settings, sidebar" },
  {
    path: "/settings/account",
    label: "settings, account",
  },
  { path: "/settings/api-tokens", label: "settings, app passwords" },
  // A bad email-change confirmation link: the server redirects to the
  // account settings with the "didn't work" banner.
  {
    path: "/auth/local/confirm-email-change?token=e2e-sweep-token",
    label: "settings, account, email change link invalid",
  },
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
    path: (ids) => `/admin/libraries/${ids.librarySlug}/settings`,
    label: "admin, library settings",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/duplicates`,
    label: "admin, library duplicates",
  },
  {
    path: (ids) => `/admin/libraries/${ids.librarySlug}/removed`,
    label: "admin, library removed",
  },
  // Merged pages' second views (`?history=`, `?view=`) are their own rows.
  { path: "/admin/scans", label: "admin, scans" },
  { path: "/admin/scans?history=runs", label: "admin, scans, every scan" },
  { path: "/admin/findings", label: "admin, library events" },
  { path: "/admin/users", label: "admin, users" },
  { path: (ids) => `/admin/users/${ids.userId}`, label: "admin, user detail" },
  {
    // The age-cap picker, the Hide unrated switch and the per-library
    // "what this hides" line (U-09) live on this tab.
    path: (ids) => `/admin/users/${ids.userId}`,
    label: "admin, user library access",
    prepare: async (page) => {
      await page.getByRole("tab", { name: "Library access" }).click();
      await expect(page.getByRole("switch").first()).toBeVisible();
    },
  },
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
  { path: "/admin/activity?view=log", label: "admin, server log" },
  { path: "/admin/auth", label: "admin, auth config" },
  { path: "/admin/email", label: "admin, email" },
  { path: "/admin/starter-lists", label: "admin, starter lists" },
  { path: "/admin/background-work?view=jobs", label: "admin, job queue" },
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
  type SeriesList = { items: { id: string; slug: string; name: string }[] };
  const allSeries = (
    await json<SeriesList>(page.request.get("/api/series?limit=50"))
  ).items;
  const series = allSeries.find((s) => s.name === "Test Series");
  const relay = allSeries.find((s) => s.name === "Relay");
  expect(relay, "fixture series Relay").toBeTruthy();
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
  const csrf = { "X-CSRF-Token": await csrfToken(context) };

  // A collection holding one series, so the grid (not the empty state)
  // is swept. The entry is added only with a new collection.
  let collection = (
    await json<{ id: string; name: string }[]>(
      page.request.get("/api/me/collections"),
    )
  ).find((c) => c.name === SWEEP_COLLECTION_NAME);
  if (!collection) {
    collection = await json<{ id: string; name: string }>(
      page.request.post("/api/me/collections", {
        headers: csrf,
        data: { name: SWEEP_COLLECTION_NAME },
      }),
    );
    await json(
      page.request.post(`/api/me/collections/${collection.id}/entries`, {
        headers: csrf,
        data: { entry_kind: "series", ref_id: relay!.id },
      }),
    );
  }

  // A reading list: upload the `.cbl`, then save it as a reading-list view
  // (the two calls the import dialog makes).
  let readingList = (
    await json<ViewList>(page.request.get("/api/me/saved-views"))
  ).items.find((v) => v.name === SWEEP_LIST_NAME);
  if (!readingList) {
    const list = await json<{ id: string }>(
      page.request.post("/api/me/cbl-lists/upload", {
        headers: csrf,
        multipart: {
          file: {
            name: "a11y-sweep.cbl",
            mimeType: "application/xml",
            buffer: Buffer.from(SWEEP_CBL, "utf8"),
          },
          name: SWEEP_LIST_NAME,
        },
      }),
    );
    readingList = await json<{ id: string; name: string }>(
      page.request.post("/api/me/saved-views", {
        headers: csrf,
        data: { kind: "cbl", name: SWEEP_LIST_NAME, cbl_list_id: list.id },
      }),
    );
  }

  // A reading-list group holding it, so the lists table shows a folder
  // row with its roll-ups as well as a list row.
  type GroupList = { groups: { name: string }[] };
  const listGroups = await json<GroupList>(
    page.request.get("/api/me/view-groups?scope=cbl"),
  );
  if (!listGroups.groups.some((g) => g.name === SWEEP_LIST_GROUP_NAME)) {
    await json(
      page.request.post("/api/me/view-groups", {
        headers: csrf,
        data: {
          scope: "cbl",
          name: SWEEP_LIST_GROUP_NAME,
          view_ids: [readingList.id],
        },
      }),
    );
  }

  // A custom page (nothing pinned: its empty state).
  type PageList = { items: { slug: string; name: string }[] };
  let customPage = (
    await json<PageList>(page.request.get("/api/me/pages"))
  ).items.find((p) => p.name === SWEEP_PAGE_NAME);
  if (!customPage) {
    customPage = await json<{ slug: string; name: string }>(
      page.request.post("/api/me/pages", {
        headers: csrf,
        data: { name: SWEEP_PAGE_NAME },
      }),
    );
  }

  return {
    seriesSlug: series!.slug,
    issueSlug: issue.slug,
    librarySlug: library.slug,
    userId: me.id,
    viewId: view.id,
    creatorSlug: await entitySlug(page, "creators", FIXTURE_ENTITIES.creators),
    publisherSlug: await entitySlug(
      page,
      "publishers",
      FIXTURE_ENTITIES.publishers,
    ),
    characterSlug: await entitySlug(
      page,
      "characters",
      FIXTURE_ENTITIES.characters,
    ),
    teamSlug: await entitySlug(page, "teams", FIXTURE_ENTITIES.teams),
    locationSlug: await entitySlug(
      page,
      "locations",
      FIXTURE_ENTITIES.locations,
    ),
    arcSlug: await entitySlug(page, "arcs", FIXTURE_ENTITIES.arcs),
    collectionId: collection.id,
    readingListId: readingList.id,
    pageSlug: customPage.slug,
  };
}

/** Slug of a fixture entity from its browse index. Entity rows (and a
 *  creator's slug) are minted by the post-scan rollup, which can finish
 *  after the series exist, so poll. Creators list as `person`; the other
 *  kinds as `name`. */
async function entitySlug(
  page: Page,
  kind: keyof typeof FIXTURE_ENTITIES,
  name: string,
): Promise<string> {
  type Row = { slug?: string | null; name?: string; person?: string };
  let slug: string | null | undefined;
  await expect
    .poll(
      async () => {
        const rows = (
          await json<{ items: Row[] }>(
            page.request.get(`/api/${kind}?limit=100`),
          )
        ).items;
        slug = rows.find((r) => (r.person ?? r.name) === name)?.slug;
        return slug ?? "";
      },
      { timeout: 60_000, message: `fixture ${kind} "${name}" has a slug` },
    )
    .not.toBe("");
  return slug!;
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
      // The worker's first signed-in test also resolves the ids, which
      // may wait on the post-scan entity rollup (`entitySlug`).
      if (withIds && !idsPromise) test.setTimeout(120_000);
      const ids = withIds ? await fixtureIds(page, context) : null;
      const path =
        typeof route.path === "function" ? route.path(ids!) : route.path;
      if (route.reducedMotion) {
        await page.emulateMedia({ reducedMotion: "reduce" });
      }
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
