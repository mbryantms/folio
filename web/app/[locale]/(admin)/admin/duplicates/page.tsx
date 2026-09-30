import { PageHeader } from "@/components/admin/PageHeader";
import { DuplicatesOverview } from "@/components/admin/library/DuplicatesOverview";

export default async function DuplicatesPage() {
  return (
    <>
      <PageHeader
        title="Duplicates"
        description="Repacks and extra copies inside each library — identical files, issues sharing a series and number, and near-identical covers. Keep, soft-remove, or open the editor."
      />
      <DuplicatesOverview />
    </>
  );
}
