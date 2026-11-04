"use client";

import { useEffect, useRef, useState } from "react";

import {
  combineSpreadEdges,
  glowBackground,
  sampleEdgeColors,
  unionBox,
  type EdgeColors,
} from "@/lib/reader/ambient-glow";
import { readerPageImages } from "@/lib/reader/page-dom";
import { useReducedMotion } from "@/lib/visual-effects";

/** Strength of the glow layer. Kept low: it tints the empty reader
 *  surface around the page, it should never compete with the art. */
const GLOW_OPACITY = 0.6;

/**
 * Ambient page glow (effect `ambient_glow`). A fixed layer behind the
 * page content — painted on top of the reader surface (`bg-reader-bg`),
 * under the page images, which are opaque — so only the empty space
 * around the page is tinted. Two stacked layers cross-fade (~500ms) when
 * the page changes. Paged views only; the Reader doesn't mount it in
 * webtoon mode, and doesn't mount it at all when the effect is off.
 *
 * `urls` are the visible pages' thumbnail URLs in visual (left-to-right)
 * order; `preloadUrls` are sampled at idle time so the next turn's colours
 * are ready.
 */
export function AmbientGlow({
  urls,
  preloadUrls,
}: {
  urls: readonly string[];
  preloadUrls?: readonly string[];
}) {
  const [layers, setLayers] = useState<{ bg: [string, string]; on: 0 | 1 }>({
    bg: ["", ""],
    on: 0,
  });
  const edgesRef = useRef<EdgeColors | null>(null);
  // The account's Motion setting as well as the OS preference: under
  // reduced motion the colours swap without the cross-fade.
  const reduced = useReducedMotion();
  const key = urls.join("|");

  useEffect(() => {
    let alive = true;
    const list = key ? key.split("|") : [];
    void Promise.all(list.map(sampleEdgeColors)).then((all) => {
      if (!alive) return;
      const edges = all.every((e) => e !== null)
        ? combineSpreadEdges(all as EdgeColors[])
        : null;
      edgesRef.current = edges;
      const bg = edges ? background(edges) : "";
      setLayers((s) => {
        const next = (s.on ^ 1) as 0 | 1;
        const pair: [string, string] = [...s.bg];
        pair[next] = bg;
        return { bg: pair, on: next };
      });
    });
    return () => {
      alive = false;
    };
  }, [key]);

  // Keep the gradients anchored to the page when the window resizes or
  // the page image settles its size (no cross-fade: same colours).
  useEffect(() => {
    let raf = 0;
    const refresh = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const edges = edgesRef.current;
        if (!edges) return;
        const bg = background(edges);
        setLayers((s) => {
          if (s.bg[s.on] === bg) return s;
          const pair: [string, string] = [...s.bg];
          pair[s.on] = bg;
          return { ...s, bg: pair };
        });
      });
    };
    const ro =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(refresh);
    for (const img of readerPageImages()) ro?.observe(img);
    window.addEventListener("resize", refresh);
    return () => {
      cancelAnimationFrame(raf);
      ro?.disconnect();
      window.removeEventListener("resize", refresh);
    };
  }, [key]);

  // Warm the next page's colours while the reader is idle.
  const preloadKey = preloadUrls?.join("|") ?? "";
  useEffect(() => {
    if (!preloadKey) return;
    const run = () => preloadKey.split("|").forEach(sampleEdgeColors);
    if (typeof window.requestIdleCallback === "function") {
      const id = window.requestIdleCallback(run, { timeout: 2000 });
      return () => window.cancelIdleCallback(id);
    }
    const t = setTimeout(run, 300);
    return () => clearTimeout(t);
  }, [preloadKey]);

  return (
    <div
      aria-hidden="true"
      data-testid="ambient-glow"
      className="pointer-events-none fixed inset-0"
    >
      {layers.bg.map((bg, i) => (
        <div
          key={i}
          className="absolute inset-0"
          style={{
            transition: reduced ? "none" : "opacity 500ms ease-out",
            background: bg || undefined,
            opacity: layers.on === i && bg ? GLOW_OPACITY : 0,
          }}
        />
      ))}
    </div>
  );
}

function background(edges: EdgeColors): string {
  const box = unionBox(
    readerPageImages().map((img) => {
      const r = img.getBoundingClientRect();
      return { left: r.left, top: r.top, width: r.width, height: r.height };
    }),
  );
  if (box) return glowBackground(edges, box);
  // Page not measurable yet: centre a portrait page-sized box.
  const h = window.innerHeight;
  const w = Math.min(window.innerWidth, h * 0.66);
  return glowBackground(edges, {
    left: (window.innerWidth - w) / 2,
    top: 0,
    width: w,
    height: h,
  });
}
