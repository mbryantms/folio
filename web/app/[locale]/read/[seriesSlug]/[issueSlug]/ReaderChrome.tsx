"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import {
  ArrowLeft,
  Bookmark,
  BookmarkCheck,
  EyeOff,
  ImageIcon,
  Maximize2,
  Minimize2,
  Settings,
  Square,
  Star,
  StickyNote,
  Type,
} from "lucide-react";
import { useReaderStore } from "@/lib/reader/store";
import type { Direction } from "@/lib/reader/detect";
import { useFullscreen } from "@/lib/reader/fullscreen";
import { useIssueMarkers } from "@/lib/api/queries";
import {
  useCreateMarker,
  useDeleteMarker,
  useUpdateMarker,
} from "@/lib/api/mutations";
import { markerToCreateReq } from "@/lib/markers/recreate";
import { toast } from "sonner";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ReaderSettings } from "./ReaderSettings";

const AUTO_HIDE_MS = 4000;

/**
 * Top-bar overlay for the reader. Always mounted; visibility is driven by the
 * `chromeVisible` store flag and animated through `data-state`. Auto-hide is
 * paused while a popover is open or an input has focus (`chromePinned`).
 */
export function ReaderChrome({
  seriesId,
  issueId,
  exitUrl,
  totalPages,
  visiblePages,
  incognito = false,
}: {
  seriesId: string | null;
  /** Issue id is needed for marker mutations / queries fired from the
   *  chrome's bookmark + note + highlight buttons. */
  issueId: string;
  /** URL to navigate to when the user exits the reader — points at the
   * issue detail page. Computed by the parent so chrome doesn't need to
   * know the slug shape. */
  exitUrl: string;
  totalPages: number;
  /** Pages currently visible on screen (length 2 in double-page mode when
   * the active group is a pair). When omitted, the chrome falls back to
   * single-index display. */
  visiblePages?: readonly number[];
  /** When true, render an "Incognito" chip in the chrome so the user can
   *  see the read isn't being tracked. */
  incognito?: boolean;
}) {
  const router = useRouter();
  const currentPage = useReaderStore((s) => s.currentPage);
  const direction = useReaderStore((s) => s.direction);
  const chromeVisible = useReaderStore((s) => s.chromeVisible);
  const chromeAutoHide = useReaderStore((s) => s.chromeAutoHide);
  const chromePinned = useReaderStore((s) => s.chromePinned);
  const setChromeVisible = useReaderStore((s) => s.setChromeVisible);
  const setChromePinned = useReaderStore((s) => s.setChromePinned);

  useChromeAutoHide({
    enabled: chromeAutoHide,
    visible: chromeVisible,
    pinned: chromePinned,
    setVisible: setChromeVisible,
  });

  const onExit = useCallback(() => {
    router.push(exitUrl);
  }, [exitUrl, router]);

  return (
    <TooltipProvider delayDuration={250}>
      <header
        data-state={chromeVisible ? "open" : "closed"}
        data-testid="reader-chrome"
        className="fixed inset-x-0 top-0 z-30 flex items-center gap-2 border-b border-neutral-800/80 bg-neutral-950/85 px-3 py-2 text-sm text-neutral-100 backdrop-blur transition-transform duration-300 ease-out data-[state=closed]:pointer-events-none data-[state=closed]:-translate-y-full motion-reduce:transition-none"
        aria-hidden={chromeVisible ? undefined : true}
      >
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={onExit}
              aria-label="Exit reader"
              className="focus-visible:ring-ring inline-flex h-9 w-9 items-center justify-center rounded-md text-neutral-100 transition-colors hover:bg-white/15 hover:text-white focus-visible:ring-2 focus-visible:outline-none [&_svg]:size-4"
            >
              <ArrowLeft />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Exit reader</TooltipContent>
        </Tooltip>

        <PageJumpDisplay
          currentPage={currentPage}
          totalPages={totalPages}
          direction={direction}
          visiblePages={visiblePages}
          onPin={setChromePinned}
        />

        {incognito && (
          <span
            className="ml-2 inline-flex items-center gap-1 rounded-full border border-amber-400/40 bg-amber-400/10 px-2 py-0.5 text-[11px] font-medium tracking-wider text-amber-200 uppercase"
            aria-label="Reading in incognito mode — progress and activity will not be saved"
          >
            <EyeOff className="h-3 w-3" />
            Incognito
          </span>
        )}

        <span className="ml-auto flex items-center gap-1">
          <BookmarkToggleButton issueId={issueId} pageIndex={currentPage} />
          <FavoriteToggleButton issueId={issueId} pageIndex={currentPage} />
          <MarkerMenuButton issueId={issueId} pageIndex={currentPage} />
          <SettingsButton seriesId={seriesId} onPinChange={setChromePinned} />
          <FullscreenButton />
        </span>
      </header>
    </TooltipProvider>
  );
}

/**
 * Click-to-jump page indicator. Reads as static text by default; click
 * (or focus + Enter) toggles a number input. Submit or blur jumps; Escape
 * cancels. Pins the chrome while editing so the auto-hide timer doesn't
 * yank the input out from under the caret.
 */
function PageJumpDisplay({
  currentPage,
  totalPages,
  direction,
  visiblePages,
  onPin,
}: {
  currentPage: number;
  totalPages: number;
  direction: Direction;
  visiblePages?: readonly number[];
  onPin: (pinned: boolean) => void;
}) {
  const setPage = useReaderStore((s) => s.setPage);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  const pairLabel =
    visiblePages && visiblePages.length === 2
      ? `Pages ${visiblePages[0]! + 1} to ${visiblePages[1]! + 1} of ${totalPages}; click to jump`
      : null;

  const beginEdit = () => {
    setDraft(String(currentPage + 1));
    setEditing(true);
    onPin(true);
  };
  const commit = () => {
    const n = Number.parseInt(draft, 10);
    if (Number.isFinite(n)) {
      const clamped = Math.max(1, Math.min(totalPages, n));
      setPage(clamped - 1);
    }
    setEditing(false);
    onPin(false);
  };
  const cancel = () => {
    setEditing(false);
    onPin(false);
  };

  if (editing) {
    return (
      <span className="ml-1 flex items-center gap-1 text-neutral-400">
        Page
        <input
          type="number"
          min={1}
          max={totalPages}
          step={1}
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancel();
            } else {
              // Don't bubble — the reader's keyboard handler would catch
              // ArrowLeft/Right and move pages while you're typing.
              e.stopPropagation();
            }
          }}
          onFocus={(e) => e.currentTarget.select()}
          className="focus:ring-ring w-14 [appearance:textfield] rounded border border-neutral-700 bg-neutral-900 px-2 py-0.5 text-neutral-200 focus:ring-1 focus:outline-none [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
          aria-label={`Jump to page (1–${totalPages})`}
        />
        <span className="text-neutral-600">/</span> {totalPages}
      </span>
    );
  }

  return (
    <button
      type="button"
      onClick={beginEdit}
      className="focus-visible:ring-ring ml-1 cursor-text rounded px-1 text-neutral-200 transition-colors hover:bg-white/15 hover:text-white focus-visible:ring-2 focus-visible:outline-none"
      aria-label={
        pairLabel ?? `Page ${currentPage + 1} of ${totalPages}; click to jump`
      }
    >
      {pairLabel ? (
        <>
          Pages {visiblePages![0]! + 1}
          <span className="text-neutral-600">–</span>
          {visiblePages![1]! + 1} <span className="text-neutral-600">/</span>{" "}
          {totalPages}
        </>
      ) : (
        <>
          Page {currentPage + 1} <span className="text-neutral-600">/</span>{" "}
          {totalPages}
        </>
      )}
      {direction === "rtl" ? (
        <span className="ml-2 rounded border border-neutral-700 px-1 text-[10px] tracking-wider text-neutral-400 uppercase">
          RTL
        </span>
      ) : null}
    </button>
  );
}

/** Toggle a page-level bookmark on the current page. Reads the
 *  per-issue marker list once and looks for a `kind='bookmark'` row
 *  on `pageIndex` with no region — when present, the click deletes
 *  it; when absent, a new whole-page bookmark is created. */
function BookmarkToggleButton({
  issueId,
  pageIndex,
}: {
  issueId: string;
  pageIndex: number;
}) {
  const markers = useIssueMarkers(issueId);
  const existing = useMemo(
    () =>
      (markers.data?.items ?? []).find(
        (m) => m.kind === "bookmark" && m.page_index === pageIndex && !m.region,
      ),
    [markers.data, pageIndex],
  );
  const create = useCreateMarker();
  // The delete hook is keyed on a marker id; only mint it when we
  // actually have one to delete.
  const del = useDeleteMarker(existing?.id ?? "", issueId, { silent: true });

  const onClick = () => {
    if (existing) {
      const snapshot = existing;
      del.mutate(undefined, {
        onSuccess: () =>
          toast.success("Bookmark removed", {
            action: {
              label: "Undo",
              onClick: () => create.mutate(markerToCreateReq(snapshot)),
            },
          }),
      });
      return;
    }
    create.mutate({
      issue_id: issueId,
      page_index: pageIndex,
      kind: "bookmark",
    });
  };

  return (
    <ChromeIconButton
      label={existing ? "Remove bookmark" : "Bookmark this page"}
      icon={existing ? <BookmarkCheck /> : <Bookmark />}
      onClick={onClick}
      active={!!existing}
    />
  );
}

/** Star toggle for the current page. Markers (any kind) carry an
 *  `is_favorite` flag; this button flips that bit on the page's
 *  page-level bookmark, creating one if there isn't one yet. A page
 *  can therefore be "starred" without explicitly being bookmarked —
 *  starring auto-bookmarks since favorite needs a parent row. Removing
 *  the star un-bookmarks the page so we don't leave orphan rows. */
function FavoriteToggleButton({
  issueId,
  pageIndex,
}: {
  issueId: string;
  pageIndex: number;
}) {
  const markers = useIssueMarkers(issueId);
  // Use ANY page-level (region NULL) marker as the favorite carrier;
  // bookmark is the canonical kind but a note or anchored highlight
  // can also be starred. Lookup order matches the chrome's bookmark
  // semantics: page-level row only.
  const existing = useMemo(
    () =>
      (markers.data?.items ?? []).find(
        (m) => m.page_index === pageIndex && !m.region,
      ),
    [markers.data, pageIndex],
  );
  const create = useCreateMarker();
  const update = useUpdateMarker(existing?.id ?? "", issueId);
  const del = useDeleteMarker(existing?.id ?? "", issueId, { silent: true });

  const onClick = () => {
    if (existing) {
      if (existing.is_favorite) {
        // Unstar — if the row only existed to carry the star (no body),
        // delete it. Otherwise just clear the flag so the user's note /
        // bookmark survives.
        const hasOtherContent =
          (existing.body && existing.body.length > 0) ||
          existing.kind !== "bookmark";
        if (hasOtherContent) {
          update.mutate({ is_favorite: false });
        } else {
          const snapshot = existing;
          del.mutate(undefined, {
            onSuccess: () =>
              toast.success("Star removed", {
                action: {
                  label: "Undo",
                  onClick: () => create.mutate(markerToCreateReq(snapshot)),
                },
              }),
          });
        }
      } else {
        update.mutate({ is_favorite: true });
      }
      return;
    }
    // No page-level marker exists — create a starred bookmark to
    // carry the flag.
    create.mutate({
      issue_id: issueId,
      page_index: pageIndex,
      kind: "bookmark",
      is_favorite: true,
    });
  };

  const starred = !!existing?.is_favorite;
  return (
    <ChromeIconButton
      label={starred ? "Unstar this page" : "Favorite this page"}
      icon={<Star className={starred ? "fill-current" : undefined} />}
      onClick={onClick}
      active={starred}
    />
  );
}

/** Dropdown that hosts add-note + highlight modes. Clicking "Add note"
 *  drops a `PendingMarker` of kind `note` (page-level) straight onto
 *  the store so the editor sheet opens without requiring a region
 *  drag. Highlight modes flip the marker overlay into the matching
 *  `select-*` mode and rely on the overlay's drag handler to push a
 *  pending marker on release. */
function MarkerMenuButton({
  issueId,
  pageIndex,
}: {
  issueId: string;
  pageIndex: number;
}) {
  void issueId; // future: per-issue analytics
  const beginMarkerEdit = useReaderStore((s) => s.beginMarkerEdit);
  const setMarkerMode = useReaderStore((s) => s.setMarkerMode);
  const setChromePinned = useReaderStore((s) => s.setChromePinned);

  function openNote() {
    beginMarkerEdit({
      kind: "note",
      page_index: pageIndex,
      region: null,
      selection: null,
      body: "",
      is_favorite: false,
      tags: [],
    });
  }
  function startHighlight(
    mode: "select-rect" | "select-text" | "select-image",
  ) {
    setMarkerMode(mode);
  }

  return (
    <DropdownMenu onOpenChange={setChromePinned}>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Marker tools"
              className="focus-visible:ring-ring data-[state=open]:bg-accent/25 data-[state=open]:text-accent inline-flex h-9 w-9 items-center justify-center rounded-md text-neutral-100 transition-colors hover:bg-white/15 hover:text-white focus-visible:ring-2 focus-visible:outline-none [&_svg]:size-4"
            >
              <StickyNote />
            </button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom">Markers</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" sideOffset={8} className="min-w-[18rem]">
        <DropdownMenuItem onSelect={openNote} className="flex-col items-start">
          <span className="flex items-center font-medium">
            <StickyNote className="mr-2 h-4 w-4" /> Add note
          </span>
          <span className="text-muted-foreground ml-6 text-xs">
            Page-level markdown note. Optional panel selection.
          </span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => startHighlight("select-rect")}
          className="flex-col items-start"
        >
          <span className="flex items-center font-medium">
            <Square className="mr-2 h-4 w-4" /> Highlight a region
          </span>
          <span className="text-muted-foreground ml-6 text-xs">
            Drag a rectangle. Saves just the box — fastest.
          </span>
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() => startHighlight("select-text")}
          className="flex-col items-start"
        >
          <span className="flex items-center font-medium">
            <Type className="mr-2 h-4 w-4" /> Highlight + capture text
          </span>
          <span className="text-muted-foreground ml-6 text-xs">
            Runs OCR on the dragged region so the text shows up in search. Takes
            a few seconds.
          </span>
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() => startHighlight("select-image")}
          className="flex-col items-start"
        >
          <span className="flex items-center font-medium">
            <ImageIcon className="mr-2 h-4 w-4" /> Highlight + image hash
          </span>
          <span className="text-muted-foreground ml-6 text-xs">
            Same as &ldquo;Highlight a region&rdquo; plus a fingerprint of the
            cropped pixels — reserved for a future &ldquo;find this panel&rdquo;
            lookup.
          </span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ChromeIconButton({
  label,
  icon,
  onClick,
  active,
}: {
  label: string;
  icon: React.ReactNode;
  onClick: () => void;
  /** When true, the button paints the accent color to signal an
   *  already-engaged state (e.g. page is currently bookmarked). */
  active?: boolean;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {/* Plain <button> instead of shadcn Button so the ghost variant's
         * `hover:bg-accent hover:text-accent-foreground` can't out-resolve
         * our hover styles via tailwind-merge. White wash + white icon
         * stays readable on any dark backdrop. */}
        <button
          type="button"
          onClick={onClick}
          aria-label={label}
          aria-pressed={active}
          className={
            "focus-visible:ring-ring inline-flex h-9 w-9 items-center justify-center rounded-md transition-colors hover:bg-white/15 hover:text-white focus-visible:ring-2 focus-visible:outline-none [&_svg]:size-4 " +
            (active ? "text-amber-300" : "text-neutral-100")
          }
        >
          {icon}
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  );
}

function SettingsButton({
  seriesId,
  onPinChange,
}: {
  seriesId: string | null;
  onPinChange: (pinned: boolean) => void;
}) {
  return (
    <Popover onOpenChange={onPinChange}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label="Reader settings"
              className="focus-visible:ring-ring data-[state=open]:bg-accent/25 data-[state=open]:text-accent inline-flex h-9 w-9 items-center justify-center rounded-md text-neutral-100 transition-colors hover:bg-white/15 hover:text-white focus-visible:ring-2 focus-visible:outline-none [&_svg]:size-4"
            >
              <Settings />
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom">Reader settings</TooltipContent>
      </Tooltip>
      <PopoverContent
        align="end"
        sideOffset={8}
        className="w-80 origin-top-right border-neutral-800 bg-neutral-950/95 text-neutral-100 transition-[opacity,transform] duration-150 ease-out data-[state=closed]:scale-95 data-[state=closed]:opacity-0 data-[state=open]:scale-100 data-[state=open]:opacity-100 motion-reduce:transition-none"
      >
        <ReaderSettings seriesId={seriesId} />
      </PopoverContent>
    </Popover>
  );
}

function FullscreenButton() {
  const { isFullscreen, toggle } = useFullscreen();
  return (
    <ChromeIconButton
      label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
      icon={isFullscreen ? <Minimize2 /> : <Maximize2 />}
      onClick={toggle}
    />
  );
}

/**
 * Auto-hide chrome after a period of input idle. Resets on any
 * pointer/keyboard/touch event on the document. Pauses while pinned (an open
 * popover or focused input) and never runs while disabled.
 */
function useChromeAutoHide({
  enabled,
  visible,
  pinned,
  setVisible,
}: {
  enabled: boolean;
  visible: boolean;
  pinned: boolean;
  setVisible: (v: boolean) => void;
}) {
  useEffect(() => {
    if (!enabled || !visible || pinned) return;
    if (typeof document === "undefined") return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const arm = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => setVisible(false), AUTO_HIDE_MS);
    };
    arm();
    document.addEventListener("pointermove", arm, { passive: true });
    document.addEventListener("keydown", arm);
    document.addEventListener("touchstart", arm, { passive: true });
    return () => {
      if (timer) clearTimeout(timer);
      document.removeEventListener("pointermove", arm);
      document.removeEventListener("keydown", arm);
      document.removeEventListener("touchstart", arm);
    };
  }, [enabled, visible, pinned, setVisible]);
}
