"use client";

import Link from "next/link";

import { LibrarySearch } from "@/components/LibrarySearch";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { useSavedViews } from "@/lib/api/queries";

import { PageActionsMenu } from "./PageActionsMenu";
import { PageHeading } from "./PageHeading";
import { SavedViewRail } from "./SavedViewRail";

/** Card-size bounds for the page rails. Mirrors the library-grid bounds
 *  so toggling between a page and a library grid feels consistent, but
 *  uses its own storage key so each surface remembers its own density. */
const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;

/** Page-rail dispatcher. Owns the page header + inline toolbar
 *  (search box, density toggle) and renders the saved views pinned to
 *  the supplied page below. Pinning, reorder, add, edit, delete all
 *  happen elsewhere — the page itself stays read-only.
 *
 *  Multi-page rails M5: same component drives both `/` (system Home)
 *  and `/pages/[slug]`. The caller resolves the page identity once
 *  (server-side in the route shell) and passes it down via props. */
export function PageRails({
  pageId,
  pageName,
  pageDescription,
  isSystem,
  showInSidebar,
}: {
  pageId: string;
  pageName: string;
  pageDescription: string | null;
  isSystem: boolean;
  showInSidebar: boolean;
}) {
  const pinnedQ = useSavedViews({ pinnedOn: pageId });
  // Per-page density key so customizing Marvel doesn't reset Home (or
  // vice versa). System page keeps the legacy global key so existing
  // users' Home density survives the M5 rename without a migration.
  const storageKey = isSystem
    ? "folio.home.cardSize"
    : `folio.page.cardSize.${pageId}`;
  const [cardSize, setCardSize] = useCardSize({
    storageKey,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  const items = pinnedQ.data?.items ?? [];

  return (
    <>
      <div
        className="flex flex-wrap items-center justify-between gap-4"
        // Header → first rail spacing also follows the density toggle.
        style={{ marginBottom: "var(--density-page-pad-y)" }}
      >
        <PageHeading
          pageId={pageId}
          pageName={pageName}
          pageDescription={pageDescription}
          isSystem={isSystem}
        />
        <div className="flex flex-wrap items-center gap-2">
          {/* On mobile the search lives in the topbar (see `MainShell`)
              so it doesn't push the rails down. Desktop keeps it inline
              in the page toolbar alongside the density toggle. */}
          <div className="hidden md:block">
            <LibrarySearch initial="" basePath="/" />
          </div>
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
          <PageActionsMenu
            pageId={pageId}
            pageDescription={pageDescription}
            isSystem={isSystem}
            showInSidebar={showInSidebar}
          />
        </div>
      </div>

      {pinnedQ.isLoading ? (
        <div className="text-muted-foreground py-12 text-sm">
          Loading views…
        </div>
      ) : items.length === 0 ? (
        <EmptyPinnedState isSystem={isSystem} />
      ) : (
        // Rail-to-rail spacing is driven by the density token defined
        // in `globals.css`: `--density-rail-gap` is 2.5rem in
        // comfortable mode and 1.25rem when compact is selected
        // under Settings → Theme.
        <div
          className="flex flex-col"
          style={{ gap: "var(--density-rail-gap)" }}
        >
          {items.map((view) => (
            <SavedViewRail key={view.id} view={view} cardSize={cardSize} />
          ))}
        </div>
      )}
    </>
  );
}

function EmptyPinnedState({ isSystem }: { isSystem: boolean }) {
  return (
    <div className="border-border/60 rounded-lg border border-dashed p-8">
      <h2 className="text-xl font-semibold tracking-tight">
        No pinned views yet
      </h2>
      <p className="text-muted-foreground mt-2 text-sm">
        {isSystem ? (
          <>
            Manage your saved views in{" "}
            <Link
              href="/settings/views"
              className="text-foreground font-medium underline-offset-4 hover:underline"
            >
              Settings → Saved views
            </Link>
            . You can create filter views, import CBL reading lists, and pick
            which ones show up here.
          </>
        ) : (
          <>
            Pin saved views to this page from{" "}
            <Link
              href="/settings/views"
              className="text-foreground font-medium underline-offset-4 hover:underline"
            >
              Settings → Saved views
            </Link>
            .
          </>
        )}
      </p>
    </div>
  );
}
