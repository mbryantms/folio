import { PageHeader } from "@/components/admin/PageHeader";
import { UrlViewTabs } from "@/components/admin/UrlViewTabs";
import { ActivityFeedClient } from "@/components/admin/observability/ActivityFeedClient";
import { LogsClient } from "@/components/admin/observability/LogsClient";

/** The Server stream in one page: Activity (audit + aggregate reading
 *  volume) and Log (`?view=log`, the in-process runtime log tail). */
export default function ActivityPage() {
  return (
    <>
      <PageHeader
        title="Server activity"
        description="The Server stream: who did what (audit), aggregate reading volume, and the live runtime log. Library scans, health, and changes live in Scans and Library events."
      />
      <UrlViewTabs
        label="Server activity views"
        views={[
          {
            value: "activity",
            label: "Activity",
            content: <ActivityFeedClient />,
          },
          { value: "log", label: "Log", content: <LogsClient /> },
        ]}
      />
    </>
  );
}
