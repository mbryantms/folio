"use client";

import { ArrowRight, BookOpen, ChevronRight, Clock, X } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { useMe } from "@/lib/api/queries";
import { useGlobalSearch } from "@/lib/search/use-search";
import { useRecentSearches } from "@/lib/search/use-recent-searches";
import { renderSearchSnippet } from "@/lib/search/render-snippet";
import {
  SEARCH_ACTIONS_CAP,
  parseCommandPrefix,
  rankSearchActions,
  type SearchAction,
} from "@/lib/search/actions-registry";
import {
  SEARCH_CATEGORIES,
  flattenGroups,
  type SearchHit,
} from "@/lib/search/types";
import { cn } from "@/lib/utils";

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
  const me = useMe();
  const [raw, setRaw] = useState("");
  const [debounced, setDebounced] = useState("");
  const [highlighted, setHighlighted] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const recents = useRecentSearches();

  // `>` prefix on the input flips the modal into pure command mode —
  // content categories disappear and only the action registry is
  // searched. The "needle" is the input with the prefix stripped, so
  // typing `> lib` matches actions whose label/keywords start with
  // "lib" (e.g. "Manage libraries").
  const { needle: rawForActions, commandMode } = parseCommandPrefix(raw);

  // Debounce the query; 200ms keeps the UI snappy without firing on each
  // keystroke. Command mode skips the debounce — action filtering is
  // entirely client-side and feels nicer when it responds keystroke-
  // by-keystroke.
  useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);
  // Strip the prefix from the content query too so a partial `>` typed
  // mid-search doesn't accidentally feed `>library` to the backend.
  const contentQuery = commandMode ? "" : debounced;

  const { enabled, isLoading, groups, total } = useGlobalSearch(contentQuery, {
    perCategory: MAX_PER_CATEGORY,
  });

  // Resolve actions on every keystroke. Role-gating uses `useMe()` —
  // non-admin users never see admin entries even via direct keyword
  // match. Capped at SEARCH_ACTIONS_CAP so the actions section
  // doesn't dominate the modal alongside content hits.
  const actions = useMemo<readonly SearchAction[]>(() => {
    const ranked = rankSearchActions(rawForActions, me.data?.role);
    return ranked.slice(0, SEARCH_ACTIONS_CAP);
  }, [rawForActions, me.data?.role]);

  // Flat list of visible hits in display order. Actions sit on top so
  // the keyboard cursor lands there first — a `>`-mode user gets the
  // top action highlighted; a free-text user sees actions but can
  // arrow past them into content hits without losing context.
  const contentFlat = flattenGroups(groups, MAX_PER_CATEGORY);
  const flat: ReadonlyArray<SearchHit | SearchAction> = commandMode
    ? actions
    : [...actions, ...contentFlat];
  const safeHighlighted =
    flat.length === 0 ? 0 : Math.min(highlighted, flat.length - 1);
  const fullSearchHref = enabled
    ? `/search?q=${encodeURIComponent(contentQuery)}`
    : "/search";

  function handleOpenChange(next: boolean) {
    if (!next) {
      setRaw("");
      setDebounced("");
      setHighlighted(0);
    }
    onOpenChange(next);
  }

  function isAction(row: SearchHit | SearchAction): row is SearchAction {
    // Action rows carry `group` + `icon` directly; content `SearchHit`s
    // have `kind` + `title`. The discriminator is `group` because every
    // action in the registry sets it and no `SearchHit` does.
    return "group" in row;
  }

  function commitRow(row: SearchHit | SearchAction | undefined) {
    if (!row) return;
    handleOpenChange(false);
    if (isAction(row)) {
      router.push(row.href);
      return;
    }
    // Content hits get recent-search bookkeeping; actions don't (a
    // chord like `>auth` isn't a query worth replaying).
    if (contentQuery) recents.add(contentQuery);
    router.push(row.href);
  }

  function commitFullSearch() {
    if (contentQuery) recents.add(contentQuery);
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
      // the modal without hunting for the footer. Command mode skips
      // the full-search escape since there's no content query to send.
      if (!commandMode && (e.metaKey || e.ctrlKey)) {
        commitFullSearch();
      } else if (flat.length > 0) {
        commitRow(flat[safeHighlighted]);
      } else if (!commandMode && enabled) {
        // No quick hits but the user pressed Enter — interpret it as
        // "take me to full search."
        commitFullSearch();
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="top-[10%] flex max-h-[80vh] max-w-xl translate-y-0 flex-col gap-0 overflow-hidden p-0"
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
        <div className="border-border shrink-0 border-b">
          <input
            ref={inputRef}
            type="search"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={
              commandMode
                ? "Jump to settings, admin pages, sections…"
                : "Search series, issues, people…  Type > for commands"
            }
            aria-label="Search the library"
            className="placeholder:text-muted-foreground w-full bg-transparent px-4 py-3 text-sm focus:outline-none"
          />
        </div>
        <ResultsBody
          enabled={enabled}
          loading={isLoading && total === 0}
          flat={flat}
          actions={actions}
          actionsOffset={0}
          contentOffset={commandMode ? 0 : actions.length}
          commandMode={commandMode}
          highlighted={safeHighlighted}
          onHover={setHighlighted}
          onSelect={(idx) => commitRow(flat[idx])}
          query={contentQuery}
          groups={groups}
          recents={recents.recents}
          onPickRecent={(q) => {
            setRaw(q);
            setDebounced(q);
            inputRef.current?.focus();
          }}
          onRemoveRecent={recents.remove}
          onClearRecents={recents.clear}
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
  actions,
  actionsOffset,
  contentOffset,
  commandMode,
  highlighted,
  onHover,
  onSelect,
  query,
  groups,
  recents,
  onPickRecent,
  onRemoveRecent,
  onClearRecents,
}: {
  enabled: boolean;
  loading: boolean;
  flat: ReadonlyArray<SearchHit | SearchAction>;
  actions: readonly SearchAction[];
  actionsOffset: number;
  contentOffset: number;
  commandMode: boolean;
  highlighted: number;
  onHover: (idx: number) => void;
  onSelect: (idx: number) => void;
  query: string;
  groups: ReturnType<typeof useGlobalSearch>["groups"];
  recents: readonly string[];
  onPickRecent: (q: string) => void;
  onRemoveRecent: (q: string) => void;
  onClearRecents: () => void;
}) {
  // Empty-state path: content search not enabled (input under 2 chars
  // AND not in command mode) and no actions either. Command mode with
  // an empty input still surfaces the full action registry so a power
  // user can just type `>` and arrow-pick.
  if (!enabled && !commandMode && actions.length === 0) {
    return (
      <EmptyStateBody
        recents={recents}
        onPickRecent={onPickRecent}
        onRemoveRecent={onRemoveRecent}
        onClearRecents={onClearRecents}
      />
    );
  }
  if (loading && actions.length === 0) {
    return (
      <p className="text-muted-foreground px-4 py-6 text-center text-xs">
        Searching…
      </p>
    );
  }
  if (flat.length === 0) {
    return (
      <p className="text-muted-foreground px-4 py-6 text-center text-xs">
        {commandMode ? "No commands match." : `No matches for “${query}”.`}
      </p>
    );
  }
  // Pre-compute the (category → starting flat index) map so the JSX
  // doesn't mutate a counter during render. `sections` walks the
  // canonical category order, dropping empty groups and recording where
  // each visible category begins inside the flattened list.
  const sections = commandMode
    ? []
    : SEARCH_CATEGORIES.reduce<
        Array<{
          def: (typeof SEARCH_CATEGORIES)[number];
          hits: SearchHit[];
          fromIdx: number;
        }>
      >((acc, def) => {
        const hits = groups[def.key].slice(0, MAX_PER_CATEGORY);
        if (hits.length === 0) return acc;
        const fromIdx =
          contentOffset + acc.reduce((n, s) => n + s.hits.length, 0);
        acc.push({ def, hits, fromIdx });
        return acc;
      }, []);

  return (
    <div
      role="listbox"
      aria-label="Search results"
      className="min-h-0 flex-1 overflow-y-auto py-1"
    >
      {actions.length > 0 ? (
        <section key="actions" className="py-1">
          <h3 className="text-muted-foreground px-3 pb-1 text-[11px] font-semibold tracking-wide uppercase">
            {commandMode ? "Commands" : "Jump to…"}
          </h3>
          <ul>
            {actions.map((action, i) => {
              const idx = actionsOffset + i;
              const Icon = action.icon;
              return (
                <li key={action.id}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={idx === highlighted}
                    onMouseEnter={() => onHover(idx)}
                    onClick={() => onSelect(idx)}
                    className={cn(
                      "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
                      idx === highlighted
                        ? "bg-accent text-accent-foreground"
                        : "text-foreground",
                    )}
                  >
                    <span
                      aria-hidden="true"
                      className="border-border bg-muted text-muted-foreground inline-flex h-9 w-9 shrink-0 items-center justify-center rounded border"
                    >
                      <Icon className="size-4" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">
                        {action.label}
                      </span>
                      <span className="block truncate text-xs opacity-70">
                        {action.group}
                      </span>
                    </span>
                    <ChevronRight
                      className="size-3.5 opacity-50"
                      aria-hidden="true"
                    />
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}
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
                      className={cn(
                        "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
                        idx === highlighted
                          ? "bg-accent text-accent-foreground"
                          : "text-foreground",
                      )}
                    >
                      <Thumb hit={hit} />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {hit.title}
                        </span>
                        {hit.snippet ? (
                          // Snippet wins over the static subtitle when
                          // the backend produced a `ts_headline` excerpt
                          // — the `<mark>` highlights inside tell the
                          // user *why* this row matched, which is
                          // higher signal than the publisher / year
                          // metadata in the static subtitle.
                          // Sanitised via `renderSearchSnippet` so the
                          // only HTML we forward is the `<mark>`
                          // allowlist; everything else is escaped.
                          <span
                            className="block truncate text-xs opacity-70 [&_mark]:rounded-sm [&_mark]:bg-amber-500/30 [&_mark]:px-0.5 [&_mark]:text-current"
                            dangerouslySetInnerHTML={{
                              __html: renderSearchSnippet(hit.snippet),
                            }}
                          />
                        ) : hit.subtitle ? (
                          // Static subtitle fallback. Inherits the
                          // button's text color (`text-foreground` or
                          // `text-accent-foreground` when the row is
                          // highlighted) at reduced opacity — using
                          // `text-muted-foreground` here would make
                          // the gray subtitle unreadable against the
                          // amber accent background.
                          <span className="block truncate text-xs opacity-70">
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

/** Empty-state body shown when the input is empty (or < 2 chars). When
 *  the user has recent searches, surfaces them as clickable chips with
 *  inline remove + a clear-all action. Otherwise falls through to a
 *  minimal "type to search" prompt + a short tips line. */
function EmptyStateBody({
  recents,
  onPickRecent,
  onRemoveRecent,
  onClearRecents,
}: {
  recents: readonly string[];
  onPickRecent: (q: string) => void;
  onRemoveRecent: (q: string) => void;
  onClearRecents: () => void;
}) {
  if (recents.length === 0) {
    return (
      <div className="text-muted-foreground space-y-2 px-4 py-6 text-center text-xs">
        <p>Type at least 2 characters to search.</p>
        <p>
          <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
            ↵
          </kbd>
          opens the highlighted hit ·{" "}
          <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
            ⌘
          </kbd>
          <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
            ↵
          </kbd>
          shows full results
        </p>
      </div>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto py-2">
      <div className="flex items-center justify-between px-3 pb-1">
        <h3 className="text-muted-foreground inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-wide uppercase">
          <Clock className="size-3" aria-hidden="true" />
          Recent searches
        </h3>
        <button
          type="button"
          onClick={onClearRecents}
          className="text-muted-foreground hover:text-foreground text-[11px] font-medium"
        >
          Clear all
        </button>
      </div>
      <ul className="flex flex-wrap gap-1.5 px-3 pb-3">
        {recents.map((q) => (
          <li key={q}>
            <span className="bg-muted/60 hover:bg-muted text-foreground border-border inline-flex items-center gap-1 rounded-full border py-0.5 pr-1 pl-3 text-xs transition-colors">
              <button
                type="button"
                onClick={() => onPickRecent(q)}
                className="focus-visible:outline-none"
                aria-label={`Search again for ${q}`}
              >
                {q}
              </button>
              <button
                type="button"
                onClick={() => onRemoveRecent(q)}
                aria-label={`Remove ${q} from recents`}
                className="text-muted-foreground hover:text-foreground inline-flex size-4 items-center justify-center rounded-full"
              >
                <X className="size-2.5" aria-hidden="true" />
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Thumb({ hit }: { hit: SearchHit }) {
  const cls =
    "border-border bg-muted h-12 w-9 shrink-0 overflow-hidden rounded border";
  if (hit.thumbUrl) {
    if (hit.region) {
      const scaleW = Math.min(100, 100 / Math.max(hit.region.w, 1));
      const scaleH = Math.min(100, 100 / Math.max(hit.region.h, 1));
      return (
        <div className={cn(cls, "relative")}>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={hit.thumbUrl}
            alt={hit.title}
            loading="lazy"
            decoding="async"
            className="max-w-none"
            style={{
              position: "absolute",
              width: `${scaleW * 100}%`,
              height: `${scaleH * 100}%`,
              left: `${-hit.region.x * scaleW}%`,
              top: `${-hit.region.y * scaleH}%`,
            }}
          />
        </div>
      );
    }
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={hit.thumbUrl}
        alt={hit.title}
        loading="lazy"
        decoding="async"
        className={cn(cls, "object-cover")}
      />
    );
  }
  const Icon = hit.icon ?? BookOpen;
  return (
    <div
      aria-hidden="true"
      className={cn(cls, "text-muted-foreground grid place-items-center")}
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
  // The shortcut hints hide on narrow viewports so the top-results link
  // always remains visible without competing for space.
  return (
    <div className="border-border text-muted-foreground flex shrink-0 flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t px-3 py-2 text-xs">
      <div className="hidden items-center gap-3 sm:flex">
        {enabled && total > 0 ? (
          <>
            <ShortcutHint keys={["↵"]} label="open" />
            <ShortcutHint keys={["⌘", "↵"]} label="top results" />
          </>
        ) : enabled ? (
          <span>No quick hits — try the search page.</span>
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
          ? `View top results for "${query}"`
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
