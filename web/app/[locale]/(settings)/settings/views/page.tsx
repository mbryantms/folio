import { PageHeader } from "@/components/admin/PageHeader";
import { SavedViewsManager } from "@/components/saved-views/SavedViewsManager";

export default function SavedViewsSettingsPage() {
  return (
    <>
      <PageHeader
        title="Saved views"
        description="Arrange where your saved views appear — pin them to pages and toggle sidebar visibility. Create, import, and edit under Views."
      />
      <SavedViewsManager />
    </>
  );
}
