import { PageHeader } from "@/components/admin/PageHeader";
import { LogsClient } from "@/components/admin/observability/LogsClient";

export default function LogsPage() {
  return (
    <>
      <PageHeader
        title="Logs"
        description="In-process structured-log tail. Triage-grade — bounded buffer, lost on restart."
      />
      <LogsClient />
    </>
  );
}
