import { PageHeader } from "@/components/admin/PageHeader";
import { PagesManager } from "@/components/pages-manager/PagesManager";
import { HiddenRailItems } from "@/components/settings/HiddenRailItems";

export default function PagesSettingsPage() {
  return (
    <>
      <PageHeader
        title="Pages"
        description="Create pages, edit descriptions, manage which saved views show as rails, reorder how they appear in the sidebar, and restore items hidden from the home rails."
      />
      <PagesManager />
      <div className="mt-10">
        <HiddenRailItems />
      </div>
    </>
  );
}
