import { PageHeader } from "@/components/admin/PageHeader";
import { LibraryList } from "@/components/admin/library/LibraryList";
import { NewLibraryDialog } from "@/components/admin/library/NewLibraryDialog";

export default async function LibrariesPage() {
  return (
    <>
      <PageHeader
        title="Libraries"
        description="Folders Folio scans for comics. Each library has its own settings, schedule, and health view."
        actions={<NewLibraryDialog />}
      />
      <LibraryList />
    </>
  );
}
