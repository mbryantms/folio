import { PageHeader } from "@/components/admin/PageHeader";
import { FindingsView } from "@/components/admin/findings/FindingsView";

export default async function FindingsPage() {
  return (
    <>
      <PageHeader
        title="Findings"
        description="Open health issues, recent scan runs, and live scan progress — aggregated across every library."
      />
      <FindingsView />
    </>
  );
}
