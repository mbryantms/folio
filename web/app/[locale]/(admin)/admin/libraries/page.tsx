import { PageHeader } from "@/components/admin/PageHeader";
import { LibraryList } from "@/components/admin/library/LibraryList";
import { NewLibraryDialog } from "@/components/admin/library/NewLibraryDialog";
import { ScanAllButton } from "@/components/admin/library/ScanAllButton";

export default async function LibrariesPage() {
  return (
    <>
      <PageHeader
        title="Libraries"
        description="Folders Folio scans for comics. Each library has its own settings, schedule, and health view."
        actions={
          <div className="flex items-center gap-2">
            <ScanAllButton />
            <NewLibraryDialog />
          </div>
        }
      />
      <LibraryList />
    </>
  );
}
