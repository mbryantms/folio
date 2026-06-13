"use client";

import * as React from "react";
import { ListChecks, X } from "lucide-react";

import { Button } from "@/components/ui/button";

/**
 * The page-header entry point into (and out of) multi-select mode,
 * shared by every selectable list surface (library grid, search,
 * bookmarks, saved views, CBL).
 *
 * Earlier this button simply hid itself (`invisible opacity-0`) once
 * select mode was active, leaving a dead gap in the header and forcing
 * the user to find the toolbar's "Done". Instead it now toggles in
 * place: **Select** to enter, **Cancel** to exit. Keeping it mounted
 * also means the focus-restore-on-exit pattern has a stable target.
 */
export function SelectModeButton({
  active,
  onEnter,
  onExit,
  className,
  ref,
}: {
  /** Whether select mode is currently active. */
  active: boolean;
  onEnter: () => void;
  onExit: () => void;
  className?: string;
  ref?: React.Ref<HTMLButtonElement>;
}) {
  return (
    <Button
      ref={ref}
      type="button"
      variant="outline"
      size="sm"
      aria-pressed={active}
      onClick={active ? onExit : onEnter}
      className={className}
    >
      {active ? (
        <>
          <X className="mr-1.5 h-4 w-4" />
          Cancel
        </>
      ) : (
        <>
          <ListChecks className="mr-1.5 h-4 w-4" />
          Select
        </>
      )}
    </Button>
  );
}
