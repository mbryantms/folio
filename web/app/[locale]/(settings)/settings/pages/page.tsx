import { PageHeader } from "@/components/admin/PageHeader";
import { PagesManager } from "@/components/pages-manager/PagesManager";

export default function PagesSettingsPage() {
  return (
    <>
      <PageHeader
        title="Pages"
        description="Create pages, edit descriptions, manage which saved views show as rails, and reorder how they appear in the sidebar."
      />
      <PagesManager />
    </>
  );
}
