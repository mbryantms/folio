"use client";

import { COVER_BOX } from "@/components/Cover";
import { ProviderCoverImage } from "@/components/library/ProviderCoverImage";
import type { UpcomingEntryView } from "@/lib/api/types";
import { useCoverTone } from "@/lib/ui/use-cover-tone";
import { cn } from "@/lib/utils";

/**
 * A release's cover, hotlinked from Metron's CDN, in the 2:3 box every
 * cover card uses (`COVER_BOX`, so the card's hover glow lands on it, and
 * `useCoverTone` for the cover's colour). A release with no cover (or one
 * whose image fails to load) shows its issue number on the placeholder:
 * pull-list-only rows never have one, and Metron adds art to an
 * announcement late.
 *
 * The image is decorative (`alt=""`): the text beside it names the
 * release.
 */
export function UpcomingCover({
  entry,
  className,
}: {
  entry: UpcomingEntryView;
  className?: string;
}) {
  const { boxRef } = useCoverTone(entry.cover_url);
  return (
    <div ref={boxRef} className={cn(COVER_BOX, "relative text-sm", className)}>
      <span
        aria-hidden
        className="text-muted-foreground absolute inset-0 flex items-center justify-center px-1 text-center font-medium"
      >
        #{entry.number}
      </span>
      <ProviderCoverImage
        src={entry.cover_url}
        alt=""
        className="relative h-full w-full object-cover"
        placeholderClassName="hidden"
      />
    </div>
  );
}
