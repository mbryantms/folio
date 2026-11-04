import { PageHeader } from "@/components/admin/PageHeader";
import { QueuePage } from "@/components/admin/queue/QueuePage";

export default function AdminQueuePage() {
  return (
    <>
      <PageHeader
        title="Queue"
        description="Every background job as it moves from queued to running to done, live. Failed jobs can be retried; archive edits list the audit trail the worker writes on completion."
      />
      <QueuePage />
    </>
  );
}
