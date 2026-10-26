"use client";

import * as React from "react";

import {
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu";
import { useTouchDevice } from "@/lib/ui/use-coarse-pointer";

/**
 * A dropdown submenu that adapts to the device.
 *
 * Radix submenus always open to the *side* of their parent. On a phone a
 * full-width-ish menu has no horizontal room beside it, so a side-anchored
 * submenu (e.g. "Fetch metadata", "Thumbnails") opens off-screen no matter
 * how much collision padding it gets.
 *
 * So:
 *  - **hover / pointer devices** → a normal nested side submenu (compact),
 *  - **touch / phone** (`useTouchDevice`) → the items are *flattened* inline
 *    under a section label, inside the main menu (which already scrolls).
 *    One vertical list, always reachable, one fewer tap.
 *
 * `children` are plain `DropdownMenuItem`s — they render identically whether
 * they sit inside the `SubContent` or inline, so callers pass the same set.
 */
export function DropdownMenuResponsiveSub({
  icon,
  label,
  children,
}: {
  /** Leading icon shown in the side-submenu trigger (omitted in the flat
   *  section label, which matches the menu's other uppercase headings). */
  icon?: React.ReactNode;
  label: React.ReactNode;
  children: React.ReactNode;
}) {
  const touch = useTouchDevice();

  if (touch) {
    return (
      <>
        <DropdownMenuLabel>{label}</DropdownMenuLabel>
        {children}
      </>
    );
  }

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        {icon}
        {label}
      </DropdownMenuSubTrigger>
      <DropdownMenuPortal>
        <DropdownMenuSubContent>{children}</DropdownMenuSubContent>
      </DropdownMenuPortal>
    </DropdownMenuSub>
  );
}
