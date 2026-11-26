import { PageHeader } from "@/components/admin/PageHeader";
import { PagesManager } from "@/components/pages-manager/PagesManager";
import { HiddenRailItems } from "@/components/settings/HiddenRailItems";

export default function PagesSettingsPage() {
  return (
    <>
      <PageHeader
        title="Pages"
        description="Create pages, edit descriptions, choose and order the saved views each page shows as rails, and restore items hidden from the home rails. Rails can also be moved from the page itself: each rail has a ⋯ menu, and the page menu has Rearrange rails."
      />
      <PagesManager />
      <div className="mt-10">
        <HiddenRailItems />
      </div>
    </>
  );
}
