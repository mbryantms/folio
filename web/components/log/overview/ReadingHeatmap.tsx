"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import {
  dayLabel,
  heatmapGrid,
  type DayPages,
  type HeatCell,
  type Run,
} from "@/lib/log/overview";
import { cn } from "@/lib/utils";

/** Fill per heat level: steps of the accent mixed into the muted surface. */
const LEVEL_BG = [
  "bg-muted",
  "bg-[color-mix(in_oklab,var(--primary)_28%,var(--muted))]",
  "bg-[color-mix(in_oklab,var(--primary)_50%,var(--muted))]",
  "bg-[color-mix(in_oklab,var(--primary)_75%,var(--muted))]",
  "bg-primary",
] as const;

const WEEKDAYS = ["Mon", "", "Wed", "", "Fri", "", ""];

/**
 * Activity heatmap for the reading log's Overview (folio-flair phase 4):
 * the last 53 weeks, Monday-first, five levels by pages read, the current
 * streak outlined. Hover (or tap) a day for its pages; on narrow screens
 * it scrolls, starting at today.
 */
export function ReadingHeatmap({
  days,
  today,
  current,
  issuesByDate,
}: {
  days: readonly DayPages[];
  today: string;
  current: Run | null;
  /** Issues finished per day, for the tooltip (optional). */
  issuesByDate?: ReadonlyMap<string, number>;
}) {
  const { columns, months } = useMemo(
    () => heatmapGrid(days, today, current),
    [days, today, current],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [tip, setTip] = useState<{
    cell: HeatCell;
    x: number;
    y: number;
  } | null>(null);

  // Start scrolled to today (the right end) where the grid overflows.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [columns]);

  const readingDays = days.filter(
    (d) => d.date > columns[0]![0]!.date && d.pages > 0,
  ).length;
  const pagesInView = days
    .filter((d) => d.date >= columns[0]![0]!.date)
    .reduce((s, d) => s + d.pages, 0);

  const show = (cell: HeatCell, el: HTMLElement) => {
    const wrap = wrapRef.current;
    if (!wrap || cell.future) return;
    const r = el.getBoundingClientRect();
    const w = wrap.getBoundingClientRect();
    setTip({ cell, x: r.left + r.width / 2 - w.left, y: r.top - w.top });
  };

  const cells = useMemo(() => columns.flat(), [columns]);
  const gridRef = useRef<HTMLDivElement>(null);
  const cellAt = (t: EventTarget | null): HTMLElement | null =>
    t instanceof HTMLElement ? t.closest<HTMLElement>("[data-date]") : null;
  const cellFor = (el: HTMLElement): HeatCell =>
    cells.find((c) => c.date === el.dataset.date) ?? cells[0]!;
  const moveTo = (i: number) => {
    const last = cells.findLastIndex((c) => !c.future);
    const idx = Math.max(0, Math.min(last, i));
    const c = cells[idx];
    const el = gridRef.current?.querySelector<HTMLElement>(
      `[data-date="${c?.date}"]`,
    );
    if (!c || !el) return;
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
    show(c, el);
  };

  const tipText = (c: HeatCell) => {
    const label = dayLabel(c.date, today.slice(0, 4));
    if (!c.pages) return `${label} · No reading`;
    const issues = issuesByDate?.get(c.date) ?? 0;
    return `${label} · ${c.pages.toLocaleString()} ${c.pages === 1 ? "page" : "pages"}${
      issues ? ` · ${issues} ${issues === 1 ? "issue" : "issues"}` : ""
    }`;
  };

  return (
    <div ref={wrapRef} className="[container-type:inline-size] relative">
      {/* Cells size to the card (container query), 10–24px, so the year
          fits a desktop card and scrolls on a phone. */}
      <div
        className="flex gap-2"
        style={
          {
            "--hm-cell": "clamp(10px, calc((100cqi - 3rem) / 53 - 3px), 24px)",
          } as React.CSSProperties
        }
      >
        <div
          aria-hidden
          className="text-muted-foreground grid shrink-0 pt-5 text-xs"
          style={{ gridTemplateRows: "repeat(7, var(--hm-cell))", rowGap: 3 }}
        >
          {WEEKDAYS.map((d, i) => (
            <span key={i} className="leading-[var(--hm-cell)]">
              {d}
            </span>
          ))}
        </div>
        <div
          ref={scrollRef}
          className="min-w-0 overflow-x-auto pb-1"
          onScroll={() => {
            if (document.activeElement !== gridRef.current) setTip(null);
          }}
        >
          <div className="w-max">
            <div
              aria-hidden
              className="text-muted-foreground relative h-5 text-xs"
            >
              {months.map((m) => (
                <span
                  key={`${m.column}-${m.label}`}
                  className="absolute top-0"
                  style={{
                    left: `calc(${m.column} * (var(--hm-cell) + 3px))`,
                  }}
                >
                  {m.label}
                </span>
              ))}
            </div>
            {/* One focus stop for the year: arrow keys walk the days (up/down
                a day, left/right a week), the tooltip reads each one out.
                Pointer: hover on a mouse, tap elsewhere. */}
            {/* eslint-disable jsx-a11y/no-noninteractive-element-interactions, jsx-a11y/no-noninteractive-tabindex -- one keyboard stop for the whole grid (arrow keys), see above */}
            <div
              ref={gridRef}
              role="application"
              tabIndex={0}
              aria-roledescription="calendar heatmap"
              aria-label={`Reading heatmap: ${readingDays} reading days and ${pagesInView.toLocaleString()} pages over the last year. Use the arrow keys to read each day.`}
              className="focus-visible:ring-ring grid grid-flow-col gap-[3px] rounded-sm focus-visible:ring-2 focus-visible:ring-offset-4 focus-visible:ring-offset-[var(--card)] focus-visible:outline-none"
              style={{
                gridTemplateRows: "repeat(7, var(--hm-cell))",
                gridAutoColumns: "var(--hm-cell)",
              }}
              onPointerOver={(e) => {
                if (e.pointerType !== "mouse") return;
                const el = cellAt(e.target);
                if (el) show(cellFor(el), el);
              }}
              onPointerLeave={(e) => {
                if (e.pointerType === "mouse") setTip(null);
              }}
              onClick={(e) => {
                const el = cellAt(e.target);
                if (!el) return;
                const c = cellFor(el);
                if (tip?.cell.date === c.date) setTip(null);
                else show(c, el);
              }}
              onFocus={() => {
                if (!tip) moveTo(cells.findLastIndex((c) => !c.future));
              }}
              onBlur={() => setTip(null)}
              onKeyDown={(e) => {
                const at = tip
                  ? cells.findIndex((c) => c.date === tip.cell.date)
                  : cells.findLastIndex((c) => !c.future);
                const step =
                  e.key === "ArrowUp"
                    ? -1
                    : e.key === "ArrowDown"
                      ? 1
                      : e.key === "ArrowLeft"
                        ? -7
                        : e.key === "ArrowRight"
                          ? 7
                          : e.key === "Home"
                            ? -at
                            : e.key === "End"
                              ? cells.findLastIndex((c) => !c.future) - at
                              : null;
                if (e.key === "Escape") {
                  setTip(null);
                  return;
                }
                if (step == null) return;
                e.preventDefault();
                moveTo(at + step);
              }}
            >
              {/* eslint-enable jsx-a11y/no-noninteractive-element-interactions, jsx-a11y/no-noninteractive-tabindex */}
              {cells.map((c) => (
                <span
                  key={c.date}
                  data-date={c.date}
                  className={cn(
                    "rounded-[3px] transition-[box-shadow]",
                    c.future ? "bg-transparent" : LEVEL_BG[c.level],
                    c.streak &&
                      "ring-foreground/70 ring-1 ring-offset-1 ring-offset-[var(--card)]",
                    tip?.cell.date === c.date && "ring-foreground ring-2",
                  )}
                />
              ))}
            </div>
          </div>
        </div>
      </div>
      <div className="text-muted-foreground mt-3 flex flex-wrap items-center justify-between gap-3 text-xs">
        <span className="inline-flex items-center gap-2">
          {current ? (
            <>
              <span className="ring-foreground/70 ring-offset-card bg-primary size-2.5 rounded-[3px] ring-1 ring-offset-1" />
              Current streak: {current.length}{" "}
              {current.length === 1 ? "day" : "days"}
            </>
          ) : (
            "No streak right now — read today to start one"
          )}
        </span>
        <span className="inline-flex items-center gap-1.5" aria-hidden>
          Less
          {LEVEL_BG.map((bg, i) => (
            <span key={i} className={cn("size-2.5 rounded-[3px]", bg)} />
          ))}
          More
        </span>
      </div>
      {tip ? (
        <div
          role="status"
          className="bg-popover text-popover-foreground pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-full rounded-md border px-2 py-1 text-xs whitespace-nowrap shadow-md"
          style={{ left: tip.x, top: tip.y - 6 }}
        >
          {tipText(tip.cell)}
        </div>
      ) : null}
    </div>
  );
}
