import { PageHeader } from "@/components/admin/PageHeader";
import { UpcomingAdmin } from "@/components/admin/upcoming/UpcomingAdmin";

/** Upcoming releases: the release calendar's switches and sync state. */
export default function AdminUpcomingPage() {
  return (
    <>
      <PageHeader
        title="Upcoming releases"
        description="Which issues go on sale in the coming weeks, as Metron lists them. Folio reads this; it never orders, downloads or changes anything."
      />
      <UpcomingAdmin />
    </>
  );
}
