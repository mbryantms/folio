import Link from "next/link";
import { ChevronRight } from "lucide-react";

import { cn } from "@/lib/utils";

/** One breadcrumb. `href` omitted ⇒ rendered as the current (non-link)
 *  crumb. When the page title (an `h1`) is the leaf, callers usually pass
 *  just the ancestor trail as links (e.g. `Admin › Libraries`). */
export type Crumb = { label: string; href?: string };

/** Shared breadcrumb trail (A2 wayfinding). Used by the admin `PageHeader`,
 *  the admin library-detail layout, and the library series-detail header so
 *  they all read alike. Theme tokens only — no hardcoded colors/borders. */
export function Breadcrumbs({
  items,
  className,
}: {
  items: Crumb[];
  className?: string;
}) {
  if (items.length === 0) return null;
  return (
    <nav aria-label="Breadcrumb" className={className}>
      <ol className="text-muted-foreground flex flex-wrap items-center gap-1 text-xs">
        {items.map((crumb, i) => (
          <li key={`${crumb.label}-${i}`} className="flex items-center gap-1">
            {i > 0 ? (
              <ChevronRight
                className="size-3 shrink-0 opacity-60"
                aria-hidden="true"
              />
            ) : null}
            {crumb.href ? (
              <Link
                href={crumb.href}
                className={cn("hover:text-foreground transition-colors")}
              >
                {crumb.label}
              </Link>
            ) : (
              <span className="text-foreground" aria-current="page">
                {crumb.label}
              </span>
            )}
          </li>
        ))}
      </ol>
    </nav>
  );
}
