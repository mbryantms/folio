import Link from "next/link";

import { PageHeader } from "@/components/admin/PageHeader";
import { DownloadsList } from "@/components/offline/DownloadsList";
import { SettingsSection } from "@/components/settings/SettingsSection";
import { Button } from "@/components/ui/button";

/** Settings → Downloads: offline copies on this device (WP-4.6). */
export default function DownloadsSettingsPage() {
  return (
    <>
      <PageHeader
        title="Downloads"
        description="Issues stored on this device for reading without a connection. Downloads belong to your account on this browser: signing out removes them."
        actions={
          <Button variant="outline" asChild>
            <Link href="/downloads">Open offline library</Link>
          </Button>
        }
      />
      <SettingsSection
        title="On this device"
        description="Sizes are what each download occupies in this browser's storage. Removing a download keeps your reading progress."
      >
        <DownloadsList />
      </SettingsSection>
    </>
  );
}
