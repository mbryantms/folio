"use client";

import { BookCheck, Library } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { useCblList } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

/**
 * Two pills that describe a CBL list at a glance:
 *
 *   - **Collection** (`Library` icon, muted outline): `matched / total`
 *     — how many of the list's books the library has on disk.
 *   - **Read progress** (`BookCheck` icon, primary accent): `read / matched`
 *     — how many of the matched issues the **calling user** has finished.
 *
 *  Different icons + a clear color split make them unambiguous: the
 *  collection pill is a neutral library statistic; the read-progress
 *  pill is your personal progress in the accent color. Each pill carries
 *  a `title` tooltip with the long-form description for hover clarity.
 *
 *  The two `size` modes match the surfaces this component lives on:
 *
 *   - `rail` (default) — `text-xs`, low-pad chips. Sits inside a
 *     rail-header row alongside the rail title.
 *   - `header` — `text-sm`, taller pad so the pills line up with the
 *     `size="sm"` buttons (`Edit` / `Pin` / `Refresh`) on a view page
 *     header.
 *
 *  Renders nothing when the list isn't loaded yet, and suppresses the
 *  read-progress pill specifically when `matched === 0` (no matched
 *  entries means the `read / 0` chip carries no information).
 */
export function CblStatsPills({
  cblListId,
  size = "rail",
  className,
}: {
  cblListId: string;
  size?: "rail" | "header";
  className?: string;
}) {
  const list = useCblList(cblListId);
  if (!list.data) return null;
  const { matched, total, read_count } = list.data.stats;
  const sizeClass =
    size === "header"
      ? "text-sm px-2.5 py-1 [&_svg]:h-3.5 [&_svg]:w-3.5"
      : "text-xs";
  return (
    <div className={cn("flex shrink-0 items-center gap-2", className)}>
      <Badge
        variant="outline"
        title={`${matched} of ${total} entries collected in your library`}
        className={cn("text-muted-foreground shrink-0", sizeClass)}
      >
        <Library aria-hidden="true" className="mr-1 h-3 w-3" />
        {matched} / {total}
      </Badge>
      {matched > 0 && (
        <Badge
          variant="outline"
          title={`You've read ${read_count} of ${matched} matched issues`}
          className={cn(
            "border-primary/40 bg-primary/10 text-primary shrink-0",
            sizeClass,
          )}
        >
          <BookCheck aria-hidden="true" className="mr-1 h-3 w-3" />
          {read_count} / {matched}
        </Badge>
      )}
    </div>
  );
}
