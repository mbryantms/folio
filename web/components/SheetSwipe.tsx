"use client";

import { useEffect } from "react";

import { installSheetSwipe } from "@/lib/ui/sheet-swipe";

/** Mounts drag-down-to-dismiss for every bottom sheet (lib/ui/sheet-swipe). */
export function SheetSwipe() {
  useEffect(() => installSheetSwipe(), []);
  return null;
}
