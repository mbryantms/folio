"use client";

import { Search } from "lucide-react";
import { Kbd } from "@/components/ui/kbd";
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
    // The keycap hint sits *beside* the button, not inside it. axe-core
    // 4.14 enables `label-content-name-mismatch` (WCAG 2.5.3 Label in
    // Name) by default, and every word visibly inside a control must
    // appear in its accessible name; a `Ctrl K` keycap in the subtree
    // would have to be part of the name. As a pointer-transparent
    // sibling it stays a purely visual hint.
    <div className={cn("relative sm:w-full sm:max-w-md", className)}>
      <button
        type="button"
        onClick={() => setOpen(true)}
        // The same words as the visible placeholder (minus the ellipsis)
        // so speech-input users can say what they see; on phones the
        // placeholder is hidden and this is the whole name.
        aria-label="Search series, issues, people"
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
          // input even before the icon prefix lands. `md:pr-16` keeps the
          // placeholder clear of the keycap overlaid on the right.
          "sm:w-full sm:justify-start sm:gap-2 sm:px-3 md:pr-16",
        )}
      >
        <Search
          aria-hidden="true"
          className="text-muted-foreground size-4 shrink-0"
        />
        <span className="text-muted-foreground hidden flex-1 truncate text-left sm:block">
          Search series, issues, people…
        </span>
      </button>
      <Kbd
        aria-hidden="true"
        suppressHydrationWarning
        className="bg-background text-muted-foreground pointer-events-none absolute top-1/2 right-3 hidden h-5 -translate-y-1/2 gap-0.5 px-1.5 md:inline-flex"
      >
        <span>{modGlyph}</span>
        <span>K</span>
      </Kbd>
    </div>
  );
}
