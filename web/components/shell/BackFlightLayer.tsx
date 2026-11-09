"use client";

import { usePathname } from "next/navigation";
import { useEffect } from "react";

import { landBackFlight, noteRoute } from "@/lib/ui/back-flight";

/** Mounted once, in the root layout, so it sees every route (the reader and
 *  the admin and settings shells included): records in-app history for
 *  `BackButton` and lands a pending Back flight when the previous page is
 *  back (lib/ui/back-flight.ts). Renders nothing. */
export function BackFlightLayer() {
  const pathname = usePathname() ?? "";
  useEffect(() => {
    noteRoute(pathname);
    landBackFlight(pathname);
  }, [pathname]);
  return null;
}
