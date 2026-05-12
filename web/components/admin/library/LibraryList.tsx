"use client";

import Link from "next/link";
import { Library as LibraryIcon } from "lucide-react";

import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/admin/EmptyState";
import { useLibraryList } from "@/lib/api/queries";

export function LibraryList() {
  const { data, isLoading, error } = useLibraryList();

  if (isLoading) {
    return (
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-28 w-full" />
        ))}
      </div>
    );
  }
  if (error) {
    return (
      <p className="text-destructive text-sm">
        Failed to load libraries: {error.message}
      </p>
    );
  }
  if (!data || data.length === 0) {
    return (
      <EmptyState
        icon={LibraryIcon}
        title="No libraries yet"
        description="Create one to point Folio at a folder on disk."
      />
    );
  }

  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {data.map((lib) => (
        <Link
          key={lib.id}
          href={`/admin/libraries/${lib.slug}`}
          className="focus-visible:ring-ring block rounded-lg focus:outline-none focus-visible:ring-2"
        >
          <Card className="hover:border-primary/40 h-full transition-colors">
            <CardContent className="flex h-full flex-col gap-2 p-5">
              <p className="text-foreground font-medium">{lib.name}</p>
              <p
                className="text-muted-foreground truncate font-mono text-xs"
                title={lib.root_path}
              >
                {lib.root_path}
              </p>
              <div className="text-muted-foreground mt-auto flex items-center justify-between pt-2 text-xs">
                <span>
                  {lib.last_scan_at
                    ? `Last scan ${new Date(lib.last_scan_at).toLocaleString()}`
                    : "Never scanned"}
                </span>
                <span className="border-border rounded-full border px-2 py-0.5">
                  {lib.default_reading_direction.toUpperCase()}
                </span>
              </div>
            </CardContent>
          </Card>
        </Link>
      ))}
    </div>
  );
}
