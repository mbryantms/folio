"use client";

import * as React from "react";
import Link from "next/link";

import { Cover } from "@/components/Cover";
import type { StartHereItemView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { coverThumbUrl } from "@/lib/urls";

/**
 * Reading-list card for the Start Here rail + page (Start Here M2).
 *
 * The first matched cover sits in front with up to two more fanned behind
 * it, so a card reads as "a list of books" next to the single-cover series
 * and issue cards on the other rails. Below: name, a two-line blurb (the
 * admin-written view description, falling back to the CBL's own) and the
 * caller's progress.
 */
export function StartHereCard({
  item,
  className,
  children,
}: {
  item: StartHereItemView;
  className?: string;
  /** Extra controls under the meta line (admin reorder on the page). */
  children?: React.ReactNode;
}) {
  const [front, ...behind] = item.cover_issue_ids;
  const pct =
    item.available > 0
      ? Math.round((item.read_count / item.available) * 100)
      : 0;
  const started = item.read_count > 0;
  const meta = started
    ? `${item.read_count} of ${item.available} read`
    : item.available === item.total
      ? `${item.total} ${item.total === 1 ? "issue" : "issues"}`
      : `${item.available} of ${item.total} issues in the library`;
  return (
    <div className={cn("group flex min-w-0 flex-col gap-2", className)}>
      <Link
        href={`/views/${item.view_id}`}
        className="focus-visible:ring-ring flex min-w-0 flex-col gap-2 rounded-md focus-visible:ring-2 focus-visible:outline-none"
      >
        {/* Stacked covers: back covers peek out to the right, each a step
         *  smaller. Scaling from the left edge's midpoint keeps them inside
         *  the front cover's height, and the right padding reserves room
         *  for the peek, so the stack never spills onto the text below. */}
        <div className="relative pr-[14%]">
          {behind
            .slice(0, 2)
            .reverse()
            .map((id, i, arr) => {
              // i = 0 is the rear-most cover.
              const depth = arr.length - i;
              return (
                <div
                  key={id}
                  aria-hidden
                  className="absolute inset-y-0 left-0 w-[86%] origin-left"
                  style={{
                    transform: `translateX(${depth * 9}%) scale(${1 - depth * 0.05})`,
                  }}
                >
                  <Cover
                    src={coverThumbUrl(id)}
                    alt=""
                    className="h-full w-full opacity-70 shadow-sm"
                  />
                </div>
              );
            })}
          <div className="relative">
            <Cover
              src={front ? coverThumbUrl(front) : null}
              alt={item.name}
              fallback={item.name}
              className="w-full shadow-md transition group-hover:brightness-110"
            />
            {started ? (
              <div
                className="bg-background/70 absolute inset-x-0 bottom-0 h-1 overflow-hidden rounded-b-md"
                aria-hidden
              >
                <div
                  className="bg-primary h-full"
                  style={{ width: `${pct}%` }}
                />
              </div>
            ) : null}
          </div>
        </div>
        <div className="min-w-0 space-y-0.5">
          <div className="truncate text-sm font-medium" title={item.name}>
            {item.name}
          </div>
          {item.description ? (
            <p className="text-muted-foreground line-clamp-2 text-xs">
              {item.description}
            </p>
          ) : null}
          <div className="text-muted-foreground text-xs">{meta}</div>
        </div>
      </Link>
      {children}
    </div>
  );
}

export function StartHereCardSkeleton() {
  return (
    <div className="flex flex-col gap-2 pr-[14%]">
      <div className="bg-muted aspect-[2/3] animate-pulse rounded-md" />
      <div className="bg-muted h-3 w-3/4 animate-pulse rounded" />
      <div className="bg-muted h-3 w-1/2 animate-pulse rounded" />
    </div>
  );
}
