import Link from "next/link";

import {
  WORDMARK_LINK_CLASS,
  WordmarkLockup,
} from "@/components/shell/WordmarkLockup";
import { cn } from "@/lib/utils";

/**
 * The Folio lockup as the title bar's home link (`MainShell`,
 * `AdminShell`). The drawing and its notes are in `WordmarkLockup.tsx`;
 * `StatusScreen` uses that file's `WordmarkAnchor`, which carries no
 * router code.
 *
 * The link's accessible name is "Folio"; the drawings are hidden from
 * assistive tech and there is no visible text, so label-in-name holds.
 */
export function Wordmark({
  href,
  className,
}: {
  href: string;
  className?: string;
}) {
  return (
    <Link
      href={href}
      aria-label="Folio"
      data-wordmark=""
      className={cn(WORDMARK_LINK_CLASS, className)}
    >
      <WordmarkLockup />
    </Link>
  );
}
