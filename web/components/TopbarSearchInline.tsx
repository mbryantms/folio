"use client";

import { ChevronRight, Clock, Search, X } from "lucide-react";
import { ActionRowContent } from "@/components/search/ActionRowContent";
import {
  RecentVisitsHeader,
  SearchHitRowContent,
  VisitRowContent,
} from "@/components/search/SearchResultRow";
import { Kbd } from "@/components/folio/kbd";
import { useRouter } from "next/navigation";
import * as React from "react";

import {
  groupSearchActions,
  parseCommandPrefix,
  type SearchAction,
} from "@/lib/search/actions-registry";
import { useGlobalSearch } from "@/lib/search/use-search";
import { useRecentSearches } from "@/lib/search/use-recent-searches";
import { useRecentVisits } from "@/lib/search/use-recent-visits";
import type { RecentVisit } from "@/lib/search/recent-visits";
import { useSearchActions } from "@/lib/search/use-search-actions";
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
 *  - Focus leaving the search root (Tab, Shift+Tab, a programmatic
 *    focus elsewhere, the window losing focus) closes it. The rows are
 *    reached with ↑/↓ from the input (`aria-activedescendant`), so they
 *    sit out of the Tab order. The panel's other controls (Clear, the
 *    recent-search chips, the footer link) have no arrow-key path, so
 *    they stay tabbable; the panel stays open while focus is on one.
 */
export function TopbarSearchInline({ className }: { className?: string }) {
  const router = useRouter();
  const recents = useRecentSearches();
  const recentVisits = useRecentVisits();
  const [raw, setRaw] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [open, setOpen] = React.useState(false);
  const [highlighted, setHighlighted] = React.useState(0);
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const inputRef = React.useRef<HTMLInputElement | null>(null);
  // Set while a pointer is down inside the root. A click on a row that
  // the browser doesn't focus (Safari buttons, the panel's scrollbar)
  // blurs the input with a null `relatedTarget`; that must not close
  // the panel before the row's click lands.
  const pointerInsideRef = React.useRef(false);

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
  // Shared with the ⌘K dialog: suggested commands on an empty field,
  // up to six matches ahead of content hits, the full grouped list in
  // command mode.
  const {
    actions: rankedActions,
    run: runAction,
    detailFor,
    keyHintFor,
  } = useSearchActions({ needle: rawForActions, commandMode, active: open });
  const suggesting = !commandMode && rawForActions.trim().length === 0;
  const actionSections = React.useMemo(
    () =>
      commandMode
        ? groupSearchActions(rankedActions).map((g) => ({
            heading: g.group as string,
            actions: g.actions,
          }))
        : rankedActions.length > 0
          ? [
              {
                heading: suggesting ? "Suggested" : "Jump to…",
                actions: [...rankedActions],
              },
            ]
          : [],
    [commandMode, rankedActions, suggesting],
  );
  // Flat, in rendered order, so the highlight index walks the sections.
  const actions = React.useMemo(
    () => actionSections.flatMap((sec) => sec.actions),
    [actionSections],
  );

  // Recently visited pages lead the empty-field state, as navigable
  // rows ahead of the suggested commands.
  const visits: readonly RecentVisit[] =
    suggesting && !enabled ? recentVisits.visits : [];
  const contentFlat = flattenGroups(groups, MAX_PER_CATEGORY);
  const flat: ReadonlyArray<RecentVisit | SearchHit | SearchAction> =
    commandMode ? actions : [...visits, ...actions, ...contentFlat];
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

  // Focus-out close, Escape from inside the panel, and the
  // pointer-inside flag. Native listeners on the root: the root carries
  // the `combobox` role, and handlers on it would make it an interactive
  // element that has to be focusable itself.
  React.useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    function onFocusOut(e: FocusEvent) {
      const next = e.relatedTarget as Node | null;
      if (next && root!.contains(next)) return;
      if (!next && pointerInsideRef.current) return;
      setOpen(false);
    }
    // The input handles its own Escape; from a control inside the panel,
    // close and hand focus back to the field. Focus first: the input's
    // `onFocus` reopens, the close then wins.
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape" || e.target === inputRef.current) return;
      e.preventDefault();
      inputRef.current?.focus();
      setOpen(false);
    }
    function onPointerDown() {
      pointerInsideRef.current = true;
    }
    function onPointerUp() {
      pointerInsideRef.current = false;
    }
    root.addEventListener("focusout", onFocusOut);
    root.addEventListener("keydown", onKeyDown);
    root.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerUp);
    return () => {
      root.removeEventListener("focusout", onFocusOut);
      root.removeEventListener("keydown", onKeyDown);
      root.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
    };
  }, []);

  // Reset highlight when the result set changes so the cursor doesn't
  // stick on a removed row after a keystroke.
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setHighlighted(0);
  }, [debounced, commandMode]);

  function isAction(
    row: RecentVisit | SearchHit | SearchAction,
  ): row is SearchAction {
    return "group" in row;
  }

  function commitRow(
    row: RecentVisit | SearchHit | SearchAction | undefined,
    index: number,
  ) {
    if (!row) return;
    setOpen(false);
    const visit = index < visits.length ? visits[index] : undefined;
    if (visit) {
      router.push(visit.href);
      return;
    }
    if (isAction(row)) {
      runAction(row);
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
        commitRow(flat[safeHighlighted], safeHighlighted);
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

  const showRecentsRow = suggesting && !enabled && recents.recents.length > 0;
  const showEmptyHint = suggesting && !enabled && recents.recents.length === 0;
  const showLoading =
    isLoading && !commandMode && total === 0 && actions.length === 0;
  const showNoMatch = enabled && flat.length === 0;

  return (
    <div
      ref={rootRef}
      className={cn("relative", className)}
      role="combobox"
      aria-expanded={open}
      aria-haspopup="listbox"
      aria-controls={open ? "topbar-search-panel" : undefined}
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
          aria-controls={open ? "topbar-search-panel" : undefined}
          aria-activedescendant={
            open && flat.length > 0 ? optionId(safeHighlighted) : undefined
          }
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
            <p className="text-muted-foreground border-border border-b px-4 py-2.5 text-center text-xs">
              Type 2+ characters to search · <Kbd className="mx-1">↵</Kbd>
              opens · <Kbd className="mx-1">&gt;</Kbd>
              for commands
            </p>
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

          {visits.length > 0 ? (
            <section className="pb-1" aria-label="Recently visited">
              <RecentVisitsHeader onClear={recentVisits.clear} />
              <ul>
                {visits.map((visit, idx) => (
                  <ResultRow
                    key={`${visit.kind}:${visit.id}`}
                    id={optionId(idx)}
                    selected={idx === safeHighlighted}
                    onHover={() => setHighlighted(idx)}
                    onSelect={() => commitRow(visit, idx)}
                  >
                    <VisitRowContent visit={visit} />
                  </ResultRow>
                ))}
              </ul>
            </section>
          ) : null}

          {actionSections.map((section) => {
            const sectionStart =
              visits.length + actions.indexOf(section.actions[0]!);
            return (
              <ResultSection key={section.heading} title={section.heading}>
                {section.actions.map((action, i) => {
                  const idx = sectionStart + i;
                  return (
                    <ResultRow
                      key={action.id}
                      id={optionId(idx)}
                      selected={idx === safeHighlighted}
                      onHover={() => setHighlighted(idx)}
                      onSelect={() => commitRow(action, idx)}
                    >
                      <ActionRowContent
                        action={action}
                        detail={detailFor(action)}
                        keyHint={keyHintFor(action)}
                      />
                    </ResultRow>
                  );
                })}
              </ResultSection>
            );
          })}

          {!commandMode
            ? SEARCH_CATEGORIES.map((def) => {
                const hits = groups[def.key].slice(0, MAX_PER_CATEGORY);
                if (hits.length === 0) return null;
                const sectionStart =
                  visits.length +
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
                          id={optionId(idx)}
                          selected={idx === safeHighlighted}
                          onHover={() => setHighlighted(idx)}
                          onSelect={() => commitRow(hit, idx)}
                        >
                          <SearchHitRowContent hit={hit} query={contentQuery} />
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
                <Kbd className="mr-1">⌘</Kbd>
                <Kbd>↵</Kbd> for top results
              </span>
              <button
                type="button"
                onClick={commitFullSearch}
                className="hover:text-foreground ml-auto inline-flex items-center gap-1 font-medium"
              >
                View top results for &ldquo;{contentQuery}&rdquo;
                <ChevronRight className="size-3" aria-hidden="true" />
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function optionId(index: number): string {
  return `topbar-search-option-${index}`;
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
  id,
  selected,
  onHover,
  onSelect,
  children,
}: {
  id: string;
  selected: boolean;
  onHover: () => void;
  onSelect: () => void;
  children: React.ReactNode;
}) {
  // Keep the keyboard-highlighted row in view as ↑/↓ walk past the
  // panel's scroll edge.
  const ref = React.useRef<HTMLButtonElement | null>(null);
  React.useEffect(() => {
    if (selected) ref.current?.scrollIntoView?.({ block: "nearest" });
  }, [selected]);
  return (
    <li>
      <button
        ref={ref}
        id={id}
        type="button"
        role="option"
        // Reached with ↑/↓ from the input, not with Tab.
        tabIndex={-1}
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
