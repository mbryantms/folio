import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Small static illustration for empty, 404 and error states: three comic
 * covers in silhouette, fanned, drawn with the same masthead / issue-number
 * / spine marks as the loading skeletons (`.cover-skeleton`) but with no
 * shimmer, so "nothing here" reads as an empty shelf of the same comics
 * the app shows while loading. An optional badge (the state's icon) sits on
 * the front cover's corner. Styles: `.cover-silhouettes` in
 * styles/flair-loading.css. Purely decorative (`aria-hidden`).
 */
export function CoverSilhouettes({
  icon: Icon,
  badge,
  size = "md",
  className,
}: {
  /** Icon component drawn in the corner badge. */
  icon?: React.ComponentType<{ className?: string }>;
  /** Or a ready-made badge node (wins over `icon`). */
  badge?: React.ReactNode;
  size?: "md" | "lg";
  className?: string;
}) {
  const corner = badge ?? (Icon ? <Icon className="size-4" /> : null);
  return (
    <div
      aria-hidden
      data-size={size}
      className={cn("cover-silhouettes", className)}
    >
      {[0, 1, 2].map((i) => (
        <span key={i} className="cover-skeleton cover-silhouette">
          <span className="cover-skeleton__masthead" />
          <span className="cover-skeleton__number" />
          <span className="cover-skeleton__spine" />
        </span>
      ))}
      {corner ? (
        <span
          className={cn(
            "cover-silhouettes__badge",
            badge ? null : "cover-silhouettes__badge--icon",
          )}
        >
          {corner}
        </span>
      ) : null}
    </div>
  );
}
