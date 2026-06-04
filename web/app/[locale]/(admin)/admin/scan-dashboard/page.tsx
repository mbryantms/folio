import { PageHeader } from "@/components/admin/PageHeader";
import { ScanDashboardClient } from "@/components/admin/library/ScanDashboardClient";

export default function ScanDashboardPage() {
  return (
    <>
      <PageHeader
        title="Scan dashboard"
        description="Live progress across a 'Scan all' run — per-library status, overall completion, and a post-run summary of what changed."
      />
      <ScanDashboardClient />
    </>
  );
}
