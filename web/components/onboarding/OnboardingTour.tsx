"use client";

import * as React from "react";
import dynamic from "next/dynamic";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

import { useUpdatePreferences } from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";

// The overlay (and the starter-list shelf) only loads when a tour runs.
const TourOverlay = dynamic(
  () => import("./TourOverlay").then((m) => m.TourOverlay),
  { ssr: false },
);

/**
 * Runs the first-run tour on Home: automatically for an account that has
 * never finished or skipped it (`onboarding_completed_at` is null), and on
 * demand at `/?tour=1` (the user menu's "Take the tour"). Finishing or
 * skipping records completion, so it never auto-starts again.
 *
 * Mounted once in the library shell; render inside `<Suspense>` (it reads
 * search params).
 */
export function OnboardingTour() {
  const me = useMe();
  const pathname = usePathname();
  const params = useSearchParams();
  const router = useRouter();
  const update = useUpdatePreferences({ silent: true });
  const [closed, setClosed] = React.useState(false);

  const replay = params.get("tour") === "1";
  const onHome = pathname === "/" && !params.has("library");
  const firstRun = !!me.data && me.data.onboarding_completed_at == null;

  // A replay link reopens the tour even after it closed this session.
  const [lastReplay, setLastReplay] = React.useState(replay);
  if (replay !== lastReplay) {
    setLastReplay(replay);
    if (replay) setClosed(false);
  }

  if (!onHome || closed || !(firstRun || replay)) return null;

  function finish() {
    setClosed(true);
    if (firstRun) update.mutate({ onboarding_completed: true });
    if (replay) router.replace("/", { scroll: false });
  }

  return <TourOverlay onFinish={finish} />;
}
