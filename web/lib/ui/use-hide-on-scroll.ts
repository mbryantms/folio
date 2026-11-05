"use client";

import { usePathname } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import {
  initialHideOnScrollState,
  nextHideOnScroll,
} from "@/lib/ui/hide-on-scroll";

/** Phones only: the bottom tab bar is `md:hidden`. */
const PHONE_QUERY = "(width < 48rem)";

/** Radix (react-remove-scroll) marks <body> while a modal dialog, sheet,
 *  dropdown or select holds the page's scroll lock. */
function overlayOpen(): boolean {
  return document.body.hasAttribute("data-scroll-locked");
}

/**
 * Whether the phone bottom tab bar should be slid away. Window scroll
 * drives `nextHideOnScroll`; the bar comes back on any route change, the
 * moment an overlay opens (`blocked` from the caller, or a Radix scroll
 * lock on <body>), and when `reveal()` is called (keyboard focus moving
 * into the bar).
 */
export function useHideOnScroll({ blocked = false }: { blocked?: boolean }) {
  const pathname = usePathname();
  const [hidden, setHidden] = useState(false);
  const state = useRef(initialHideOnScrollState());
  const blockedRef = useRef(blocked);

  useEffect(() => {
    blockedRef.current = blocked;
    if (blocked) {
      state.current = initialHideOnScrollState(window.scrollY);
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setHidden(false);
    }
  }, [blocked]);

  // A new page starts with the bar in view.
  useEffect(() => {
    state.current = initialHideOnScrollState(window.scrollY);
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setHidden(false);
  }, [pathname]);

  useEffect(() => {
    const mq = window.matchMedia(PHONE_QUERY);
    let frame = 0;
    const update = () => {
      frame = 0;
      const doc = document.documentElement;
      state.current = nextHideOnScroll(state.current, {
        y: window.scrollY,
        maxY: doc.scrollHeight - window.innerHeight,
        blocked: blockedRef.current || overlayOpen(),
      });
      setHidden(state.current.hidden);
    };
    const onScroll = () => {
      if (!mq.matches) return;
      if (!frame) frame = requestAnimationFrame(update);
    };
    // An overlay opening while the bar is away brings it straight back
    // (its trigger may live in the bar, and menus keep clear of it).
    const observer = new MutationObserver(() => {
      if (overlayOpen()) update();
    });
    observer.observe(document.body, {
      attributes: true,
      attributeFilter: ["data-scroll-locked"],
    });
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  const reveal = () => {
    state.current = { ...state.current, hidden: false, acc: 0 };
    setHidden(false);
  };

  return { hidden, reveal };
}
