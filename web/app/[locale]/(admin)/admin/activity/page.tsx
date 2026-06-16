import { PageHeader } from "@/components/admin/PageHeader";
import { ActivityFeedClient } from "@/components/admin/observability/ActivityFeedClient";

export default function ActivityPage() {
  return (
    <>
      <PageHeader
        title="Server activity"
        description="The Server stream: audit entries (who did what) and aggregate reading volume (per-hour, never per-user). Library scans, health, and changes live in Library events."
      />
      <ActivityFeedClient />
    </>
  );
}
