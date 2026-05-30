import { PageHeader } from "@/components/admin/PageHeader";
import { QueuePage } from "@/components/admin/queue/QueuePage";

export default function AdminQueuePage() {
  return (
    <>
      <PageHeader
        title="Queue"
        description="Background-job depth and recent archive operations. Pending counts poll live; archive edits list the audit trail the worker writes on completion."
      />
      <QueuePage />
    </>
  );
}
