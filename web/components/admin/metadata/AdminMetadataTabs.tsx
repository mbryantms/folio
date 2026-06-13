"use client";

/**
 * Tab shell for `/admin/metadata`. Five tabs:
 *   - Dashboard — counts + quota gauges (M6)
 *   - Providers — per-provider test buttons + credential forms (M6)
 *   - Review — bulk-fetch result queue, deep-linkable via `?batch=`
 *   - Runs — paginated metadata_run history with detail drilldown (M6)
 *   - Settings — weekly-refresh toggle + cron + staleness (M7 follow-up)
 *
 * Folio has no generic `/admin/settings` page, so per-feature settings
 * need their own forms (the Settings tab here).
 */

import dynamic from "next/dynamic";
import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { DashboardTab } from "./DashboardTab";

const ProvidersTab = dynamic(
  () => import("./ProvidersTab").then((m) => m.ProvidersTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const AutoSyncedTab = dynamic(
  () => import("./AutoSyncedTab").then((m) => m.AutoSyncedTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const RunsTab = dynamic(() => import("./RunsTab").then((m) => m.RunsTab), {
  ssr: false,
  loading: () => <Skeleton className="h-64 w-full" />,
});
const SettingsTab = dynamic(
  () => import("./SettingsTab").then((m) => m.SettingsTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const ReviewTab = dynamic(
  () => import("./ReviewTab").then((m) => m.ReviewTab),
  {
    ssr: false,
    loading: () => <Skeleton className="h-64 w-full" />,
  },
);

const TABS = [
  { value: "dashboard", label: "Dashboard" },
  { value: "review", label: "Review" },
  { value: "providers", label: "Providers" },
  { value: "auto-synced", label: "Auto-synced" },
  { value: "runs", label: "Runs" },
  { value: "settings", label: "Settings" },
] as const;
type TabValue = (typeof TABS)[number]["value"];

export function AdminMetadataTabs() {
  // Deep-link support: `?tab=review&batch=<id>` from the bulk-fetch triggers.
  const params = useSearchParams();
  const deepTab = params.get("tab");
  const initialTab = TABS.some((t) => t.value === deepTab)
    ? (deepTab as TabValue)
    : "dashboard";
  const initialBatchId = params.get("batch");
  const [tab, setTab] = useState<TabValue>(initialTab);
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
      <TabsContent value="review" className="pt-4">
        <ReviewTab initialBatchId={initialBatchId} />
      </TabsContent>
      <TabsContent value="providers" className="pt-4">
        <ProvidersTab />
      </TabsContent>
      <TabsContent value="auto-synced" className="pt-4">
        <AutoSyncedTab />
      </TabsContent>
      <TabsContent value="runs" className="pt-4">
        <RunsTab />
      </TabsContent>
      <TabsContent value="settings" className="pt-4">
        <SettingsTab />
      </TabsContent>
    </Tabs>
  );
}
