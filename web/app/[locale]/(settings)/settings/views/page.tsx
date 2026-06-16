import { PageHeader } from "@/components/admin/PageHeader";
import { SavedViewsManager } from "@/components/saved-views/SavedViewsManager";

export default function SavedViewsSettingsPage() {
  return (
    <>
      <PageHeader
        title="Views"
        description="Arrange where your saved views appear — pin them to pages and toggle sidebar visibility. Create, import, and edit from the Views library."
      />
      <SavedViewsManager />
    </>
  );
}
