import { PageHeader } from "@/components/admin/PageHeader";
import { ActivityPanel } from "@/components/settings/ActivityPanel";

export default async function ActivityPage() {
  return (
    <>
      <PageHeader
        title="Activity"
        description="Your reading totals, heatmaps, pace, rankings, rereads, and recent sessions. Privacy controls and history deletion sit at the bottom."
      />
      <ActivityPanel />
    </>
  );
}
