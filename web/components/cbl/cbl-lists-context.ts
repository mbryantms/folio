"use client";

import * as React from "react";

import type { CblListView } from "@/lib/api/types";

/** CBL list rows keyed by list id, so each reading-list card or row can
 *  show stats without its own detail query. `null` until loaded. Filled
 *  by `ReadingListsBoard` from the one uncapped `/me/cbl-lists` query. */
export const CblListsContext = React.createContext<Map<
  string,
  CblListView
> | null>(null);
