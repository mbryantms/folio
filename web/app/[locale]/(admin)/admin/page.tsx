import { PageHeader } from "@/components/admin/PageHeader";
import { DashboardClient } from "@/components/admin/dashboard/DashboardClient";

export default async function AdminDashboardPage() {
  return (
    <>
      <PageHeader
        title="Dashboard"
        description="Library totals, scan health, recent activity, and live service status."
      />
      <DashboardClient />
    </>
  );
}
