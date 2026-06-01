"use client";

import { createContext, useContext } from "react";

/**
 * Ambient hint: when true, `<Cover>` images in this subtree eager-load with
 * high fetch priority (and skip the fade-in) instead of `loading="lazy"`.
 *
 * Used to prioritize the first, above-the-fold rail's covers on the home /
 * page-rails surface so the LCP element (the largest visible cover) isn't
 * deferred behind the lazy-load + preload-scanner deprioritization that gave
 * the home page a fast FCP but a slow LCP. Everything outside a provider keeps
 * the default lazy behavior, so no other surface changes.
 */
const CoverPriorityContext = createContext(false);

export const CoverPriorityProvider = CoverPriorityContext.Provider;

export function useCoverPriority(): boolean {
  return useContext(CoverPriorityContext);
}
