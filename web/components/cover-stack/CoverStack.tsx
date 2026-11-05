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

/** How much a spilling stack grows past its card while fanned. */
export const SPILL_SCALE = 1.18;

/**
 * Which way a spilling stack may grow without crossing `bounds` (the
 * content area beside the sidebar): from its centre when there is room on
 * both sides, otherwise pinned to the edge that would be clipped.
 */
export function spillOrigin(
  stack: { left: number; right: number },
  bounds: { left: number; right: number },
  scale = SPILL_SCALE,
  margin = 8,
): "center" | "left" | "right" {
  const grow = ((stack.right - stack.left) * (scale - 1)) / 2;
  if (stack.left - grow < bounds.left + margin) return "left";
  if (stack.right + grow > bounds.right - margin) return "right";
  return "center";
}

/**
 * A fanned stack of up to four covers. Purely presentational: it fans out
 * when an ancestor marked `cover-stack-host` is hovered or holds keyboard
 * focus (see `styles/globals.css`), so the whole card is the hover target
 * and the stack itself never takes focus.
 *
 * With `spill` (the default) the fanned stack also grows past the card
 * ("spill cover fan", folio-flair-1.0). Just before it does, it measures
 * itself against the content area so a card in the first or last column
 * grows inward instead of sliding under the sidebar or off screen.
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
  spill = true,
  spillScale = SPILL_SCALE,
}: {
  urls: readonly string[];
  /** Override the user's preference (Settings previews). */
  style?: CoverStackStyle;
  Icon?: LucideIcon;
  className?: string;
  /** Grow past the card while fanned. Off for fixed-size previews. */
  spill?: boolean;
  /** How much the stack grows while spilling (large stacks need less). */
  spillScale?: number;
}) {
  const userStyle = useCoverStackStyle();
  const layout = layoutFor(style ?? userStyle);
  const covers = urls.slice(0, BACK_COVERS + 1);
  const empty = covers.length === 0;
  const count = empty ? 3 : covers.length;
  const layers = layout.layers.filter((l) => l.depth < count);
  const ref = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    const el = ref.current;
    const host = el?.closest<HTMLElement>(".cover-stack-host");
    if (!spill || !el || !host) return;
    const measure = () => {
      // Layout box, not the (possibly already scaled) painted one.
      const parent = (el.offsetParent as HTMLElement | null) ?? host;
      const left = parent.getBoundingClientRect().left + el.offsetLeft;
      const area = el.closest("main")?.getBoundingClientRect();
      el.dataset.spill = spillOrigin(
        { left, right: left + el.offsetWidth },
        {
          left: Math.max(0, area?.left ?? 0),
          right: Math.min(window.innerWidth, area?.right ?? window.innerWidth),
        },
        spillScale,
      );
    };
    host.addEventListener("pointerenter", measure);
    host.addEventListener("focusin", measure);
    return () => {
      host.removeEventListener("pointerenter", measure);
      host.removeEventListener("focusin", measure);
    };
  }, [spill, spillScale]);

  return (
    <div
      ref={ref}
      aria-hidden="true"
      className={cn("cover-stack", spill && "cover-stack--spill", className)}
      data-spill="center"
      style={
        {
          aspectRatio: `1 / ${STACK_ASPECT}`,
          "--cs-spill": spillScale,
        } as React.CSSProperties
      }
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
