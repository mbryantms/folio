import { PageHeader } from "@/components/admin/PageHeader";
import { StatsTabs } from "@/components/admin/dashboard/StatsTabs";

export default async function StatsPage() {
  return (
    <>
      <PageHeader
        title="Stats"
        description="Overview, per-user aggregates, engagement curves, content insights, and data-quality diagnostics."
      />
      <StatsTabs />
    </>
  );
}
