import { CoverChip } from "@/components/library/CoverChip";
import { Badge } from "@/components/ui/badge";
import type { UpcomingEntryView } from "@/lib/api/types";
import { statusTone } from "@/lib/ui/status-tone";
import { stageBadge } from "@/lib/upcoming/stage";
import { cn } from "@/lib/utils";

/**
 * Where a release stands, as one badge. `onCover` is the variant that
 * sits on cover art: the standard `CoverChip` (a tone with its dot, or
 * the solid chip for a failed download) instead of the tinted surface,
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
  if (onCover) {
    return (
      <CoverChip tone={tone} data-stage={entry.stage} className={className}>
        {label}
      </CoverChip>
    );
  }
  return (
    <Badge
      variant="outline"
      data-stage={entry.stage}
      className={cn(statusTone(tone), className)}
    >
      {label}
    </Badge>
  );
}
