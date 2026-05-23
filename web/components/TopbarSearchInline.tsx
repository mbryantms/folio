"use client";

import {
  Bookmark,
  ChevronRight,
  Clock,
  Search,
  X,
} from "lucide-react";
import { useRouter } from "next/navigation";
import * as React from "react";

import { useMe } from "@/lib/api/queries";
import {
  SEARCH_ACTIONS_CAP,
  parseCommandPrefix,
  rankSearchActions,
  type SearchAction,
} from "@/lib/search/actions-registry";
import { renderSearchSnippet } from "@/lib/search/render-snippet";
import { useGlobalSearch } from "@/lib/search/use-search";
import { useRecentSearches } from "@/lib/search/use-recent-searches";
import {
  SEARCH_CATEGORIES,
  flattenGroups,
  type SearchHit,
} from "@/lib/search/types";
import { cn } from "@/lib/utils";

const MAX_PER_CATEGORY = 5;
const QUERY_DEBOUNCE_MS = 200;

/** Desktop topbar search: a real `<input>` plus a popover panel
 *  anchored beneath it. Replaces the click-to-open-modal trigger
 *  that pretended to be an input. Same rendering primitives the
 *  modal uses (categorised hits, snippet highlights, recents,
 *  commands) but inline so the user can type directly without
 *  context-switching to a centred dialog.
 *
 *  Mobile shell still uses the modal (see `TopbarSearchTrigger`) —
 *  the dropdown is too cramped at phone widths and the centred
 *  fullscreen Dialog is a better fit.
 *
 *  Behaviour:
 *  - Open: focus the input (already the default; `Mod+K` + `/`
 *    forwarded by the global hotkey handler also land here).
 *  - Esc closes the panel without clearing the input — second Esc
 *    blurs.
 *  - ↑/↓ moves the highlighted row.
 *  - `↵` opens the highlighted row.
 *  - `Mod+↵` jumps to the full-results page.
 *  - Typing `>` as the first character flips to command-only mode,
 *    matching the modal's behaviour.
 *  - Clicking outside the panel closes it (input stays focused
 *    until blur).
 */
export function TopbarSearchInline({ className }: { className?: string }) {
  const router = useRouter();
  const me = useMe();
  const recents = useRecentSearches();
  const [raw, setRaw] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [open, setOpen] = React.useState(false);
  const [highlighted, setHighlighted] = React.useState(0);
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const inputRef = React.useRef<HTMLInputElement | null>(null);

  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);

  // `>` prefix → command-only mode. Strip the prefix off both the
  // content query (we don't want to send `>library` to the backend)
  // and the action-search needle (the registry doesn't store the
  // glyph either).
  const { needle: rawForActions, commandMode } = parseCommandPrefix(raw);
  const contentQuery = commandMode ? "" : debounced;

  const { enabled, isLoading, groups, total } = useGlobalSearch(contentQuery, {
    perCategory: MAX_PER_CATEGORY,
  });
  const actions = React.useMemo<readonly SearchAction[]>(() => {
    const ranked = rankSearchActions(rawForActions, me.data?.role);
    return ranked.slice(0, SEARCH_ACTIONS_CAP);
  }, [rawForActions, me.data?.role]);

  const contentFlat = flattenGroups(groups, MAX_PER_CATEGORY);
  const flat: ReadonlyArray<SearchHit | SearchAction> = commandMode
    ? actions
    : [...actions, ...contentFlat];
  const safeHighlighted =
    flat.length === 0 ? 0 : Math.min(highlighted, flat.length - 1);
  const fullSearchHref = enabled
    ? `/search?q=${encodeURIComponent(contentQuery)}`
    : "/search";

  // Outside-click / Escape close. Don't strip `raw` on close — users
  // expect the field to remember what they typed if they accidentally
  // dismiss the panel.
  React.useEffect(() => {
    if (!open) return;
    function onPointerDown(e: PointerEvent) {
      const root = rootRef.current;
      if (root && !root.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  // Reset highlight when the result set changes so the cursor doesn't
  // stick on a removed row after a keystroke.
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setHighlighted(0);
  }, [debounced, commandMode]);

  function isAction(row: SearchHit | SearchAction): row is SearchAction {
    return "group" in row;
  }

  function commitRow(row: SearchHit | SearchAction | undefined) {
    if (!row) return;
    setOpen(false);
    if (isAction(row)) {
      router.push(row.href);
      return;
    }
    if (contentQuery) recents.add(contentQuery);
    router.push(row.href);
  }

  function commitFullSearch() {
    if (contentQuery) recents.add(contentQuery);
    setOpen(false);
    router.push(fullSearchHref);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      if (!open) {
        setOpen(true);
        e.preventDefault();
        return;
      }
      e.preventDefault();
      setHighlighted(Math.min(flat.length - 1, safeHighlighted + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlighted(Math.max(0, safeHighlighted - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!commandMode && (e.metaKey || e.ctrlKey)) {
        commitFullSearch();
      } else if (flat.length > 0) {
        commitRow(flat[safeHighlighted]);
      } else if (!commandMode && enabled) {
        commitFullSearch();
      }
    } else if (e.key === "Escape") {
      if (open) {
        e.preventDefault();
        setOpen(false);
      } else {
        inputRef.current?.blur();
      }
    }
  }

  const showRecentsRow =
    !enabled && !commandMode && actions.length === 0 && recents.recents.length > 0;
  const showEmptyHint =
    !enabled && !commandMode && actions.length === 0 && recents.recents.length === 0;
  const showLoading = isLoading && !commandMode && total === 0 && actions.length === 0;
  const showNoMatch = enabled && flat.length === 0;

  return (
    <div
      ref={rootRef}
      className={cn("relative", className)}
      role="combobox"
      aria-expanded={open}
      aria-haspopup="listbox"
      aria-controls="topbar-search-panel"
    >
      <div className="relative">
        <Search
          aria-hidden="true"
          className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
        />
        <input
          ref={inputRef}
          type="search"
          value={raw}
          onChange={(e) => {
            setRaw(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onKeyDown={onKeyDown}
          placeholder={
            commandMode
              ? "Jump to settings, admin pages…"
              : "Search series, issues, people…"
          }
          aria-label="Search the library"
          aria-autocomplete="list"
          aria-controls="topbar-search-panel"
          className="border-border bg-muted/40 focus-visible:ring-ring h-9 w-full rounded-md border pr-8 pl-8 text-sm transition-colors focus:bg-transparent focus-visible:ring-2 focus-visible:outline-none"
        />
        {raw ? (
          <button
            type="button"
            onClick={() => {
              setRaw("");
              setDebounced("");
              inputRef.current?.focus();
            }}
            className="text-muted-foreground hover:text-foreground absolute top-1/2 right-1.5 inline-flex size-6 -translate-y-1/2 items-center justify-center rounded"
            aria-label="Clear search"
          >
            <X aria-hidden="true" className="size-3.5" />
          </button>
        ) : null}
      </div>

      {open ? (
        <div
          id="topbar-search-panel"
          role="listbox"
          aria-label="Search results"
          className="border-border bg-popover text-popover-foreground absolute top-[calc(100%+0.25rem)] right-0 left-0 z-40 max-h-[70vh] overflow-y-auto rounded-md border shadow-xl"
        >
          {showEmptyHint ? (
            <div className="text-muted-foreground space-y-2 px-4 py-6 text-center text-xs">
              <p>Type at least 2 characters to search.</p>
              <p>
                <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
                  ↵
                </kbd>
                opens · type{" "}
                <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
                  &gt;
                </kbd>
                for commands
              </p>
            </div>
          ) : null}

          {showRecentsRow ? (
            <div className="py-2">
              <div className="flex items-center justify-between px-3 pb-1">
                <h3 className="text-muted-foreground inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-wide uppercase">
                  <Clock className="size-3" aria-hidden="true" />
                  Recent searches
                </h3>
                <button
                  type="button"
                  onClick={() => recents.clear()}
                  className="text-muted-foreground hover:text-foreground text-[11px] font-medium"
                >
                  Clear all
                </button>
              </div>
              <ul className="flex flex-wrap gap-1.5 px-3 pb-2">
                {recents.recents.map((q) => (
                  <li key={q}>
                    <span className="bg-muted/60 hover:bg-muted text-foreground border-border inline-flex items-center gap-1 rounded-full border py-0.5 pr-1 pl-3 text-xs transition-colors">
                      <button
                        type="button"
                        onClick={() => {
                          setRaw(q);
                          setDebounced(q);
                          inputRef.current?.focus();
                        }}
                        className="focus-visible:outline-none"
                        aria-label={`Search again for ${q}`}
                      >
                        {q}
                      </button>
                      <button
                        type="button"
                        onClick={() => recents.remove(q)}
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
          ) : null}

          {showLoading ? (
            <p className="text-muted-foreground px-4 py-6 text-center text-xs">
              Searching…
            </p>
          ) : null}

          {showNoMatch ? (
            <p className="text-muted-foreground px-4 py-6 text-center text-xs">
              {commandMode
                ? "No commands match."
                : `No matches for “${contentQuery}”.`}
            </p>
          ) : null}

          {actions.length > 0 ? (
            <ResultSection title={commandMode ? "Commands" : "Jump to…"}>
              {actions.map((action, i) => {
                const idx = i;
                const Icon = action.icon;
                return (
                  <ResultRow
                    key={action.id}
                    selected={idx === safeHighlighted}
                    onHover={() => setHighlighted(idx)}
                    onSelect={() => commitRow(action)}
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
                  </ResultRow>
                );
              })}
            </ResultSection>
          ) : null}

          {!commandMode
            ? SEARCH_CATEGORIES.map((def) => {
                const hits = groups[def.key].slice(0, MAX_PER_CATEGORY);
                if (hits.length === 0) return null;
                const sectionStart =
                  actions.length +
                  SEARCH_CATEGORIES.slice(0, SEARCH_CATEGORIES.indexOf(def))
                    .map((d) => groups[d.key].slice(0, MAX_PER_CATEGORY).length)
                    .reduce((a, b) => a + b, 0);
                return (
                  <ResultSection key={def.key} title={def.labelPlural}>
                    {hits.map((hit, i) => {
                      const idx = sectionStart + i;
                      return (
                        <ResultRow
                          key={hit.id}
                          selected={idx === safeHighlighted}
                          onHover={() => setHighlighted(idx)}
                          onSelect={() => commitRow(hit)}
                        >
                          <Thumb hit={hit} />
                          <span className="min-w-0 flex-1">
                            <span className="block truncate font-medium">
                              {hit.title}
                            </span>
                            {hit.snippet ? (
                              <span
                                className="block truncate text-xs opacity-70 [&_mark]:bg-amber-500/30 [&_mark]:text-current [&_mark]:rounded-sm [&_mark]:px-0.5"
                                dangerouslySetInnerHTML={{
                                  __html: renderSearchSnippet(hit.snippet),
                                }}
                              />
                            ) : hit.subtitle ? (
                              <span className="block truncate text-xs opacity-70">
                                {hit.subtitle}
                              </span>
                            ) : null}
                          </span>
                        </ResultRow>
                      );
                    })}
                  </ResultSection>
                );
              })
            : null}

          {enabled && !commandMode ? (
            <div className="border-border text-muted-foreground flex items-center justify-between border-t px-3 py-2 text-xs">
              <span className="hidden sm:inline">
                <kbd className="bg-muted text-foreground border-border mr-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
                  ⌘
                </kbd>
                <kbd className="bg-muted text-foreground border-border inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
                  ↵
                </kbd>{" "}
                for full results
              </span>
              <button
                type="button"
                onClick={commitFullSearch}
                className="hover:text-foreground ml-auto inline-flex items-center gap-1 font-medium"
              >
                View all results for &ldquo;{contentQuery}&rdquo;
                <ChevronRight className="size-3" aria-hidden="true" />
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function ResultSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="py-1">
      <h3 className="text-muted-foreground px-3 pb-1 text-[11px] font-semibold tracking-wide uppercase">
        {title}
      </h3>
      <ul>{children}</ul>
    </section>
  );
}

function ResultRow({
  selected,
  onHover,
  onSelect,
  children,
}: {
  selected: boolean;
  onHover: () => void;
  onSelect: () => void;
  children: React.ReactNode;
}) {
  return (
    <li>
      <button
        type="button"
        role="option"
        aria-selected={selected}
        onMouseEnter={onHover}
        onClick={onSelect}
        className={cn(
          "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
          selected ? "bg-accent text-accent-foreground" : "text-foreground",
        )}
      >
        {children}
      </button>
    </li>
  );
}

/** Cover thumbnail for content hits. Falls back to the bookmark glyph
 *  on coverless or icon-only hits. */
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
        className={cn(cls, "object-cover")}
      />
    );
  }
  const Icon = hit.icon ?? Bookmark;
  return (
    <div
      aria-hidden="true"
      className={cn(cls, "text-muted-foreground grid place-items-center")}
    >
      <Icon className="size-4" />
    </div>
  );
}
