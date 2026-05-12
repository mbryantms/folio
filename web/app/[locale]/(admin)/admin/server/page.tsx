import { PageHeader } from "@/components/admin/PageHeader";
import { ServerInfoClient } from "@/components/admin/observability/ServerInfoClient";

export default function ServerInfoPage() {
  return (
    <>
      <PageHeader
        title="Server info"
        description="Build SHA, uptime, Postgres + Redis health, scheduler status, and probe links. Polled every 15 seconds."
      />
      <ServerInfoClient />
    </>
  );
}
