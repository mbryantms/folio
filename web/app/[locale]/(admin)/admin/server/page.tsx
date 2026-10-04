import Link from "next/link";

import { PageHeader } from "@/components/admin/PageHeader";
import { Button } from "@/components/ui/button";
import { ServerSettingsCards } from "@/components/admin/server/ServerSettingsCards";
import { ServerInfoClient } from "@/components/admin/observability/ServerInfoClient";

export default function ServerInfoPage() {
  return (
    <div className="space-y-6">
      <div>
        <PageHeader
          title="Server info"
          description="Build SHA, uptime, Postgres + Redis health, scheduler status, and probe links. Polled every 15 seconds."
          actions={
            // Temporary: in-app entry to the iOS status-bar experiments
            // (the installed app has no address bar). Remove with
            // `app/[locale]/pwa-lab`.
            <Button asChild variant="outline" size="sm">
              <Link href="/pwa-lab">PWA lab</Link>
            </Button>
          }
        />
        <ServerInfoClient />
      </div>
      <ServerSettingsCards />
    </div>
  );
}
