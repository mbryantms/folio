"use client";

import * as React from "react";

import { DuplicatesPanel } from "@/components/admin/library/DuplicatesPanel";
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { useLibraryList } from "@/lib/api/queries";

/**
 * Admin-nav entry point for the Duplicates page (WP-3.3). Duplicates are
 * library-scoped, so this picks a library (first one by default) and
 * renders the same panel as the per-library "Duplicates" tab.
 */
export function DuplicatesOverview() {
  const libraries = useLibraryList();
  const [picked, setPicked] = React.useState<string | null>(null);

  if (libraries.isLoading) return <Skeleton className="h-64 w-full" />;
  if (libraries.error) {
    return (
      <p className="text-destructive text-sm">{libraries.error.message}</p>
    );
  }
  const list = libraries.data ?? [];
  if (list.length === 0) {
    return (
      <p className="border-border bg-card/40 text-muted-foreground rounded-md border border-dashed px-4 py-12 text-center text-sm">
        No libraries yet.
      </p>
    );
  }
  const slug = picked ?? list[0]!.slug;

  return (
    <div className="space-y-4">
      {list.length > 1 ? (
        <label className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">Library</span>
          <NativeSelect
            size="sm"
            aria-label="Library"
            value={slug}
            onChange={setPicked}
            options={list.map((l) => ({ value: l.slug, label: l.name }))}
          />
        </label>
      ) : null}
      <DuplicatesPanel key={slug} libraryId={slug} />
    </div>
  );
}
