"use client";

import { useEffect, useState } from "react";

import { useReducedMotion } from "@/lib/visual-effects";

export interface ListStats {
  total: number;
  matched: number;
  read_count?: number;
  ambiguous: number;
  missing: number;
}

export interface Segment {
  key: "read" | "unread" | "ambiguous" | "missing";
  label: string;
  count: number;
  className: string;
}

/** Split a reading list's counts into the bar's segments, in display
 *  order, dropping empty ones. Pure, for tests. */
export function listSegments(s: ListStats): Segment[] {
  const read = Math.min(s.read_count ?? 0, s.matched);
  const segs: Segment[] = [
    { key: "read", label: "read", count: read, className: "bg-primary" },
    {
      key: "unread",
      label: "to read",
      count: Math.max(0, s.matched - read),
      className: "bg-foreground/25",
    },
    {
      key: "ambiguous",
      label: "need a match",
      count: s.ambiguous,
      className: "bg-warning/60",
    },
    {
      key: "missing",
      label: "not in your library",
      count: s.missing,
      className:
        "bg-[repeating-linear-gradient(135deg,var(--muted-foreground)_0_2px,transparent_2px_5px)] opacity-50",
    },
  ];
  return segs.filter((x) => x.count > 0);
}

/**
 * Segmented list progress (folio-flair-1.0, standard): one bar per reading
 * list split into what you've read, what's left to read, entries that
 * still need a match and entries missing from the library, each sized by
 * count. The segments grow in from the left on first view. The list API
 * reports counts, not per-entry state, so segments are grouped, not in
 * reading order.
 */
export function ListProgressBar({
  stats,
  className,
}: {
  stats: ListStats;
  className?: string;
}) {
  const reduced = useReducedMotion();
  const [grown, setGrown] = useState(reduced);
  useEffect(() => {
    if (reduced) return;
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, [reduced]);

  const segs = listSegments(stats);
  const total = segs.reduce((n, x) => n + x.count, 0);
  if (total === 0) return null;
  const summary = segs.map((x) => `${x.count} ${x.label}`).join(", ");

  return (
    <div
      role="img"
      aria-label={summary}
      title={summary}
      className={`flex h-1.5 w-full gap-0.5 overflow-hidden rounded-full ${className ?? ""}`}
    >
      {segs.map((x, i) => (
        <span
          key={x.key}
          className={`h-full rounded-full ${x.className}`}
          style={{
            flexGrow: grown ? x.count : 0,
            flexBasis: 0,
            minWidth: grown ? 3 : 0,
            transition: reduced
              ? undefined
              : `flex-grow 700ms cubic-bezier(0.2, 0.8, 0.2, 1) ${i * 90}ms, min-width 700ms ${i * 90}ms`,
          }}
        />
      ))}
    </div>
  );
}
