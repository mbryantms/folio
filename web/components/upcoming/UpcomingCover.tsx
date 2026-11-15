import { ProviderCoverImage } from "@/components/library/ProviderCoverImage";
import type { UpcomingEntryView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * A release's cover, hotlinked from Metron's CDN, in the 2:3 box every
 * cover card uses. A release with no cover (or one whose image fails to
 * load) shows its issue number on the placeholder: pull-list-only rows
 * never have one, and Metron adds art to an announcement late.
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
  return (
    <div
      className={cn(
        "bg-muted border-border relative aspect-[2/3] overflow-hidden rounded-md border text-sm",
        className,
      )}
    >
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
