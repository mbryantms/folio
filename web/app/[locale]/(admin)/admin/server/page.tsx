import { PageHeader } from "@/components/admin/PageHeader";
import { ServerSettingsCards } from "@/components/admin/server/ServerSettingsCards";
import { ServerInfoClient } from "@/components/admin/observability/ServerInfoClient";

export default function ServerInfoPage() {
  return (
    <div className="space-y-6">
      <div>
        <PageHeader
          title="Server info"
          description="Build SHA, uptime, Postgres + Redis health, scheduler status, and probe links. Polled every 15 seconds."
        />
        <ServerInfoClient />
      </div>
      <ServerSettingsCards />
    </div>
  );
}
