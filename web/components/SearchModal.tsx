"use client";

import { ArrowRight, BookOpen } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { useGlobalSearch } from "@/lib/search/use-search";
import {
  SEARCH_CATEGORIES,
  flattenGroups,
  type SearchHit,
} from "@/lib/search/types";

const MAX_PER_CATEGORY = 5;
const QUERY_DEBOUNCE_MS = 200;

/**
 * Global search dialog — opened from anywhere via the `openSearch` keybind
 * or the layout's search button; closes on Escape or backdrop click.
 *
 * Hits are grouped by category (`SEARCH_CATEGORIES`). The footer links to
 * the dedicated `/search` page for the full results across every
 * category — useful for browsing past the per-category cap or filtering
 * by category. `Mod+Enter` from the input jumps straight to that page.
 */
export function SearchModal({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  const router = useRouter();
  const [raw, setRaw] = useState("");
  const [debounced, setDebounced] = useState("");
  const [highlighted, setHighlighted] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  // Debounce the query; 200ms keeps the UI snappy without firing on each
  // keystroke.
  useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);

  const { enabled, isLoading, groups, total } = useGlobalSearch(debounced, {
    perCategory: MAX_PER_CATEGORY,
  });

  // Flat list of visible hits in display order — needed so ↑/↓ can move
  // across category boundaries without the keyboard handler having to
  // know about the section structure.
  const flat = flattenGroups(groups, MAX_PER_CATEGORY);
  const safeHighlighted =
    flat.length === 0 ? 0 : Math.min(highlighted, flat.length - 1);
  const fullSearchHref = enabled
    ? `/search?q=${encodeURIComponent(debounced)}`
    : "/search";

  function handleOpenChange(next: boolean) {
    if (!next) {
      setRaw("");
      setDebounced("");
      setHighlighted(0);
    }
    onOpenChange(next);
  }

  function commitHit(hit: SearchHit | undefined) {
    if (!hit) return;
    handleOpenChange(false);
    router.push(hit.href);
  }

  function commitFullSearch() {
    handleOpenChange(false);
    router.push(fullSearchHref);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlighted(Math.min(flat.length - 1, safeHighlighted + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlighted(Math.max(0, safeHighlighted - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      // Mod+Enter jumps to the full search page so power users can leave
      // the modal without hunting for the footer.
      if (e.metaKey || e.ctrlKey) {
        commitFullSearch();
      } else if (flat.length > 0) {
        commitHit(flat[safeHighlighted]);
      } else if (enabled) {
        // No quick hits but the user pressed Enter — interpret it as
        // "take me to full search."
        commitFullSearch();
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="top-[20%] max-w-xl translate-y-0 gap-0 overflow-hidden p-0"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          inputRef.current?.focus();
        }}
      >
        <DialogTitle className="sr-only">Search</DialogTitle>
        <DialogDescription className="sr-only">
          Quick search across your library. Press Enter to open the highlighted
          result, or Mod+Enter to jump to the full results page.
        </DialogDescription>
        <div className="border-border border-b">
          <input
            ref={inputRef}
            type="search"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Search series, issues, people…"
            aria-label="Search the library"
            className="placeholder:text-muted-foreground w-full bg-transparent px-4 py-3 text-sm focus:outline-none"
          />
        </div>
        <ResultsBody
          enabled={enabled}
          loading={isLoading && total === 0}
          flat={flat}
          highlighted={safeHighlighted}
          onHover={setHighlighted}
          onSelect={(idx) => commitHit(flat[idx])}
          query={debounced}
          groups={groups}
        />
        <SearchFooter
          enabled={enabled}
          query={debounced}
          total={total}
          href={fullSearchHref}
          onNavigate={commitFullSearch}
        />
      </DialogContent>
    </Dialog>
  );
}

function ResultsBody({
  enabled,
  loading,
  flat,
  highlighted,
  onHover,
  onSelect,
  query,
  groups,
}: {
  enabled: boolean;
  loading: boolean;
  flat: ReadonlyArray<SearchHit>;
  highlighted: number;
  onHover: (idx: number) => void;
  onSelect: (idx: number) => void;
  query: string;
  groups: ReturnType<typeof useGlobalSearch>["groups"];
}) {
  if (!enabled) {
    return (
      <p className="text-muted-foreground px-4 py-6 text-center text-xs">
        Type at least 2 characters to search.
      </p>
    );
  }
  if (loading) {
    return (
      <p className="text-muted-foreground px-4 py-6 text-center text-xs">
        Searching…
      </p>
    );
  }
  if (flat.length === 0) {
    return (
      <p className="text-muted-foreground px-4 py-6 text-center text-xs">
        No matches for &ldquo;{query}&rdquo;.
      </p>
    );
  }
  // Pre-compute the (category → starting flat index) map so the JSX
  // doesn't mutate a counter during render. `sections` walks the
  // canonical category order, dropping empty groups and recording where
  // each visible category begins inside the flattened list.
  const sections = SEARCH_CATEGORIES.reduce<
    Array<{
      def: (typeof SEARCH_CATEGORIES)[number];
      hits: SearchHit[];
      fromIdx: number;
    }>
  >((acc, def) => {
    const hits = groups[def.key].slice(0, MAX_PER_CATEGORY);
    if (hits.length === 0) return acc;
    const fromIdx = acc.reduce((n, s) => n + s.hits.length, 0);
    acc.push({ def, hits, fromIdx });
    return acc;
  }, []);

  return (
    <div
      role="listbox"
      aria-label="Search results"
      className="max-h-96 overflow-y-auto py-1"
    >
      {sections.map(({ def, hits, fromIdx }) => {
        return (
          <section key={def.key} className="py-1">
            <h3 className="text-muted-foreground px-3 pb-1 text-[11px] font-semibold tracking-wide uppercase">
              {def.labelPlural}
            </h3>
            <ul>
              {hits.map((hit, i) => {
                const idx = fromIdx + i;
                return (
                  <li key={hit.id}>
                    <button
                      type="button"
                      role="option"
                      aria-selected={idx === highlighted}
                      onMouseEnter={() => onHover(idx)}
                      onClick={() => onSelect(idx)}
                      className={`flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors ${
                        idx === highlighted
                          ? "bg-accent text-accent-foreground"
                          : "text-foreground"
                      }`}
                    >
                      <Thumb hit={hit} />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {hit.title}
                        </span>
                        {hit.subtitle ? (
                          <span className="text-muted-foreground block truncate text-xs">
                            {hit.subtitle}
                          </span>
                        ) : null}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

function Thumb({ hit }: { hit: SearchHit }) {
  const cls =
    "border-border bg-muted h-12 w-9 shrink-0 overflow-hidden rounded border";
  if (hit.thumbUrl) {
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={hit.thumbUrl}
        alt={hit.title}
        loading="lazy"
        decoding="async"
        className={`${cls} object-cover`}
      />
    );
  }
  const Icon = hit.icon ?? BookOpen;
  return (
    <div
      aria-hidden="true"
      className={`${cls} text-muted-foreground grid place-items-center`}
    >
      <Icon className="size-4" />
    </div>
  );
}

function SearchFooter({
  enabled,
  query,
  total,
  href,
  onNavigate,
}: {
  enabled: boolean;
  query: string;
  total: number;
  href: string;
  onNavigate: () => void;
}) {
  // Drop `truncate` here on purpose: it was clipping the kbd glyph chips
  // (especially `⌘ ↵`) whenever the modal was narrower than its content.
  // The shortcut hints hide on narrow viewports so the "View all" link
  // always remains visible without competing for space.
  return (
    <div className="border-border text-muted-foreground flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t px-3 py-2 text-xs">
      <div className="hidden items-center gap-3 sm:flex">
        {enabled && total > 0 ? (
          <>
            <ShortcutHint keys={["↵"]} label="open" />
            <ShortcutHint keys={["⌘", "↵"]} label="full results" />
          </>
        ) : enabled ? (
          <span>No quick hits — try the full search page.</span>
        ) : (
          <span>Quick search across your library.</span>
        )}
      </div>
      <Link
        href={href}
        onClick={(e) => {
          // Imperative push so the modal closes synchronously and we
          // don't lose state between unmount and the next page paint.
          e.preventDefault();
          onNavigate();
        }}
        className="hover:text-foreground ml-auto inline-flex shrink-0 items-center gap-1 font-medium"
      >
        {enabled && query.length > 0
          ? `View all results for "${query}"`
          : "Open search page"}
        <ArrowRight className="size-3" aria-hidden="true" />
      </Link>
    </div>
  );
}

/** Single key-hint chip — `<kbd>` glyphs in inline-flex so multi-key
 *  combos like `⌘ ↵` line up at a stable height regardless of which
 *  glyphs the user's font supplies. */
function ShortcutHint({
  keys,
  label,
}: {
  keys: readonly string[];
  label: string;
}) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <span className="inline-flex items-center gap-1">
        {keys.map((k, i) => (
          <kbd
            key={i}
            className="bg-muted text-foreground border-border inline-flex h-5 min-w-5 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none"
          >
            {k}
          </kbd>
        ))}
      </span>
      <span>{label}</span>
    </span>
  );
}
