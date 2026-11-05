import { ChevronRight } from "lucide-react";

import { Kbd } from "@/components/folio/kbd";
import type { SearchAction } from "@/lib/search/actions-registry";

/**
 * The inside of one palette action row, shared by the ⌘K dialog
 * (`CommandItem`) and the desktop topbar dropdown (`ResultRow`): icon
 * tile, label, a subtitle (the group, or the issue a continue-reading
 * row resumes), and on the right the action's shortcut as a keycap, or a
 * chevron for a plain destination.
 */
export function ActionRowContent({
  action,
  detail,
  keyHint,
}: {
  action: SearchAction;
  detail: string;
  keyHint: string | null;
}) {
  const Icon = action.icon;
  return (
    <>
      <span
        aria-hidden="true"
        className="border-border bg-muted text-muted-foreground inline-flex h-9 w-9 shrink-0 items-center justify-center rounded border"
      >
        <Icon className="size-4" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate font-medium">{action.label}</span>
        <span className="block truncate text-xs opacity-70">{detail}</span>
      </span>
      {keyHint ? (
        // The chord is decorative here (the row runs it); the shortcuts
        // sheet is where it is announced.
        <Kbd aria-hidden="true" className="h-5 shrink-0 px-1.5">
          {keyHint}
        </Kbd>
      ) : action.href ? (
        <ChevronRight className="size-3.5 opacity-50" aria-hidden="true" />
      ) : null}
    </>
  );
}
