import type { LucideIcon } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * What a cover chip says about its card. Colour is never the only
 * carrier: every tone but `neutral` also draws an icon or a dot.
 *
 *  - `neutral`: context, not status (a list position, a release day, a
 *    page number, a publication status).
 *  - `success` / `warning` / `info`: status, as coloured text with a mark.
 *  - `error`: something is broken or failed. The one solid chip.
 */
export type CoverChipTone =
  "neutral" | "success" | "warning" | "info" | "error";

/**
 * The text pill drawn on cover art (docs/dev/card-corner-conventions.md,
 * "The pill"). One shape for every card: 20px tall, 12px medium text,
 * sentence case, on a near-opaque backing with a hairline ring, so it
 * reads on any art without relying on `backdrop-filter`.
 *
 * The look lives in `styles/globals.css` (`.cover-chip`), where each
 * tone's text is mixed toward the foreground until it clears 4.5:1 on
 * the backing in every theme; `tests/ui/cover-chip-contrast.test.ts`
 * holds it there. Do not restyle a chip at the call site beyond its
 * position: `statusTone()`'s tinted surface, a `<Badge>` with a
 * translucent background and hand-built spans are what this replaces.
 *
 * Without children it is an icon alone (give it `role="img"` and an
 * `aria-label`). The round "read" mark is `ReadCheck`, not a chip.
 */
export function CoverChip({
  tone = "neutral",
  icon: Icon,
  className,
  children,
  ...rest
}: React.HTMLAttributes<HTMLSpanElement> & {
  tone?: CoverChipTone;
  /** The mark beside the text. A status tone without one draws a dot. */
  icon?: LucideIcon;
}) {
  return (
    <span
      data-cover-chip=""
      data-tone={tone}
      className={cn("cover-chip", className)}
      {...rest}
    >
      {Icon ? (
        <Icon aria-hidden="true" className="cover-chip__icon" />
      ) : tone !== "neutral" ? (
        <span aria-hidden="true" className="cover-chip__dot" />
      ) : null}
      {children == null ? null : (
        <span className="cover-chip__label">{children}</span>
      )}
    </span>
  );
}

/** Sentence case for a raw state token: `"ambiguous"` → `"Ambiguous"`,
 *  `"on_hold"` → `"On hold"`. A value that already carries capitals
 *  ("Ended", "TBA") is someone's wording and is left as it is. */
export function coverChipLabel(raw: string): string {
  const words = raw.replace(/[_-]+/g, " ").trim();
  if (!words) return raw;
  if (words !== words.toLowerCase()) return words;
  return words[0]!.toUpperCase() + words.slice(1);
}
