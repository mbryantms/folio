"use client";

import { useReaderStore } from "@/lib/reader/store";
import { formatKey } from "@/lib/reader/keybinds";
import { usePageTextPanel } from "@/lib/reader/page-text";

const LINK_CLASS =
  "sr-only focus-visible:not-sr-only focus-visible:bg-background focus-visible:text-foreground focus-visible:ring-ring focus-visible:fixed focus-visible:top-[max(0.75rem,var(--safe-top))] focus-visible:left-[max(0.75rem,var(--safe-left))] focus-visible:z-50 focus-visible:rounded-md focus-visible:px-4 focus-visible:py-2 focus-visible:text-sm focus-visible:font-medium focus-visible:shadow-lg focus-visible:ring-2 focus-visible:outline-none";

/**
 * Skip-link style entry points for keyboard and screen-reader users
 * (WP-4.8, audit AC-2). The reader chrome is hidden (and `inert`) by
 * default, so without these the first Tab lands nowhere useful and the
 * only way to the controls is knowing the `t` shortcut. They are the
 * first focusable elements in the reader, invisible until focused, and
 * name their shortcut so the key is learned on the way.
 *
 * "Show reader controls" reveals the chrome and moves focus onto its
 * first button (focus inside the header pins auto-hide, see
 * ReaderChrome). "Show page text" opens the OCR page-text panel.
 */
export function ReaderSkipLinks({
  toggleChromeKey,
  pageTextKey,
}: {
  /** Resolved (possibly user-rebound) key specs, for the labels. */
  toggleChromeKey: string;
  pageTextKey: string;
}) {
  const setChromeVisible = useReaderStore((s) => s.setChromeVisible);
  const setPageTextOpen = usePageTextPanel((s) => s.setOpen);

  const showControls = () => {
    setChromeVisible(true);
    // The header drops `inert` on the next commit; wait two frames so
    // the button is focusable before moving focus onto it.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        document
          .querySelector<HTMLElement>('[data-testid="reader-chrome"] button')
          ?.focus();
      }),
    );
  };

  return (
    <nav aria-label="Reader shortcuts">
      <button
        type="button"
        onClick={showControls}
        aria-keyshortcuts={toggleChromeKey}
        className={LINK_CLASS}
      >
        Show reader controls ({formatKey(toggleChromeKey)})
      </button>
      <button
        type="button"
        onClick={() => setPageTextOpen(true)}
        aria-keyshortcuts={pageTextKey}
        className={LINK_CLASS}
      >
        Show page text ({formatKey(pageTextKey)})
      </button>
    </nav>
  );
}
