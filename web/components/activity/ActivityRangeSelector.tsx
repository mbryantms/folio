"use client";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import type { ReadingStatsRange } from "@/lib/api/types";
import { useIsPhone } from "@/lib/ui/use-is-phone";

/** The ranges the selector offers (also the valid `?range=` values). */
export const RANGE_OPTIONS: ReadonlyArray<{
  value: ReadingStatsRange;
  label: string;
  short: string;
}> = [
  { value: "7d", label: "7 days", short: "7d" },
  { value: "30d", label: "30 days", short: "30d" },
  { value: "60d", label: "60 days", short: "60d" },
  { value: "90d", label: "90 days", short: "90d" },
  { value: "1y", label: "1 year", short: "1y" },
  { value: "all", label: "All time", short: "All" },
];

/** `raw` if it's one of the selector's ranges, else `fallback`. */
export function parseRange(
  raw: string | null | undefined,
  fallback: ReadingStatsRange,
): ReadingStatsRange {
  return RANGE_OPTIONS.find((o) => o.value === raw)?.value ?? fallback;
}

const LONG = RANGE_OPTIONS.map(({ value, label }) => ({ value, label }));
// Phones: short labels sharing the row's width, so all six fit (the long
// labels scrolled "All time" off the edge).
const SHORT = RANGE_OPTIONS.map(({ value, label, short }) => ({
  value,
  label: short,
  title: label,
}));

export function ActivityRangeSelector({
  value,
  onChange,
}: {
  value: ReadingStatsRange;
  onChange: (next: ReadingStatsRange) => void;
}) {
  const phone = useIsPhone();
  return (
    <SegmentedControl
      value={value}
      onChange={onChange}
      options={phone ? SHORT : LONG}
      fill={phone}
      ariaLabel="Stats range"
      // Toolbar-row convention: h-9 (36px) so the group lines up with
      // the Add widget / Export / Reset buttons in the /log header
      // (its natural height is 38px). The pills drop their vertical
      // padding and stretch to the group instead. `folio-control`
      // joins it to the 44px touch floor (phone-layout.md rule 9).
      className="folio-control h-9 [&>button]:py-0"
    />
  );
}
