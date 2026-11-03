"use client";

import * as React from "react";
import type { LucideIcon } from "lucide-react";

import { Cover } from "@/components/Cover";
import { useMe } from "@/lib/api/queries";
import {
  BACK_COVERS,
  DEFAULT_COVER_STACK_STYLE,
  STACK_ASPECT,
  isCoverStackStyle,
  layoutFor,
  type CoverStackStyle,
} from "@/lib/cover-stack/variants";
import { cn } from "@/lib/utils";

/** The signed-in user's cover-stack style (`users.cover_stack_style`),
 *  `pickup` until `/auth/me` lands or for an unknown value. */
export function useCoverStackStyle(): CoverStackStyle {
  const me = useMe();
  const v = me.data?.cover_stack_style;
  return isCoverStackStyle(v) ? v : DEFAULT_COVER_STACK_STYLE;
}

/**
 * A fanned stack of up to four covers. Purely presentational: it fans out
 * when an ancestor marked `cover-stack-host` is hovered or holds keyboard
 * focus (see `styles/globals.css`), so the whole card is the hover target
 * and the stack itself never takes focus.
 *
 * Fewer covers than slots draws only what exists; no covers draws a
 * three-deep placeholder stack with `Icon` on the front tile, so an empty
 * list still reads as a list.
 */
export function CoverStack({
  urls,
  style,
  Icon,
  className,
}: {
  urls: readonly string[];
  /** Override the user's preference (Settings previews). */
  style?: CoverStackStyle;
  Icon?: LucideIcon;
  className?: string;
}) {
  const userStyle = useCoverStackStyle();
  const layout = layoutFor(style ?? userStyle);
  const covers = urls.slice(0, BACK_COVERS + 1);
  const empty = covers.length === 0;
  const count = empty ? 3 : covers.length;
  const layers = layout.layers.filter((l) => l.depth < count);

  return (
    <div
      aria-hidden="true"
      className={cn("cover-stack", className)}
      style={{ aspectRatio: `1 / ${STACK_ASPECT}` }}
    >
      {layers.map((layer) => {
        const src = covers[layer.depth];
        return (
          <div
            key={layer.depth}
            className="cover-stack__layer"
            data-front={layer.depth === 0 ? "" : undefined}
            style={
              {
                left: `${layout.leftPct}%`,
                top: `${layout.topPct}%`,
                width: `${layout.coverWidthPct}%`,
                "--cs-rest": layer.rest,
                "--cs-fan": layer.fan,
              } as React.CSSProperties
            }
          >
            {src ? (
              <Cover src={src} alt="" className="w-full" />
            ) : (
              <div className="bg-muted border-border flex aspect-[2/3] w-full items-center justify-center rounded-md border">
                {layer.depth === 0 && Icon ? (
                  <Icon className="text-muted-foreground/70 size-6" />
                ) : null}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
