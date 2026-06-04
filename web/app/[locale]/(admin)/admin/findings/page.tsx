import { PageHeader } from "@/components/admin/PageHeader";
import { FindingsView } from "@/components/admin/findings/FindingsView";

export default async function FindingsPage() {
  return (
    <>
      <PageHeader
        title="Library activity"
        description="The Library stream: an itemized log of every change (issues, series, thumbnails, metadata, archives), plus open health issues and recent scan runs — across every library."
      />
      <FindingsView />
    </>
  );
}
