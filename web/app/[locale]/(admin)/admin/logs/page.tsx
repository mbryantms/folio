import { PageHeader } from "@/components/admin/PageHeader";
import { LogsClient } from "@/components/admin/observability/LogsClient";

export default function LogsPage() {
  return (
    <>
      <PageHeader
        title="Server log"
        description="In-process app-runtime log tail (Server stream) — filter by stream, level, and error code to triage. Triage-grade: bounded buffer, lost on restart. Library scanner/worker events live in Library activity."
      />
      <LogsClient />
    </>
  );
}
