import { PageHeader } from "@/components/admin/PageHeader";
import { ActivityFeedClient } from "@/components/admin/observability/ActivityFeedClient";

export default function ActivityPage() {
  return (
    <>
      <PageHeader
        title="Activity"
        description="Combined feed of audit entries, scan runs, open health issues, and aggregate reading volume. Reading entries are aggregated per hour — never per-user."
      />
      <ActivityFeedClient />
    </>
  );
}
