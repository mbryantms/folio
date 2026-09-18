"use client";
import { useEffect } from "react";

/** Keyboard clearance is independent of the hardware safe area. Ignore
 * pinch zoom so accessibility magnification does not move sheets. */
export function VisualViewportSync() {
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const root = document.documentElement;
    const update = () => {
      if (viewport.scale !== 1) return;
      const inset = Math.max(
        0,
        window.innerHeight - viewport.height - viewport.offsetTop,
      );
      root.style.setProperty("--keyboard-inset", `${inset}px`);
      root.style.setProperty("--visual-height", `${viewport.height}px`);
    };
    update();
    viewport.addEventListener("resize", update);
    viewport.addEventListener("scroll", update);
    return () => {
      viewport.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
      root.style.removeProperty("--keyboard-inset");
      root.style.removeProperty("--visual-height");
    };
  }, []);
  return null;
}
