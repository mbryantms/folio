import { PageHeader } from "@/components/admin/PageHeader";
import { UrlViewTabs } from "@/components/admin/UrlViewTabs";
import { BackgroundWorkClient } from "@/components/admin/background/BackgroundWorkClient";
import { QueuePage } from "@/components/admin/queue/QueuePage";

/** Everything the server is doing: Overview (per-library + per-queue
 *  totals) and Jobs (`?view=jobs`: every job card, failed jobs, archive
 *  operations). Scans link onward to the Scans page / a library's Scan
 *  tab. */
export default function BackgroundWorkPage() {
  return (
    <>
      <PageHeader
        title="Background work"
        description="Everything the server is doing right now: scans, thumbnail and cover generation, content hashing, metadata and archive jobs — per library, per queue, and job by job."
      />
      <UrlViewTabs
        label="Background work views"
        views={[
          {
            value: "overview",
            label: "Overview",
            content: <BackgroundWorkClient />,
          },
          { value: "jobs", label: "Jobs", content: <QueuePage /> },
        ]}
      />
    </>
  );
}
