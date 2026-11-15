"use client";

import * as React from "react";

import { localToday } from "@/lib/upcoming/stage";

const noopSubscribe = () => () => {};

/** The viewer's local date (`YYYY-MM-DD`), or `null` on the server and
 *  during hydration: "this week" turns over at the viewer's midnight,
 *  which the server cannot know, so nothing date-dependent renders (and
 *  no upcoming query starts) before then. */
export function useLocalToday(): string | null {
  return React.useSyncExternalStore(
    noopSubscribe,
    () => localToday(),
    () => null,
  );
}
