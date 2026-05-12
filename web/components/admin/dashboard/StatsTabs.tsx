"use client";

import dynamic from "next/dynamic";
import { useState } from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { StatsClient } from "./StatsClient";

// Each tab body loads its own recharts payload only when shown.
const UsersTab = dynamic(() => import("./UsersTab").then((m) => m.UsersTab), {
  ssr: false,
  loading: () => <Skeleton className="h-64 w-full" />,
});
const EngagementTab = dynamic(
  () => import("./EngagementTab").then((m) => m.EngagementTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const ContentTab = dynamic(
  () => import("./ContentTab").then((m) => m.ContentTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);
const QualityTab = dynamic(
  () => import("./QualityTab").then((m) => m.QualityTab),
  { ssr: false, loading: () => <Skeleton className="h-64 w-full" /> },
);

const TABS = [
  { value: "overview", label: "Overview" },
  { value: "users", label: "Users" },
  { value: "engagement", label: "Engagement" },
  { value: "content", label: "Content" },
  { value: "quality", label: "Quality" },
] as const;
type TabValue = (typeof TABS)[number]["value"];

export function StatsTabs() {
  const [tab, setTab] = useState<TabValue>("overview");
  return (
    <Tabs value={tab} onValueChange={(v) => setTab(v as TabValue)}>
      <TabsList>
        {TABS.map((t) => (
          <TabsTrigger key={t.value} value={t.value}>
            {t.label}
          </TabsTrigger>
        ))}
      </TabsList>
      <TabsContent value="overview" className="pt-4">
        <StatsClient />
      </TabsContent>
      <TabsContent value="users" className="pt-4">
        <UsersTab />
      </TabsContent>
      <TabsContent value="engagement" className="pt-4">
        <EngagementTab />
      </TabsContent>
      <TabsContent value="content" className="pt-4">
        <ContentTab />
      </TabsContent>
      <TabsContent value="quality" className="pt-4">
        <QualityTab />
      </TabsContent>
    </Tabs>
  );
}
