"use client";

import * as React from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/folio/tabs";

export type UrlView = {
  value: string;
  label: string;
  content: React.ReactNode;
};

/**
 * Top-level tabs for a page with more than one view (Server activity,
 * Background work, Scans history): the active view lives in `?<param>=`
 * so other pages can link straight to it. Switching views drops the other view's query
 * params — each view owns its own filters — and the first view is the
 * default (its value is left out of the URL).
 */
export function UrlViewTabs({
  views,
  param = "view",
  label,
}: {
  views: UrlView[];
  param?: string;
  /** Accessible name for the tab list. */
  label: string;
}) {
  const router = useRouter();
  const pathname = usePathname();
  const sp = useSearchParams();
  const fallback = views[0]?.value ?? "";
  const raw = sp.get(param);
  const active = views.some((v) => v.value === raw) ? raw! : fallback;

  function select(next: string) {
    if (next === active) return;
    const qs = next === fallback ? "" : `?${param}=${encodeURIComponent(next)}`;
    router.replace(`${pathname}${qs}`, { scroll: false });
  }

  return (
    <Tabs value={active} onValueChange={select}>
      <TabsList aria-label={label}>
        {views.map((v) => (
          <TabsTrigger key={v.value} value={v.value}>
            {v.label}
          </TabsTrigger>
        ))}
      </TabsList>
      {views.map((v) => (
        <TabsContent key={v.value} value={v.value} className="mt-4">
          {v.content}
        </TabsContent>
      ))}
    </Tabs>
  );
}
