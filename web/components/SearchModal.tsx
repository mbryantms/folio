"use client";

import { ArrowRight, BookOpen, ChevronRight, Clock, X } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
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
import { SEARCH_CATEGORIES, type SearchHit } from "@/lib/search/types";
import { cn } from "@/lib/utils";

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
 * A `>` prefix flips into command mode (action registry only). The footer
 * links to the dedicated `/search` page; `Mod+Enter` jumps there.
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
  const recents = useRecentSearches();

  const { needle: rawForActions, commandMode } = parseCommandPrefix(raw);

  // Debounce the content query (200ms). Command mode skips it — action
  // filtering is client-side and feels nicer keystroke-by-keystroke.
  useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);
  const contentQuery = commandMode ? "" : debounced;

  const { enabled, isLoading, groups, total } = useGlobalSearch(contentQuery, {
    perCategory: MAX_PER_CATEGORY,
  });

  // Role-gated actions, recomputed per keystroke. Non-admins never see
  // admin destinations even via direct keyword match.
  const actions = useMemo<readonly SearchAction[]>(
    () =>
      rankSearchActions(rawForActions, me.data?.role).slice(
        0,
        SEARCH_ACTIONS_CAP,
      ),
    [rawForActions, me.data?.role],
  );

  const contentSections = commandMode
    ? []
    : SEARCH_CATEGORIES.map((def) => ({
        def,
        hits: groups[def.key].slice(0, MAX_PER_CATEGORY),
      })).filter((s) => s.hits.length > 0);

  const hasItems =
    actions.length > 0 || contentSections.some((s) => s.hits.length > 0);
  // Empty-input states: recents chips (if any) or a short hint. Shown
  // only when there's nothing to search yet (not enabled, not command
  // mode) — once the user types, cmdk's list takes over.
  const showRecents = !commandMode && !enabled && recents.recents.length > 0;
  const showHint = !commandMode && !enabled && recents.recents.length === 0;

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

  function commitHit(hit: SearchHit) {
    if (contentQuery) recents.add(contentQuery);
    handleOpenChange(false);
    router.push(hit.href);
  }
  function commitAction(action: SearchAction) {
    handleOpenChange(false);
    router.push(action.href);
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
      <DialogContent className="top-[10%] flex max-h-[80vh] max-w-xl translate-y-0 flex-col gap-0 overflow-hidden p-0">
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
          <CommandInput
            autoFocus
            value={raw}
            onValueChange={setRaw}
            onKeyDown={onInputKeyDown}
            placeholder={
              commandMode
                ? "Jump to settings, admin pages, sections…"
                : "Search series, issues, people…  Type > for commands"
            }
            aria-label="Search the library"
          />
          {showHint ? (
            <SearchHints />
          ) : showRecents ? (
            <RecentsSection
              recents={recents.recents}
              onPick={(q) => {
                setRaw(q);
                setDebounced(q);
              }}
              onRemove={recents.remove}
              onClear={recents.clear}
            />
          ) : (
            <CommandList className="min-h-0 flex-1">
              {isLoading && !hasItems ? (
                <p
                  role="status"
                  className="text-muted-foreground px-4 py-6 text-center text-xs"
                >
                  Searching…
                </p>
              ) : null}
              {/* cmdk auto-hides Empty while any item is mounted. */}
              <CommandEmpty>
                {commandMode
                  ? "No commands match."
                  : isLoading
                    ? "Searching…"
                    : `No matches for “${contentQuery}”.`}
              </CommandEmpty>
              {actions.length > 0 ? (
                <CommandGroup heading={commandMode ? "Commands" : "Jump to…"}>
                  {actions.map((action) => {
                    const Icon = action.icon;
                    return (
                      <CommandItem
                        key={action.id}
                        value={`action:${action.id}`}
                        onSelect={() => commitAction(action)}
                        className="gap-3 px-3 py-2"
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
                      </CommandItem>
                    );
                  })}
                </CommandGroup>
              ) : null}
              {contentSections.map(({ def, hits }) => (
                <CommandGroup key={def.key} heading={def.labelPlural}>
                  {hits.map((hit) => (
                    <CommandItem
                      key={hit.id}
                      value={`${def.key}:${hit.id}`}
                      onSelect={() => commitHit(hit)}
                      className="gap-3 px-3 py-2"
                    >
                      <Thumb hit={hit} />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {hit.title}
                        </span>
                        {hit.snippet ? (
                          // `<mark>`-highlighted excerpt (sanitised) wins
                          // over the static subtitle — it shows *why* the
                          // row matched.
                          <span
                            className="block truncate text-xs opacity-70 [&_mark]:rounded-sm [&_mark]:bg-amber-500/30 [&_mark]:px-0.5 [&_mark]:text-current"
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
                    </CommandItem>
                  ))}
                </CommandGroup>
              ))}
            </CommandList>
          )}
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

/** Short "type to search" prompt + key hints, shown when the input is
 *  empty and there are no recents. */
function SearchHints() {
  return (
    <div className="text-muted-foreground space-y-2 px-4 py-6 text-center text-xs">
      <p>Type at least 2 characters to search.</p>
      <p>
        <KbdGlyph>↵</KbdGlyph> opens the highlighted hit ·{" "}
        <KbdGlyph>⌘</KbdGlyph>
        <KbdGlyph>↵</KbdGlyph> shows full results
      </p>
    </div>
  );
}

/** Recent-search chips (empty-input state). The remove control is a
 *  44×44 touch target (audit E6) wrapping a smaller visual glyph. */
function RecentsSection({
  recents,
  onPick,
  onRemove,
  onClear,
}: {
  recents: readonly string[];
  onPick: (q: string) => void;
  onRemove: (q: string) => void;
  onClear: () => void;
}) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto py-2">
      <div className="flex items-center justify-between px-3 pb-1">
        <h3 className="text-muted-foreground inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-wide uppercase">
          <Clock className="size-3" aria-hidden="true" />
          Recent searches
        </h3>
        <button
          type="button"
          onClick={onClear}
          className="text-muted-foreground hover:text-foreground text-[11px] font-medium"
        >
          Clear all
        </button>
      </div>
      <ul className="flex flex-wrap gap-1.5 px-3 pb-3">
        {recents.map((q) => (
          <li key={q}>
            <span className="bg-muted/60 hover:bg-muted text-foreground border-border inline-flex items-center rounded-full border pl-3 text-xs transition-colors">
              <button
                type="button"
                onClick={() => onPick(q)}
                className="py-1.5 focus-visible:outline-none"
                aria-label={`Search again for ${q}`}
              >
                {q}
              </button>
              <button
                type="button"
                onClick={() => onRemove(q)}
                aria-label={`Remove ${q} from recents`}
                className="text-muted-foreground hover:text-foreground inline-flex size-11 items-center justify-center rounded-full"
              >
                <X className="size-3" aria-hidden="true" />
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function KbdGlyph({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="bg-muted text-foreground border-border mx-1 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
      {children}
    </kbd>
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
