"use client";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import type { ReadingStatsRange } from "@/lib/api/types";

const OPTIONS: ReadonlyArray<{ value: ReadingStatsRange; label: string }> = [
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
    />
  );
}
