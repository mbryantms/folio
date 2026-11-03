import { PageHeader } from "@/components/admin/PageHeader";
import { StarterListsAdmin } from "@/components/admin/StarterListsAdmin";

export default function StarterListsPage() {
  return (
    <>
      <PageHeader
        title="Starter lists"
        description="Reading lists the first-run tour offers new readers as a place to start. Hide picks that don't suit this server or add your own from the catalog."
      />
      <StarterListsAdmin />
    </>
  );
}
