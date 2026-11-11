import { Check } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * The "I've read this" check on a cover's top-right corner (see
 * `docs/dev/card-corner-conventions.md`). One component so issue, CBL and
 * series cards draw the same mark. `role="img"` + `aria-label` puts the
 * meaning into the card link's accessible name; the tooltip repeats it.
 */
export function ReadCheck({
  label = "Read",
  className,
}: {
  label?: string;
  className?: string;
}) {
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={cn(
        "bg-primary/90 text-primary-foreground inline-flex size-6 shrink-0 items-center justify-center rounded-full shadow-sm ring-1 ring-black/10 backdrop-blur dark:ring-white/10",
        className,
      )}
    >
      <Check aria-hidden="true" className="size-3.5" />
    </span>
  );
}
