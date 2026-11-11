"use client";

import { LibraryBig, Search } from "lucide-react";
import Link from "next/link";

import { Button } from "@/components/ui/button";

/** The `/search` page's state when every category came back empty (U-11):
 *  the closest series names the server offered, each a button that runs
 *  that search, and a way into Browse. Suggestions are library data —
 *  rendered as text, never as markup. */
export function SearchNoResults({
  query,
  suggestions,
  onPick,
}: {
  query: string;
  suggestions: readonly string[];
  onPick: (q: string) => void;
}) {
  return (
    <div className="text-muted-foreground space-y-3 text-sm">
      <p>No matches for &ldquo;{query}&rdquo; in any category.</p>
      {suggestions.length > 0 ? (
        <div className="space-y-2">
          <p id="search-suggestions-label">Did you mean:</p>
          <ul
            aria-labelledby="search-suggestions-label"
            className="flex flex-wrap gap-2"
          >
            {suggestions.map((name) => (
              <li key={name} className="min-w-0">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => onPick(name)}
                  aria-label={`Search for ${name}`}
                  className="max-w-full"
                >
                  <Search className="size-3.5" aria-hidden="true" />
                  <span className="truncate">{name}</span>
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <Button asChild variant="ghost" size="sm" className="-ml-3">
        <Link href="/browse">
          <LibraryBig className="size-3.5" aria-hidden="true" />
          Browse the library
        </Link>
      </Button>
    </div>
  );
}
