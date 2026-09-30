import { PageHeader } from "@/components/admin/PageHeader";
import { ScanDashboardClient } from "@/components/admin/library/ScanDashboardClient";

export default function ScanDashboardPage() {
  return (
    <>
      <PageHeader
        title="Scan dashboard"
        description="File-watcher mode and last trigger per library, plus live progress across a 'Scan all' run — per-library status, overall completion, and a post-run summary of what changed."
      />
      <ScanDashboardClient />
    </>
  );
}
