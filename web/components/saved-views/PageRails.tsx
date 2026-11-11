"use client";

import Link from "next/link";
import { RailError } from "@/components/saved-views/RailError";
import { RailsSkeleton } from "@/components/skeletons/LoadingShapes";
import { FolderPlus, LibraryBig } from "lucide-react";

import {
  CoverSizeFields,
  CoverSizeOptions,
} from "@/components/library/CoverSizeOptions";
import { PhoneControlsBar } from "@/components/library/PhoneControlsBar";
import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { Button } from "@/components/ui/button";
import { useLibraryList, useMe, useSavedViews } from "@/lib/api/queries";

import { LazyRail } from "./LazyRail";
import { PageActionsMenu } from "./PageActionsMenu";
import { PageHeading } from "./PageHeading";

/** Page-rail dispatcher. Owns the page heading and the pinned control
 *  row (cover size + page actions) and renders the saved views pinned to
 *  the supplied page below. Pinning, reorder, add, edit, delete all
 *  happen elsewhere — the page itself stays read-only.
 *
 *  Cover size is the app-wide one (lib/library/cover-size.ts): the page
 *  holds no size state and passes none down. The control here sets the
 *  same size as the library's, and the rails follow it through CSS.
 *
 *  Chrome (docs/dev/phone-layout.md, rule 3): a rails page is a long
 *  stack of rails whose density you tune while browsing, so its two
 *  controls pin under the topbar at every width. From `sm` up the
 *  heading (title + description) stays in flow and a `StickyToolbarRow`
 *  beneath it carries `CoverSizeOptions` + `PageActionsMenu`, right
 *  aligned. Below `sm` the heading is hidden and the title rides in the
 *  sticky `PhoneControlsBar`'s leading slot beside View options (cover
 *  size as a sheet) and `⋯`; the description, when set, follows the bar
 *  in flow. Inline title editing is a `sm`-and-up affordance — phones
 *  rename through `⋯`.
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

  const items = pinnedQ.data?.items ?? [];

  const pageActions = (
    <PageActionsMenu
      pageId={pageId}
      pageDescription={pageDescription}
      isSystem={isSystem}
      showInSidebar={showInSidebar}
    />
  );

  return (
    <>
      {/* `sm` and up: the heading in flow, then the pinned row. Row →
          first rail spacing follows the density toggle. */}
      <div className="flex max-sm:hidden">
        <PageHeading
          pageId={pageId}
          pageName={pageName}
          pageDescription={pageDescription}
          isSystem={isSystem}
        />
      </div>
      {/* Universal search lives in the topbar (see `MainShell`) at every
          width, so no inline search input here: the row is the
          cover-size control + page actions. */}
      <StickyToolbarRow className="mb-(--density-page-pad-y) justify-end">
        <CoverSizeOptions />
        {pageActions}
      </StickyToolbarRow>
      {/* Phone (< sm): title · View options · ⋯ in one sticky band at the
          top of the page; the description, when set, follows in flow. */}
      <PhoneControlsBar
        pageTop
        className={pageDescription ? "mb-3" : "mb-(--density-page-pad-y)"}
        leading={
          <h1 className="min-w-0 truncate text-xl font-semibold tracking-tight">
            {pageName}
          </h1>
        }
        more={pageActions}
        menu={() => <CoverSizeFields fieldId="page-phone-card-size" />}
      />
      {pageDescription ? (
        <p className="text-muted-foreground mb-(--density-page-pad-y) text-xs sm:hidden">
          {pageDescription}
        </p>
      ) : null}

      {pinnedQ.isLoading ? (
        <RailsSkeleton
          rails={3}
          card={isSystem ? "issue" : "series"}
          label="Loading views…"
        />
      ) : pinnedQ.isError && !pinnedQ.data ? (
        // A failed fetch is not "nothing pinned" (post-audit T2.4).
        <RailError
          message="Couldn't load your pinned views."
          onRetry={() => void pinnedQ.refetch()}
          className="py-12"
        />
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
          {items.map((view, idx) => (
            <LazyRail
              key={view.id}
              view={view}
              // Prioritize the first (above-the-fold) rail's covers — the
              // LCP element lives there; the rest stay lazy.
              priority={idx === 0}
            />
          ))}
        </div>
      )}
    </>
  );
}

function EmptyPinnedState({ isSystem }: { isSystem: boolean }) {
  // First-run signposting (audit UX-1): a fresh sign-in lands here with
  // zero pinned views, so the system Home must route the user to their
  // actual content (or to creating some) instead of dead-ending on a
  // Settings link. Only the system page probes libraries/role — custom
  // pages keep the lightweight pin hint.
  const me = useMe({ enabled: isSystem });
  const librariesQ = useLibraryList({ enabled: isSystem });
  const isAdmin = me.data?.role === "admin";
  const hasLibraries = (librariesQ.data?.length ?? 0) > 0;

  if (!isSystem) {
    return (
      <div className="border-border/60 rounded-lg border border-dashed p-8">
        <h2 className="text-xl font-semibold tracking-tight">
          No pinned views yet
        </h2>
        <p className="text-muted-foreground mt-2 text-sm">
          Pin saved views to this page from{" "}
          <Link
            href="/settings/views"
            className="text-foreground font-medium underline-offset-4 hover:underline"
          >
            Settings → Saved views
          </Link>
          .
        </p>
      </div>
    );
  }

  return (
    <div className="border-border/60 rounded-lg border border-dashed p-8">
      <h2 className="text-xl font-semibold tracking-tight">
        {hasLibraries ? "Welcome to Folio" : "No libraries yet"}
      </h2>
      <p className="text-muted-foreground mt-2 text-sm">
        {hasLibraries
          ? "Your comics are ready — browse your library, or pin saved views here to build a custom home."
          : isAdmin
            ? "Create a library pointing at your comics folder, run a scan, and your collection shows up here."
            : "Ask your server admin to grant you access to a library, then your comics will show up here."}
      </p>
      <div className="mt-4 flex flex-wrap items-center gap-2">
        {hasLibraries ? (
          <Button asChild>
            <Link href="/?library=all">
              <LibraryBig />
              Browse your library
            </Link>
          </Button>
        ) : null}
        {isAdmin && !hasLibraries ? (
          <Button asChild>
            <Link href="/admin/libraries">
              <FolderPlus />
              Create a library
            </Link>
          </Button>
        ) : null}
        <Button asChild variant="outline">
          <Link href="/settings/views">Manage saved views</Link>
        </Button>
      </div>
    </div>
  );
}
