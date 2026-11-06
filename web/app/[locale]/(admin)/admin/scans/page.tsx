import { PageHeader } from "@/components/admin/PageHeader";
import { ScanDashboardClient } from "@/components/admin/library/ScanDashboardClient";

export default function ScansPage() {
  return (
    <>
      <PageHeader
        title="Scans"
        description="Watch scans live — one library or a whole “Scan all” run — with per-library progress, a summary of what changed when it finishes, and every past run."
      />
      <ScanDashboardClient />
    </>
  );
}
