import { Badge } from "@/components/ui/badge";
import type { UpcomingEntryView } from "@/lib/api/types";
import { statusTone, statusToneText } from "@/lib/ui/status-tone";
import { stageBadge } from "@/lib/upcoming/stage";
import { cn } from "@/lib/utils";

/**
 * Where a release stands, as one badge. `onCover` is the variant that
 * sits on cover art: an opaque backing instead of the tinted surface,
 * which is unreadable over a picture.
 *
 * The badge only reports. Folio has no way to act on Mylar.
 */
export function UpcomingStageBadge({
  entry,
  onCover = false,
  className,
}: {
  entry: UpcomingEntryView;
  onCover?: boolean;
  className?: string;
}) {
  const { label, tone } = stageBadge(entry);
  return (
    <Badge
      variant="outline"
      data-stage={entry.stage}
      className={cn(
        onCover
          ? cn("bg-background/85 backdrop-blur", statusToneText(tone))
          : statusTone(tone),
        className,
      )}
    >
      {label}
    </Badge>
  );
}
