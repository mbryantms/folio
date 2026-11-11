"use client";

import { ArrowRight, LibraryBig, Search } from "lucide-react";
import { Kbd } from "@/components/folio/kbd";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { ActionRowContent } from "@/components/search/ActionRowContent";
import { RecentSearches } from "@/components/search/RecentSearches";
import {
  RecentVisitsHeader,
  SearchHitRowContent,
  VisitRowContent,
} from "@/components/search/SearchResultRow";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { useGlobalSearch } from "@/lib/search/use-search";
import { useRecentSearches } from "@/lib/search/use-recent-searches";
import { useRecentVisits } from "@/lib/search/use-recent-visits";
import type { RecentVisit } from "@/lib/search/recent-visits";
import {
  groupSearchActions,
  parseCommandPrefix,
  type SearchAction,
} from "@/lib/search/actions-registry";
import { useSearchActions } from "@/lib/search/use-search-actions";
import { SEARCH_CATEGORIES, type SearchHit } from "@/lib/search/types";

const MAX_PER_CATEGORY = 5;
const QUERY_DEBOUNCE_MS = 200;

/**
 * Global search dialog — opened from anywhere via the `openSearch` keybind
 * or the layout's search button; closes on Escape or backdrop click.
 *
 * Built on `cmdk` (audit E2): cmdk owns the listbox/option roles,
 * `aria-activedescendant`, and the arrow/enter keyboard model, so we no
 * longer hand-roll a highlight index. Results are server-driven
 * (`useGlobalSearch`), so `shouldFilter={false}` disables cmdk's own
 * substring filter — it only does selection + a11y, not matching.
 *
 * A `>` prefix flips into command mode (action registry only, grouped by
 * area: Actions / Library / Settings / Admin). An empty input offers a
 * few suggested commands (continue reading, theme, sidebar, …) so the
 * palette is keyboard-useful before the first keystroke; typed queries
 * lead with up to six matching actions above the content hits. Actions
 * come from `useSearchActions`, shared with the desktop topbar dropdown.
 * The footer links to the dedicated `/search` page; `Mod+Enter` jumps
 * there.
 *
 * Below `sm` it is the phone's search surface (the bottom tab bar's
 * Search tab opens it): a sheet from the top edge, full width, with a
 * Cancel beside the input because a phone has no Escape key and the
 * backdrop sits under the keyboard. The dialog's own corner close button
 * is off at every width — it overlaid the input's right end.
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
  const recents = useRecentSearches();
  const recentVisits = useRecentVisits();

  const { needle: rawForActions, commandMode } = parseCommandPrefix(raw);

  // Debounce the content query (200ms). Command mode skips it — action
  // filtering is client-side and feels nicer keystroke-by-keystroke.
  useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);
  const contentQuery = commandMode ? "" : debounced;

  const { enabled, isLoading, groups, total, suggestions } = useGlobalSearch(
    contentQuery,
    { perCategory: MAX_PER_CATEGORY },
  );

  // Role-gated actions, recomputed per keystroke. Non-admins never see
  // admin destinations even via direct keyword match.
  const {
    actions,
    run: runAction,
    detailFor,
    keyHintFor,
  } = useSearchActions({ needle: rawForActions, commandMode, active: open });
  const suggesting = !commandMode && rawForActions.trim().length === 0;
  const actionSections = commandMode
    ? groupSearchActions(actions).map((g) => ({
        heading: g.group,
        actions: g.actions,
      }))
    : actions.length > 0
      ? [{ heading: suggesting ? "Suggested" : "Jump to…", actions }]
      : [];

  const contentSections = commandMode
    ? []
    : SEARCH_CATEGORIES.map((def) => ({
        def,
        hits: groups[def.key].slice(0, MAX_PER_CATEGORY),
      })).filter((s) => s.hits.length > 0);

  // Every category came back empty (U-11): say so, offer the server's
  // "Did you mean" series names as items, and a Browse link.
  const noResults = !commandMode && enabled && !isLoading && total === 0;
  const suggestionItems = noResults ? suggestions : [];

  const hasItems =
    actions.length > 0 ||
    suggestionItems.length > 0 ||
    contentSections.some((s) => s.hits.length > 0);
  // Empty-input states: recents chips (if any) or a short hint, above the
  // suggested commands. Shown only when there's nothing to search yet
  // (not enabled, not command mode).
  const showRecents = !commandMode && !enabled && recents.recents.length > 0;
  // Recently visited detail pages: real cmdk items (arrow-navigable),
  // above the suggested commands.
  const showVisits = !commandMode && !enabled && recentVisits.visits.length > 0;
  const showHint =
    !commandMode &&
    !enabled &&
    recents.recents.length === 0 &&
    recentVisits.visits.length === 0;

  const fullSearchHref = enabled
    ? `/search?q=${encodeURIComponent(contentQuery)}`
    : "/search";

  function handleOpenChange(next: boolean) {
    if (!next) {
      setRaw("");
      setDebounced("");
    }
    onOpenChange(next);
  }

  function commitVisit(visit: RecentVisit) {
    handleOpenChange(false);
    router.push(visit.href);
  }

  function commitHit(hit: SearchHit) {
    if (contentQuery) recents.add(contentQuery);
    handleOpenChange(false);
    router.push(hit.href);
  }
  // A command that opens another dialog (the shortcuts sheet) must keep
  // focus there: Radix would otherwise hand focus back to this dialog's
  // trigger on close, which reads as "focus outside" to the new dialog
  // and dismisses it.
  const keepFocusOnClose = useRef(false);
  function commitAction(action: SearchAction) {
    keepFocusOnClose.current = action.command === "show-shortcuts";
    handleOpenChange(false);
    runAction(action);
  }
  function runSuggestion(q: string) {
    setRaw(q);
    setDebounced(q);
  }
  function commitBrowse() {
    handleOpenChange(false);
  }
  function commitFullSearch() {
    if (contentQuery) recents.add(contentQuery);
    handleOpenChange(false);
    router.push(fullSearchHref);
  }

  // cmdk owns Enter for the highlighted item. We intercept two cases at
  // the input (stopping propagation so cmdk doesn't also act): Mod+Enter
  // → full search, and plain Enter with no quick hits → full search.
  function onInputKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "Enter") return;
    if (!commandMode && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.stopPropagation();
      commitFullSearch();
    } else if (!commandMode && enabled && !hasItems) {
      e.preventDefault();
      e.stopPropagation();
      commitFullSearch();
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        showCloseButton={false}
        onCloseAutoFocus={(e) => {
          if (keepFocusOnClose.current) {
            keepFocusOnClose.current = false;
            e.preventDefault();
          }
        }}
        // Phones: globals.css turns every dialog into a bottom sheet; this
        // attribute picks the top-sheet geometry there instead (the input
        // stays put while the keyboard owns the bottom of the screen).
        data-search-sheet=""
        className="flex max-h-[80vh] flex-col gap-0 overflow-hidden p-0 sm:top-[10%] sm:max-w-xl sm:translate-y-0"
      >
        <DialogTitle className="sr-only">Search</DialogTitle>
        <DialogDescription className="sr-only">
          Quick search across your library. Press Enter to open the highlighted
          result, or Mod+Enter to jump to the full results page.
        </DialogDescription>
        {/* `shouldFilter={false}`: results are server-filtered; cmdk only
            does selection + ARIA. `loop` wraps arrow nav at the ends. */}
        <Command
          shouldFilter={false}
          loop
          className="flex min-h-0 flex-1 flex-col gap-0 bg-transparent"
        >
          <div className="flex items-start [&>[data-slot=command-input-wrapper]]:min-w-0 [&>[data-slot=command-input-wrapper]]:flex-1">
            <CommandInput
              autoFocus
              value={raw}
              onValueChange={setRaw}
              onKeyDown={onInputKeyDown}
              placeholder={
                commandMode
                  ? "Jump to settings, admin pages, sections…"
                  : "Search series, issues, people, characters…  Type > for commands"
              }
              aria-label="Search the library"
            />
            {/* Phones: no Escape key, and the backdrop is under the
                keyboard, so the sheet closes from here. */}
            <DialogClose asChild>
              <Button
                variant="ghost"
                size="sm"
                className="mt-1 mr-1 h-8 shrink-0 sm:hidden"
              >
                Cancel
              </Button>
            </DialogClose>
          </div>
          {showRecents ? (
            <RecentSearches
              recents={recents.recents}
              onPick={(q) => {
                setRaw(q);
                setDebounced(q);
              }}
              onRemove={recents.remove}
              onClear={recents.clear}
              className={showVisits ? "flex-none" : undefined}
            />
          ) : null}
          {showVisits ? (
            <RecentVisitsHeader onClear={recentVisits.clear} />
          ) : null}
          <CommandList className="min-h-0 flex-1 max-sm:max-h-none">
            {isLoading && !hasItems ? (
              <p
                role="status"
                className="text-muted-foreground px-4 py-6 text-center text-xs"
              >
                Searching…
              </p>
            ) : null}
            {/* cmdk auto-hides Empty while any item is mounted. Nothing
                  to be empty about before the user has typed. */}
            {noResults ? (
              <div className="text-muted-foreground space-y-1 px-4 pt-5 pb-3 text-center text-xs">
                <p>No matches for &ldquo;{contentQuery}&rdquo;.</p>
                <Link
                  href="/browse"
                  onClick={commitBrowse}
                  className="text-foreground focus-visible:ring-ring inline-flex items-center gap-1 rounded-sm font-medium underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:outline-none"
                >
                  <LibraryBig className="size-3.5" aria-hidden="true" />
                  Browse the library
                </Link>
              </div>
            ) : null}
            {suggestionItems.length > 0 ? (
              <CommandGroup heading="Did you mean">
                {suggestionItems.map((name) => (
                  <CommandItem
                    key={name}
                    value={`suggest:${name}`}
                    onSelect={() => runSuggestion(name)}
                    aria-label={`Search for ${name}`}
                    className="gap-3 px-3 py-2"
                  >
                    <Search
                      className="text-muted-foreground size-4 shrink-0"
                      aria-hidden="true"
                    />
                    <span className="truncate">{name}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            ) : null}
            {showHint || showRecents || showVisits || noResults ? null : (
              <CommandEmpty>
                {commandMode
                  ? "No commands match."
                  : isLoading
                    ? "Searching…"
                    : `No matches for “${contentQuery}”.`}
              </CommandEmpty>
            )}
            {showVisits ? (
              <>
                {/* The visible header (above the list) carries the
                    Clear button, since cmdk hides its own heading from
                    AT; this heading stays visually hidden and only
                    names the group. */}
                <CommandGroup
                  heading="Recently visited"
                  className="pt-0 **:[[cmdk-group-heading]]:sr-only"
                >
                  {recentVisits.visits.map((visit) => (
                    <CommandItem
                      key={`${visit.kind}:${visit.id}`}
                      value={`visit:${visit.kind}:${visit.id}`}
                      onSelect={() => commitVisit(visit)}
                      className="gap-3 px-3 py-2"
                    >
                      <VisitRowContent visit={visit} />
                    </CommandItem>
                  ))}
                </CommandGroup>
              </>
            ) : null}
            {actionSections.map((section) => (
              <CommandGroup key={section.heading} heading={section.heading}>
                {section.actions.map((action) => (
                  <CommandItem
                    key={action.id}
                    value={`action:${action.id}`}
                    onSelect={() => commitAction(action)}
                    className="gap-3 px-3 py-2"
                  >
                    <ActionRowContent
                      action={action}
                      detail={detailFor(action)}
                      keyHint={keyHintFor(action)}
                    />
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
            {contentSections.map(({ def, hits }) => (
              <CommandGroup key={def.key} heading={def.labelPlural}>
                {hits.map((hit) => (
                  <CommandItem
                    key={hit.id}
                    value={`${def.key}:${hit.id}`}
                    onSelect={() => commitHit(hit)}
                    className="gap-3 px-3 py-2"
                  >
                    <SearchHitRowContent hit={hit} query={contentQuery} />
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
          {showHint ? <SearchHints /> : null}
          <SearchFooter
            enabled={enabled}
            query={debounced}
            total={total}
            href={fullSearchHref}
            onNavigate={commitFullSearch}
          />
        </Command>
      </DialogContent>
    </Dialog>
  );
}

/** Short "type to search" prompt + key hints, shown under the suggested
 *  commands when the input is empty and there are no recents. */
function SearchHints() {
  return (
    <div className="text-muted-foreground border-border space-y-2 border-t px-4 py-3 text-center text-xs">
      <p>
        Type at least 2 characters to search, or <KbdGlyph>&gt;</KbdGlyph>for
        commands.
      </p>
      <p className="max-sm:hidden">
        <KbdGlyph>↵</KbdGlyph> opens the highlighted hit ·{" "}
        <KbdGlyph>⌘</KbdGlyph>
        <KbdGlyph>↵</KbdGlyph> shows full results
      </p>
    </div>
  );
}

function KbdGlyph({ children }: { children: React.ReactNode }) {
  return <Kbd className="mx-1">{children}</Kbd>;
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
          <Kbd key={i} className="h-5 min-w-5">
            {k}
          </Kbd>
        ))}
      </span>
      <span>{label}</span>
    </span>
  );
}
