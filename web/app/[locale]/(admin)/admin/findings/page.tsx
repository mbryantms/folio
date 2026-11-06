import { PageHeader } from "@/components/admin/PageHeader";
import { FindingsView } from "@/components/admin/findings/FindingsView";

export default function FindingsPage() {
  return (
    <>
      <PageHeader
        title="Library events"
        description="The Library stream: an itemized log of every change (issues, series, thumbnails, metadata, archives) plus open health issues, across every library. Scans live on the Scans page."
      />
      <FindingsView />
    </>
  );
}
