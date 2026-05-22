"use client";

import { Search } from "lucide-react";
import { useEffect, useState } from "react";

import { useSearchModal } from "@/lib/search/use-search-modal";
import { cn } from "@/lib/utils";

/** Topbar search affordance: a button that visually mimics a search input
 *  (icon + placeholder + ⌘K hint) but opens the rich `<SearchModal>` on
 *  click. Same surface as the `Mod+K` and `/` hotkeys, so the button and
 *  the keyboard both land in one place.
 *
 *  Two layouts driven by viewport width:
 *  - mobile (`< sm`): icon-only square button. Keeps the topbar tight on
 *    phones; the modal handles the actual input.
 *  - `sm+`: full input-shaped button with placeholder text and a `⌘K`
 *    kbd hint when there's room (`md+`).
 *
 *  The component lives in MainShell's flex row, so it grows to fill the
 *  available space between the brand label and the right-edge chrome,
 *  capped at `max-w-md` to avoid stretching across very wide viewports. */
export function TopbarSearchTrigger({ className }: { className?: string }) {
  const { setOpen } = useSearchModal();
  // Render the `⌘K` glyph only after hydration to avoid an SSR/CSR
  // mismatch — the server doesn't know the user's OS so we'd otherwise
  // ship `Ctrl K` and replace it after first paint.
  const [isMac, setIsMac] = useState(false);
  useEffect(() => {
    if (typeof navigator === "undefined") return;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setIsMac(/Mac|iPhone|iPad|iPod/.test(navigator.platform));
  }, []);
  const modGlyph = isMac ? "⌘" : "Ctrl";

  return (
    <button
      type="button"
      onClick={() => setOpen(true)}
      aria-label="Open search"
      aria-haspopup="dialog"
      className={cn(
        // Match the height of the topbar's other affordances (h-9 in
        // `<Input>`-land). `bg-muted/40` reads as a subtle inset that
        // looks input-ish without competing with surrounding chrome,
        // and `hover:bg-muted/70` gives a tactile press feel.
        "border-border bg-muted/40 hover:bg-muted/70 focus-visible:ring-ring inline-flex h-9 items-center rounded-md border text-sm transition-colors focus-visible:ring-2 focus-visible:outline-none",
        // Mobile: icon-only square, sits flush with the other ghost
        // buttons on the row.
        "size-9 justify-center px-0",
        // sm+: grow to fill, max ~360px so the topbar still feels tight
        // on wide viewports. `text-left` so the placeholder reads as an
        // input even before the icon prefix lands.
        "sm:w-full sm:max-w-md sm:justify-start sm:gap-2 sm:px-3",
        className,
      )}
    >
      <Search
        aria-hidden="true"
        className="text-muted-foreground size-4 shrink-0"
      />
      <span className="text-muted-foreground hidden flex-1 truncate text-left sm:block">
        Search series, issues, people…
      </span>
      <kbd
        aria-hidden="true"
        suppressHydrationWarning
        className="bg-background text-muted-foreground border-border hidden h-5 items-center gap-0.5 rounded border px-1.5 font-mono text-[10px] leading-none md:inline-flex"
      >
        <span>{modGlyph}</span>
        <span>K</span>
      </kbd>
    </button>
  );
}
