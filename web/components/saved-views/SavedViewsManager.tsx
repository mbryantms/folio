"use client";

import * as React from "react";
import Link from "next/link";
import { Lock, Pin } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useSidebarSavedView } from "@/lib/api/mutations";
import { useSavedViews } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

import { MultiPinDialog } from "./MultiPinDialog";

/** Per-user saved-views **arrangement** surface. Lives at `/settings/views`.
 *  Defaults & arrangement only (A3): pin each view to one or more pages and
 *  toggle sidebar visibility. Creating, importing, editing, and deleting now
 *  live in the library at `/views`; pin order + sidebar order live on
 *  `/settings/pages` / `/settings/navigation`. */
export function SavedViewsManager() {
  const viewsQ = useSavedViews();
  const all = React.useMemo(
    () => viewsQ.data?.items ?? [],
    [viewsQ.data?.items],
  );
  const sorted = React.useMemo(() => {
    // Alphabetical, system and user views interleaved — same ordering the
    // picker on `/settings/navigation` uses, so a row is recognizable in
    // both places.
    return [...all].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  }, [all]);

  if (viewsQ.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading views…</div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <p className="text-muted-foreground text-sm">
        {sorted.length} view{sorted.length === 1 ? "" : "s"}. Create, import,
        and edit under{" "}
        <Link
          href="/views"
          className="text-foreground underline underline-offset-2"
        >
          Views
        </Link>
        ; manage page rails under{" "}
        <Link
          href="/settings/pages"
          className="text-foreground underline underline-offset-2"
        >
          Pages
        </Link>{" "}
        and sidebar order under{" "}
        <Link
          href="/settings/navigation"
          className="text-foreground underline underline-offset-2"
        >
          Sidebar
        </Link>
        .
      </p>

      {sorted.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm">
          No saved views yet. Create one under{" "}
          <Link
            href="/views"
            className="text-foreground underline underline-offset-2"
          >
            Views
          </Link>
          .
        </div>
      ) : (
        <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
          {sorted.map((view) => (
            <ViewRow key={view.id} view={view} />
          ))}
        </ul>
      )}
    </div>
  );
}

function ViewRow({ view }: { view: SavedViewView }) {
  const sidebar = useSidebarSavedView();
  const [pinOpen, setPinOpen] = React.useState(false);
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  // The per-user Want to Read collection (kind='collection',
  // system_key='want_to_read') is auto-seeded + backend-guarded; surface it
  // as built-in (lock chip) alongside the system rails.
  const isWantToRead =
    view.kind === "collection" && view.system_key === "want_to_read";
  const isBuiltIn = view.is_system || isWantToRead;

  return (
    <li className="bg-background flex items-center gap-3 px-3 py-2">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Link
          href={`/views/${view.id}`}
          className="hover:text-foreground truncate text-sm font-medium"
          title={view.name}
        >
          {view.name}
        </Link>
        {isBuiltIn ? (
          <span
            className="text-muted-foreground bg-muted/40 inline-flex shrink-0 items-center rounded-md border px-2 py-0.5 text-xs"
            title={
              isWantToRead
                ? "Built-in collection — its contents are curated on the detail page; the list itself can't be deleted."
                : "Built-in views can't be deleted"
            }
          >
            <Lock className="mr-1 h-3 w-3" /> Built-in
          </span>
        ) : null}
      </div>

      {/* Multi-page rails M6: pinning is per-page. The pill opens a picker
       *  listing every user page (system + custom) with a checkbox; toggling
       *  pins/unpins this view on that page. The label reflects total pin
       *  count so you can see at-a-glance where the view appears. */}
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="shrink-0"
        onClick={() => setPinOpen(true)}
        title="Pin this view to one or more pages"
      >
        <Pin className="h-3.5 w-3.5 sm:mr-1" />
        <span className="hidden sm:inline">
          {pinnedCount === 0
            ? "Pin to pages…"
            : pinnedCount === 1
              ? "Pinned to 1 page"
              : `Pinned to ${pinnedCount} pages`}
        </span>
      </Button>
      <MultiPinDialog view={view} open={pinOpen} onOpenChange={setPinOpen} />
      <ToggleControl
        label="In sidebar"
        checked={view.show_in_sidebar}
        onCheckedChange={(next) => sidebar.mutate({ id: view.id, show: next })}
      />
    </li>
  );
}

/** Compact Switch + label cluster for the per-row "In sidebar" toggle.
 *  De-emphasized so it reads as a secondary affordance; primary sidebar
 *  arrangement lives on `/settings/navigation`. */
function ToggleControl({
  label,
  checked,
  disabled,
  title,
  onCheckedChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  title?: string;
  onCheckedChange: (next: boolean) => void;
}) {
  return (
    <label
      className="text-muted-foreground hidden shrink-0 cursor-pointer items-center gap-2 text-xs sm:inline-flex"
      title={title}
    >
      <span>{label}</span>
      <Switch
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        aria-label={`${label} (${checked ? "on" : "off"})`}
      />
    </label>
  );
}
