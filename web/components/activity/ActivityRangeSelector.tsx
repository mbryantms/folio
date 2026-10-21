"use client";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import type { ReadingStatsRange } from "@/lib/api/types";

const OPTIONS: ReadonlyArray<{ value: ReadingStatsRange; label: string }> = [
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "60d", label: "60 days" },
  { value: "90d", label: "90 days" },
  { value: "1y", label: "1 year" },
  { value: "all", label: "All time" },
];

export function ActivityRangeSelector({
  value,
  onChange,
}: {
  value: ReadingStatsRange;
  onChange: (next: ReadingStatsRange) => void;
}) {
  return (
    <SegmentedControl
      value={value}
      onChange={onChange}
      options={OPTIONS}
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
