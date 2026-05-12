import Link from "next/link";

import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

/**
 * Horizontal rail with a header + optional "see all" link. Children are
 * laid out in a flex row that scrolls horizontally on overflow. Used by the
 * home page rails ("Recently Updated", "Recently Added") and reusable
 * elsewhere if we add more discovery rows.
 */
export function Rail({
  title,
  description,
  href,
  hrefLabel = "See all",
  children,
  className,
}: {
  title: string;
  description?: string;
  href?: string;
  hrefLabel?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("space-y-3", className)}>
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
          {description && (
            <p className="text-muted-foreground text-sm">{description}</p>
          )}
        </div>
        {href && (
          <Link
            href={href}
            className="text-muted-foreground hover:text-foreground text-sm font-medium underline-offset-4 hover:underline"
          >
            {hrefLabel}
          </Link>
        )}
      </div>
      <ScrollArea className="-mx-1 w-[calc(100%+0.5rem)] pb-3">
        <div className="flex gap-3 px-1">{children}</div>
        <ScrollBar orientation="horizontal" />
      </ScrollArea>
    </section>
  );
}
