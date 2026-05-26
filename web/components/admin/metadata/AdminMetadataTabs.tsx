"use client";

/**
 * Tab shell for `/admin/metadata` (M6). Four tabs:
 *   - Dashboard — counts + quota gauges
 *   - Providers — per-provider test buttons
 *   - Review queue — pending medium/low candidates
 *   - Runs — paginated metadata_run history with detail drilldown
 *
 * Settings (provider priority, threshold, cache TTLs, weekly refresh)
 * live on `/admin/settings` rather than here — the unified settings
 * page already surfaces every `metadata.*` key; duplicating the form
 * surface would just rot.
 */

import dynamic from "next/dynamic";
import { useState } from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { DashboardTab } from "./DashboardTab";

const ProvidersTab = dynamic(
  () => import("./ProvidersTab").then((m) => m.ProvidersTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const ReviewQueueTab = dynamic(
  () => import("./ReviewQueueTab").then((m) => m.ReviewQueueTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const RunsTab = dynamic(() => import("./RunsTab").then((m) => m.RunsTab), {
  ssr: false,
  loading: () => <Skeleton className="h-64 w-full" />,
});

const TABS = [
  { value: "dashboard", label: "Dashboard" },
  { value: "providers", label: "Providers" },
  { value: "review", label: "Review queue" },
  { value: "runs", label: "Runs" },
] as const;
type TabValue = (typeof TABS)[number]["value"];

export function AdminMetadataTabs() {
  const [tab, setTab] = useState<TabValue>("dashboard");
  return (
    <Tabs value={tab} onValueChange={(v) => setTab(v as TabValue)}>
      <TabsList>
        {TABS.map((t) => (
          <TabsTrigger key={t.value} value={t.value}>
            {t.label}
          </TabsTrigger>
        ))}
      </TabsList>
      <TabsContent value="dashboard" className="pt-4">
        <DashboardTab />
      </TabsContent>
      <TabsContent value="providers" className="pt-4">
        <ProvidersTab />
      </TabsContent>
      <TabsContent value="review" className="pt-4">
        <ReviewQueueTab />
      </TabsContent>
      <TabsContent value="runs" className="pt-4">
        <RunsTab />
      </TabsContent>
    </Tabs>
  );
}
