"use client";

import Link from "next/link";

import { LibrarySearch } from "@/components/LibrarySearch";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { useSavedViews } from "@/lib/api/queries";

import { SavedViewRail } from "./SavedViewRail";

/** Card-size bounds for the home rails. Mirrors the library-grid bounds
 *  so toggling between Home and a library grid feels consistent, but
 *  uses its own storage key so each surface remembers its own density. */
const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.home.cardSize";

/** Home-page rail dispatcher. Owns the page header + inline toolbar
 *  (search box, density toggle) and renders the user's pinned saved
 *  views below. Pin order, reorder, add, edit, and delete all live on
 *  `/settings/views` so the home page stays clean — no filter/sort
 *  controls either, since each rail is already a curated view. */
export function PinnedViewsHome() {
  const pinnedQ = useSavedViews({ pinned: true });
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
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
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Home</h1>
          <p className="text-muted-foreground mt-1 text-sm">
            Pinned saved views and reading lists.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <LibrarySearch initial="" basePath="/" />
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
        </div>
      </div>

      {pinnedQ.isLoading ? (
        <div className="text-muted-foreground py-12 text-sm">
          Loading views…
        </div>
      ) : items.length === 0 ? (
        <EmptyPinnedState />
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

function EmptyPinnedState() {
  return (
    <div className="border-border/60 rounded-lg border border-dashed p-8">
      <h2 className="text-xl font-semibold tracking-tight">
        No pinned views yet
      </h2>
      <p className="text-muted-foreground mt-2 text-sm">
        Manage your saved views in{" "}
        <Link
          href="/settings/views"
          className="text-foreground font-medium underline-offset-4 hover:underline"
        >
          Settings → Saved views
        </Link>
        . You can create filter views, import CBL reading lists, and pick which
        ones show up here.
      </p>
    </div>
  );
}
