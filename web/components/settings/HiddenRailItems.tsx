"use client";

import Link from "next/link";
import { EyeOff, ListOrdered } from "lucide-react";

import { Cover } from "@/components/Cover";
import { EmptyState } from "@/components/folio/empty-state";
import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useRestoreRailItem } from "@/lib/api/mutations";
import { useRailDismissals } from "@/lib/api/queries";
import type { DismissalRail, RailDismissalView } from "@/lib/api/types";
import { timeAgo } from "@/lib/sessions";
import { issueUrl, seriesUrl } from "@/lib/urls";

import { SettingsSection } from "./SettingsSection";

const TITLE = "Hidden from rails";
const DESCRIPTION =
  "Items you hid from the home rails. Restore one to put it back; reading it again also brings it back on its own.";

const RAIL_LABELS: Record<DismissalRail, string> = {
  continue_reading: "Continue reading",
  recent_history: "Recently read",
  on_deck: "On deck",
  random_picks: "Random picks",
  similar_series: "Similar series",
  opds_continue_reading: "OPDS Continue reading",
};

/** "Continue reading, Recently read" — the rails a dismissal hides from. */
export function hiddenFromLabel(rails: DismissalRail[]): string {
  return rails.map((r) => RAIL_LABELS[r]).join(", ");
}

/** Link to the target, or `null` when it has none (a reading list that
 *  no saved view wraps). */
function targetHref(item: RailDismissalView): string | null {
  const t = item.target;
  switch (item.target_kind) {
    case "issue":
      return t.slug && t.series_slug ? issueUrl(t.series_slug, t.slug) : null;
    case "series":
      return t.slug ? seriesUrl(t.slug) : null;
    case "cbl":
      return t.saved_view_id
        ? `/views/${encodeURIComponent(t.saved_view_id)}`
        : null;
  }
}

/**
 * Settings → Pages "Hidden from rails": every dismissal the caller has,
 * newest first, walked with an IntersectionObserver sentinel. Targets the
 * caller can no longer see are withheld by the server and only counted.
 */
export function HiddenRailItems() {
  const query = useRailDismissals();
  const restore = useRestoreRailItem();

  if (query.isLoading) {
    return (
      <SettingsSection title={TITLE} description={DESCRIPTION}>
        <Skeleton className="h-24 w-full" />
      </SettingsSection>
    );
  }
  // A failed *next* page keeps the loaded rows (v5 keeps `data`); only a
  // failed first page replaces the list.
  if (!query.data) {
    return (
      <SettingsSection title={TITLE} description={DESCRIPTION}>
        <p className="text-destructive text-sm">Failed to load hidden items.</p>
      </SettingsSection>
    );
  }

  const items = query.data.pages.flatMap((p) => p.items);
  const unavailable = query.data.pages[0]?.unavailable ?? 0;

  return (
    <SettingsSection title={TITLE} description={DESCRIPTION}>
      {items.length === 0 ? (
        <EmptyState
          size="sm"
          icon={EyeOff}
          title="Nothing hidden"
          description="Use Hide from rail on a home-rail card to tuck it away."
          className="border-border/60"
        />
      ) : (
        <ul className="divide-border/60 divide-y" aria-label={TITLE}>
          {items.map((item) => (
            <HiddenRow
              key={item.id}
              item={item}
              disabled={
                restore.isPending &&
                restore.variables?.target_kind === item.target_kind &&
                restore.variables?.target_id === item.target_id
              }
              onRestore={() =>
                restore.mutate({
                  target_kind: item.target_kind,
                  target_id: item.target_id,
                })
              }
            />
          ))}
        </ul>
      )}
      <InfiniteSentinel query={query} />
      {query.isFetchNextPageError ? (
        <div className="mt-2 flex items-center justify-center gap-2">
          <p className="text-destructive text-xs">Couldn’t load more.</p>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void query.fetchNextPage()}
          >
            Retry
          </Button>
        </div>
      ) : null}
      {unavailable > 0 ? (
        <p className="text-muted-foreground mt-3 text-xs">
          {unavailable === 1
            ? "1 hidden item is no longer available to you and isn't listed."
            : `${unavailable} hidden items are no longer available to you and aren't listed.`}
        </p>
      ) : null}
    </SettingsSection>
  );
}

function HiddenRow({
  item,
  disabled,
  onRestore,
}: {
  item: RailDismissalView;
  disabled: boolean;
  onRestore: () => void;
}) {
  const href = targetHref(item);
  const name = item.target.name;
  return (
    <li className="flex items-center gap-3 py-2 first:pt-0 last:pb-0">
      <div className="w-10 shrink-0">
        {item.target_kind === "cbl" ? (
          <div className="bg-muted border-border text-muted-foreground flex aspect-[2/3] items-center justify-center rounded-md border">
            <ListOrdered className="size-4" aria-hidden />
          </div>
        ) : (
          <Cover src={item.target.cover_url} alt="" fallback={name} />
        )}
      </div>
      <div className="min-w-0 flex-1">
        {href ? (
          <Link
            href={href}
            className="folio-tap text-foreground block truncate text-sm font-medium hover:underline"
          >
            {name}
          </Link>
        ) : (
          <span className="text-foreground block truncate text-sm font-medium">
            {name}
          </span>
        )}
        {item.target.subtitle ? (
          <p className="text-muted-foreground truncate text-xs">
            {item.target.subtitle}
          </p>
        ) : null}
        <p className="text-muted-foreground truncate text-xs">
          Hidden from {hiddenFromLabel(item.hidden_from)} ·{" "}
          <time dateTime={item.dismissed_at}>{timeAgo(item.dismissed_at)}</time>
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        disabled={disabled}
        onClick={onRestore}
        aria-label={`Restore ${name}`}
      >
        Restore
      </Button>
    </li>
  );
}
