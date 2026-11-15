import { cn } from "@/lib/utils";

/**
 * The Folio lockup: the app icon's book glyph in the accent colour beside
 * the name drawn as outlines.
 *
 * Both halves are inline SVG, so the lockup looks the same on every
 * device (no web font, no system-font fallback) and follows the theme:
 * the glyph and the dot of the "i" take `--primary`, the letters take the
 * text colour. The masters are `public/brand/icon-master.svg` (its five
 * paths without the tile) and `public/brand/logotype-master.svg`.
 *
 * This file imports no router code on purpose. `StatusScreen` renders the
 * lockup from the error boundaries, which every route loads, the reader
 * included; `next/link` here would land in the reader's first-load bundle
 * (it cost 7 KB against the budget in `scripts/check-bundle-size.mjs`).
 * The shells' link is `Wordmark`, in `Wordmark.tsx`.
 */

/** Classes for the element that carries the lockup: an `<a>` or a
 *  `<Link>`. `folio-tap`: the lockup is 24px tall; touch gets the 44px
 *  area. */
export const WORDMARK_LINK_CLASS =
  "folio-tap text-foreground focus-visible:ring-ring focus-visible:ring-offset-background inline-flex shrink-0 items-center gap-[9px] rounded-sm outline-none transition-opacity hover:opacity-80 focus-visible:ring-2 focus-visible:ring-offset-4";

/** The two drawings, hidden from assistive tech: the link that holds
 *  them is named "Folio". */
export function WordmarkLockup() {
  return (
    <>
      <svg
        aria-hidden="true"
        focusable="false"
        viewBox="100 126 312 272"
        width="27"
        height="24"
        className="text-primary shrink-0"
        fill="currentColor"
      >
        <path d="M246 156C204 138 158 132 108 138V244H246Z" />
        <path d="M108 262V372C158 366 204 372 246 390V262Z" />
        <path d="M266 156C308 138 354 132 404 138V222H266Z" />
        <path d="M266 240V390C284 382 302 377 320 373V240Z" />
        <path d="M338 240V370C360 367 382 367 404 372V240Z" />
      </svg>
      <svg
        aria-hidden="true"
        focusable="false"
        viewBox="0 0 270 100"
        width="43.2"
        height="16"
        className="shrink-0"
        fill="currentColor"
      >
        <path d="M0 0h58v16H16v26h32v16H16v42H0z" />
        <circle
          cx="96"
          cy="66"
          r="26"
          fill="none"
          stroke="currentColor"
          strokeWidth="16"
        />
        <path d="M142 0h16v100h-16z" />
        <path d="M174 32h16v68h-16z" />
        <circle cx="182" cy="10" r="10" className="text-primary" />
        <circle
          cx="236"
          cy="66"
          r="26"
          fill="none"
          stroke="currentColor"
          strokeWidth="16"
        />
      </svg>
    </>
  );
}

/** The lockup as a plain `<a href>`, for `StatusScreen`, which also
 *  renders from `global-error` where the router may be gone. */
export function WordmarkAnchor({
  href,
  className,
}: {
  href: string;
  className?: string;
}) {
  return (
    <a
      href={href}
      aria-label="Folio"
      data-wordmark=""
      className={cn(WORDMARK_LINK_CLASS, className)}
    >
      <WordmarkLockup />
    </a>
  );
}
